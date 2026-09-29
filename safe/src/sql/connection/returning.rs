//! Buffered RETURNING results, evaluated after each successful row change.
use super::*;
use alloc::rc::Rc;
use parser::SelectItem;

pub(super) struct Output {
    result: QueryResult,
    expressions: Vec<Expr>,
    scope: Option<Rc<query::Scope>>,
    runtime: query::Runtime,
    bytes: usize,
}
impl Output {
    pub(super) fn site(&mut self, site: usize) {
        // INSERT and each DO UPDATE clause compile distinct RETURNING programs.
        self.runtime.returning_site = site;
    }
    pub(super) fn bind(
        db: &Connection,
        table: &StoredTable,
        items: &[SelectItem],
        scope: Option<Rc<query::Scope>>,
        runtime: &query::Runtime,
        context: &mut Eval<'_>,
    ) -> Result<Self> {
        let mut output = Self {
            result: QueryResult::changed(0),
            expressions: Vec::new(),
            scope,
            runtime: runtime.for_returning(&table.name),
            bytes: 0,
        };
        if items.is_empty() {
            return Ok(output);
        }
        let fields = table.fields(&table.name);
        for item in items {
            context.fuel.spend()?;
            if let Some(expr) = &item.expr {
                let bound = db
                    .expressions(output.scope.clone(), &mut output.runtime)
                    .bind(expr, &fields, &[], false, context)?;
                let label = item.alias.clone().unwrap_or_else(|| {
                    if let (ExprKind::Column { .. }, ExprKind::Slot(slot, _, _)) =
                        (&expr.kind, &bound.kind)
                    {
                        if *slot == table.columns.len() {
                            table
                                .alias()
                                .map_or_else(|| "rowid".into(), |i| table.columns[i].name.clone())
                        } else {
                            fields[*slot].name.clone()
                        }
                    } else {
                        item.label.clone()
                    }
                });
                output.result.columns.push(label);
                output.expressions.push(bound);
            } else {
                if item.star.is_some() {
                    return Err(error("RETURNING may not use TABLE.* wildcards"));
                }
                for (i, field) in fields.iter().enumerate().filter(|(_, f)| !f.hidden) {
                    context.fuel.spend()?;
                    output.result.columns.push(field.name.clone());
                    output.expressions.push(Expr {
                        kind: ExprKind::Slot(i, field.affinity, field.collation),
                        depth: 1,
                    });
                }
            }
            if output.expressions.len() > 2000 {
                return Err(Error::Limit("RETURNING columns"));
            }
        }
        for column in &output.result.columns {
            output.bytes = output
                .bytes
                .checked_add(column.len())
                .ok_or(Error::Limit("RETURNING bytes"))?;
        }
        if output.bytes > context.limits.max_database_bytes {
            return Err(Error::Limit("RETURNING bytes"));
        }
        Ok(output)
    }
    pub(super) fn emit(
        &mut self,
        db: &Connection,
        id: i64,
        values: &[Value],
        runtime: &mut query::Runtime,
        context: &mut Eval<'_>,
    ) -> Result<()> {
        if self.expressions.is_empty() {
            return Ok(());
        }
        self.runtime.inherit_materialized(runtime, context.fuel)?;
        let mut row = values.to_vec();
        row.push(Value::Integer(id));
        let values = db
            .expressions(self.scope.clone(), &mut self.runtime)
            .values(self.expressions.iter(), &row, None, context)?;
        push_row(
            &mut self.result.rows,
            values,
            &mut self.bytes,
            context.limits,
        )?;
        runtime.inherit_materialized(&self.runtime, context.fuel)
    }
    pub(super) fn finish(mut self, changes: usize) -> QueryResult {
        self.result.changes = changes;
        self.result
    }
}
