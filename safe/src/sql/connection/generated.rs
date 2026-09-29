//! Generated column declarations, dependency scheduling and write-time values.
use super::*;
use alloc::rc::Rc;
use eval::generated::{Column as GeneratedColumn, Schema};

pub(super) fn declaration(table: &mut StoredTable, fuel: &mut Fuel) -> Result<()> {
    if table.columns.iter().all(|c| c.generated.is_none()) {
        return Ok(());
    }
    if table.columns.iter().all(|c| c.generated.is_some()) {
        return Err(error("a table needs a non-generated column"));
    }
    let mut fields = table.fields(&table.name);
    fields.truncate(table.columns.len());
    let mut columns = Vec::new();
    let mut dependencies = Vec::new();
    for (i, column) in table.columns.iter().enumerate() {
        let mut deps = Vec::new();
        let expression = if let Some((expression, _)) = &column.generated {
            if column.default.is_some() || table.primary_key.contains(&i) {
                return Err(error(
                    "generated columns cannot have DEFAULT or PRIMARY KEY",
                ));
            }
            validate(expression, fuel)?;
            let bound = eval::bind_generated(expression, &fields)?;
            dependencies_of(&bound, &mut deps, fuel)?;
            Some(bound)
        } else {
            None
        };
        dependencies.push(deps);
        columns.push(GeneratedColumn {
            expression,
            virtual_column: column.virtual_column(),
            affinity: column.affinity,
            collation: column.collation,
            declared_type: column.declared_type.clone(),
        });
    }
    // Resolve dependencies once. A cycle entirely among VIRTUAL columns is
    // rejected by DDL. Cycles through STORED values can be read, but writing
    // requires computing every generated value and rejects those cycles too.
    let mut read_depth: Vec<_> = columns
        .iter()
        .map(|c| (!c.virtual_column).then_some(1usize))
        .collect();
    let mut available: Vec<_> = columns.iter().map(|c| c.expression.is_none()).collect();
    let mut order = Vec::new();
    loop {
        let mut progress = false;
        for (i, deps) in dependencies.iter().enumerate() {
            fuel.spend()?;
            if read_depth[i].is_none() && deps.iter().all(|d| read_depth[*d].is_some()) {
                let depth = deps
                    .iter()
                    .filter_map(|d| read_depth[*d])
                    .max()
                    .unwrap_or(0);
                read_depth[i] = Some(
                    depth.saturating_add(columns[i].expression.as_ref().map_or(1, |e| e.depth)),
                );
                progress = true;
            }
            if !available[i] && deps.iter().all(|d| available[*d]) {
                available[i] = true;
                order.push(i);
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }
    if read_depth.iter().any(Option::is_none) {
        return Err(error("generated column loop"));
    }
    table.generated = Some(Rc::new(Schema {
        columns,
        read_depth,
        write_order: available.iter().all(|v| *v).then_some(order),
        strict: table.strict,
        presence: if table.without_rowid {
            table.primary_key[0]
        } else {
            table.columns.len()
        },
        width: table.columns.len() + usize::from(!table.without_rowid),
    }));
    Ok(())
}
fn validate(expr: &Expr, fuel: &mut Fuel) -> Result<()> {
    fuel.spend()?;
    if matches!(
        expr.kind,
        ExprKind::Parameter(_)
            | ExprKind::Subquery(_)
            | ExprKind::BoundSubquery(_)
            | ExprKind::Column {
                qualifier: Some(_),
                ..
            }
    ) || matches!(&expr.kind, ExprKind::Call { name, .. } if ["changes", "total_changes", "last_insert_rowid"].iter().any(|n| name.eq_ignore_ascii_case(n)))
    {
        return Err(error("non-deterministic expression in generated column"));
    }
    for child in expr.children() {
        validate(child, fuel)?;
    }
    Ok(())
}
fn dependencies_of(expr: &Expr, deps: &mut Vec<usize>, fuel: &mut Fuel) -> Result<()> {
    fuel.spend()?;
    if let ExprKind::Slot(i, _, _) = expr.kind {
        if !deps.contains(&i) {
            deps.push(i);
        }
    }
    for child in expr.children() {
        dependencies_of(child, deps, fuel)?;
    }
    Ok(())
}
impl StoredTable {
    pub(super) fn changed_columns(
        &self,
        mut columns: Vec<usize>,
        fuel: &mut Fuel,
    ) -> Result<Vec<usize>> {
        let Some(schema) = &self.generated else {
            return Ok(columns);
        };
        if columns.contains(&self.columns.len()) {
            if let Some(alias) = self.alias() {
                columns.push(alias);
            }
        }
        if let Some(order) = &schema.write_order {
            for &i in order {
                let mut deps = Vec::new();
                dependencies_of(
                    schema.columns[i]
                        .expression
                        .as_ref()
                        .ok_or(Error::Corrupt("generated expression missing"))?,
                    &mut deps,
                    fuel,
                )?;
                if deps.iter().any(|d| columns.contains(d)) {
                    columns.push(i);
                }
            }
        }
        Ok(columns)
    }
    pub(super) fn write_column(&self, name: &str) -> Result<usize> {
        let i = self.column_index(name)?;
        if self.columns.get(i).is_some_and(|c| c.generated.is_some()) {
            return Err(error(format!("cannot write generated column: {name}")));
        }
        Ok(i)
    }
    pub(super) fn generated_plan(&self) -> Result<()> {
        if self
            .generated
            .as_ref()
            .is_some_and(|s| s.write_order.is_none())
        {
            return Err(error("generated column loop"));
        }
        Ok(())
    }
    pub(super) fn compute_generated(
        &self,
        values: &mut [Value],
        context: &mut Eval<'_>,
    ) -> Result<()> {
        let Some(schema) = &self.generated else {
            return Ok(());
        };
        let order = schema
            .write_order
            .as_ref()
            .ok_or_else(|| error("generated column loop"))?;
        // STRICT checks ordinary inputs before evaluating generated expressions.
        if self.strict {
            for (column, value) in self.columns.iter().zip(values.iter()) {
                if column.generated.is_none() {
                    eval::generated::typecheck(value, &column.declared_type)?;
                }
            }
        }
        for &i in order {
            context.fuel.spend()?;
            let column = &schema.columns[i];
            let expression = column
                .expression
                .as_ref()
                .ok_or(Error::Corrupt("generated expression missing"))?;
            values[i] = scalar::affinity(context.eval(expression, values, None)?, column.affinity)?;
            if self.strict && column.virtual_column {
                eval::generated::typecheck(&values[i], &column.declared_type)?;
            }
            if scalar::size(&values[i]) > context.limits.max_value_bytes
                || values_size(values)? > context.limits.max_database_bytes
            {
                return Err(Error::Limit("generated values size"));
            }
        }
        Ok(())
    }
    pub(super) fn read_column(
        &self,
        column: usize,
        id: i64,
        values: &[Value],
        context: &mut Eval<'_>,
    ) -> Result<Value> {
        if self.columns[column].virtual_column() {
            let reference = eval::generated::Reference {
                schema: self
                    .generated
                    .as_ref()
                    .ok_or(Error::Corrupt("generated schema missing"))?
                    .clone(),
                column,
                offset: 0,
                outer: None,
            };
            context.eval(
                &reference.expression(column, None)?,
                &self.row(id, values),
                None,
            )
        } else {
            Ok(values[column].clone())
        }
    }
}
