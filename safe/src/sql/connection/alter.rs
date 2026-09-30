//! Atomic column edits. Reparse edited CREATE statements through the ordinary
//! schema validator, retaining row identities and stored generated values.
use super::*;
use crate::sql::lexer::{lex, Kind};
use parser::{Alter, Binary, Unary};
mod constraints;

struct ColumnSpan {
    start: usize,
    end: usize,
    comma: usize,
}
fn column_spans(sql: &str, count: usize, limits: SqlLimits) -> Result<Vec<ColumnSpan>> {
    let tokens = lex(sql, limits)?;
    let mut depth = 0usize;
    let mut start = None;
    let mut comma = 0;
    let mut spans = Vec::new();
    for token in tokens {
        if depth == 1 && matches!(token.kind, Kind::Symbol("," | ")")) {
            if let Some(start) = start.take() {
                spans.push(ColumnSpan {
                    start,
                    end: token.start,
                    comma,
                });
                if spans.len() == count {
                    return Ok(spans);
                }
            }
            comma = token.start;
        } else if depth == 1 && start.is_none() {
            start = Some(token.start);
        }
        match token.kind {
            Kind::Symbol("(") => depth += 1,
            Kind::Symbol(")") => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Err(Error::Corrupt("missing table column spans"))
}

/// sqlite3ValueFromExpr accepts literal, signed and CAST defaults for missing
/// record fields. Functions and arithmetic are evaluated on future INSERTs,
/// but cannot supply a value for existing records during ADD COLUMN.
pub(super) fn record_default(
    expr: &Expr,
    affinity: Affinity,
    context: &mut Eval<'_>,
) -> Result<Option<Value>> {
    context.fuel.spend()?;
    let value = match &expr.kind {
        ExprKind::Literal(Value::Real(_)) | ExprKind::MinMagnitude
            if affinity == Affinity::Text && expr.token.is_some() =>
        {
            Value::Text(Text::utf8(expr.token.as_deref().unwrap_or("")))
        }
        ExprKind::Literal(v) => v.clone(),
        ExprKind::MinMagnitude => Value::Real(9223372036854775808.0),
        ExprKind::Boolean(v) => Value::Integer(i64::from(*v)),
        ExprKind::Column {
            qualifier: None,
            name,
            quoted: false,
        } if name.eq_ignore_ascii_case("true") || name.eq_ignore_ascii_case("false") => {
            Value::Integer(i64::from(name.eq_ignore_ascii_case("true")))
        }
        ExprKind::Unary(Unary::Plus, child) => return record_default(child, affinity, context),
        ExprKind::Unary(Unary::Minus, child)
            if affinity == Affinity::Text
                && matches!(
                    child.kind,
                    ExprKind::Literal(Value::Real(_)) | ExprKind::MinMagnitude
                )
                && child.token.is_some() =>
        {
            Value::Text(Text::utf8(&format!(
                "-{}",
                child.token.as_deref().unwrap_or("")
            )))
        }
        ExprKind::Unary(Unary::Minus, child) if matches!(child.kind, ExprKind::MinMagnitude) => {
            Value::Integer(i64::MIN)
        }
        ExprKind::Unary(Unary::Minus, child) => {
            let Some(value) = record_default(child, affinity, context)? else {
                return Ok(None);
            };
            let signed = Expr {
                kind: ExprKind::Unary(Unary::Minus, alloc::boxed::Box::new(Expr::literal(value))),
                depth: 2,
                token: None,
            };
            context.eval(&signed, &[], None)?
        }
        ExprKind::Cast(child, cast) => {
            let Some(value) = record_default(child, *cast, context)? else {
                return Ok(None);
            };
            let cast = Expr {
                kind: ExprKind::Cast(alloc::boxed::Box::new(Expr::literal(value)), *cast),
                depth: 2,
                token: None,
            };
            context.eval(&cast, &[], None)?
        }
        _ => return Ok(None),
    };
    let value = scalar::affinity(value, affinity)?;
    if scalar::size(&value) > context.limits.max_value_bytes {
        return Err(Error::Limit("column default size"));
    }
    Ok(Some(value))
}
fn references(expr: &Expr, column: usize, fuel: &mut Fuel) -> Result<bool> {
    fuel.spend()?;
    if matches!(expr.kind, ExprKind::Slot(i, ..) if i == column) {
        return Ok(true);
    }
    for child in expr.children() {
        if references(child, column, fuel)? {
            return Ok(true);
        }
    }
    Ok(false)
}
fn drop_dependencies(table: &StoredTable, column: usize, fuel: &mut Fuel) -> Result<()> {
    if table.primary_key.contains(&column) {
        return Err(error("cannot drop PRIMARY KEY column"));
    }
    for index in &table.indexes {
        for term in &index.terms {
            let used = match &term.key {
                IndexKey::Column(i) => *i == column,
                IndexKey::Expression(e) => references(&e.signature, column, fuel)?,
            };
            if used {
                return Err(error(format!("column is used by index {}", index.name)));
            }
        }
        if let Some(predicate) = &index.predicate {
            if references(&predicate.signature, column, fuel)? {
                return Err(error(format!("column is used by index {}", index.name)));
            }
        }
    }
    if let Some(schema) = &table.generated {
        for (i, generated) in schema.columns.iter().enumerate() {
            if i != column {
                if let Some(expr) = &generated.expression {
                    if references(expr, column, fuel)? {
                        return Err(error("column is used by generated expression"));
                    }
                }
            }
        }
    }
    Ok(())
}
impl Connection {
    pub(super) fn alter_table(
        &mut self,
        name: &str,
        action: &Alter,
        context: &mut Eval<'_>,
    ) -> Result<QueryResult> {
        let id = self.index(name)?;
        let old = &self.state.tables[id];
        if old.name.to_ascii_lowercase().starts_with("sqlite_") {
            return Err(error("system tables may not be altered"));
        }
        let spans = column_spans(&old.sql, old.columns.len(), self.limits)?;
        let mut sql = old.sql.clone();
        let mut dropped = None;
        let mut value = Value::Null;
        let mut validate = false;
        let mut result_columns = Vec::new();
        match action {
            Alter::Add {
                column,
                sql: definition,
            } => {
                if column.primary || column.unique {
                    return Err(error("cannot add a PRIMARY KEY or UNIQUE column"));
                }
                let mut guarded_error = None;
                if column.generated.as_ref().is_some_and(|(_, stored)| *stored) {
                    guarded_error = Some("cannot add a STORED column");
                }
                if column.generated.is_none() {
                    if column.not_null
                        && column
                            .default
                            .as_ref()
                            .is_none_or(|e| matches!(e.kind, ExprKind::Literal(Value::Null)))
                    {
                        guarded_error =
                            Some("Cannot add a NOT NULL column with default value NULL");
                    }
                    if let Some(default) = &column.default {
                        let affinity = if old.strict && column.declared_type == "ANY" {
                            Affinity::Blob
                        } else {
                            column.affinity
                        };
                        match record_default(default, affinity, context)? {
                            Some(default) => value = default,
                            None => {
                                guarded_error
                                    .get_or_insert("Cannot add a column with non-constant default");
                            }
                        }
                    }
                }
                if let Some(message) = guarded_error {
                    if !old.rows.is_empty() {
                        return Err(error(message));
                    }
                    result_columns.push(format!("raise(ABORT,'{message}')"));
                }
                validate = !column.checks.is_empty()
                    || (column.not_null && column.generated.is_some())
                    || old.strict;
                if validate && result_columns.is_empty() {
                    result_columns.push("CASE WHEN quick_check GLOB 'CHECK*' THEN raise(ABORT,'CHECK constraint failed') WHEN quick_check GLOB 'non-* value in*' THEN raise(ABORT,'type mismatch on DEFAULT') ELSE raise(ABORT,'NOT NULL constraint failed') END".into());
                }
                let end = spans
                    .last()
                    .ok_or(Error::Corrupt("table has no columns"))?
                    .end;
                sql.insert_str(end, &format!(", {definition}"));
            }
            Alter::SetNotNull {
                column,
                sql: definition,
            } => {
                let n = real_column(old, column)?;
                let fields = old.fields(&old.name);
                let bound = eval::generated::field(&fields[n], n, None)?;
                for (&id, values) in &old.rows {
                    context.fuel.spend()?;
                    if scalar::null(&context.eval(&bound, &old.row(id, values), None)?) {
                        return Err(Error::Constraint("NOT NULL constraint failed".into()));
                    }
                }
                sql = constraints::drop(
                    &sql,
                    constraints::Target::NotNull(n),
                    self.limits,
                    context.fuel,
                )?;
                sql = constraints::add(&sql, definition, Some(n), self.limits, context.fuel)?;
                result_columns.push("sqlite_fail('constraint failed', 19)".into());
            }
            Alter::DropNotNull(column) => {
                sql = constraints::drop(
                    &sql,
                    constraints::Target::NotNull(real_column(old, column)?),
                    self.limits,
                    context.fuel,
                )?;
            }
            Alter::DropConstraint(name) => {
                sql = constraints::drop(
                    &sql,
                    constraints::Target::Name(name),
                    self.limits,
                    context.fuel,
                )?;
            }
            Alter::AddCheck {
                name,
                expression,
                sql: definition,
            } => {
                if contains_parameter(expression) {
                    return Err(error("parameters prohibited in schema"));
                }
                let condition = Expr {
                    kind: ExprKind::Binary(
                        Binary::IsNot,
                        expression.clone(),
                        alloc::boxed::Box::new(Expr {
                            kind: ExprKind::Column {
                                qualifier: None,
                                name: "TRUE".into(),
                                quoted: false,
                            },
                            depth: 1,
                            token: None,
                        }),
                    ),
                    depth: expression.depth + 1,
                    token: None,
                };
                let bound = eval::bind(&condition, &old.fields(&old.name), &[], false)?;
                if let Some(name) = name {
                    if constraints::has_name(&sql, name, self.limits, context.fuel)? {
                        return Err(error(format!("constraint {name} already exists")));
                    }
                    result_columns.push(format!(
                        "sqlite_fail('constraint {} already exists', 1)",
                        name.replace('\'', "''")
                    ));
                } else {
                    result_columns.push("sqlite_fail('constraint failed', 19)".into());
                }
                // A table-independent WHERE condition is evaluated before the
                // scan, even when there are no records to validate.
                if !row_dependent(&bound) {
                    context.condition(&bound, &[], true, false)?;
                }
                for (&id, values) in &old.rows {
                    context.fuel.spend()?;
                    if context.condition(&bound, &old.row(id, values), true, false)? {
                        return Err(Error::Constraint("CHECK constraint failed".into()));
                    }
                }
                sql = constraints::add(&sql, definition, None, self.limits, context.fuel)?;
            }
            Alter::Drop(column) => {
                let n = old
                    .columns
                    .iter()
                    .position(|c| c.name.eq_ignore_ascii_case(column))
                    .ok_or_else(|| error(format!("no such column: {column}")))?;
                if old.columns.len() == 1 {
                    return Err(error("cannot drop the only column"));
                }
                drop_dependencies(old, n, context.fuel)?;
                self.alter_validate_views(context)?;
                let range = if n + 1 < spans.len() {
                    spans[n].start..spans[n + 1].start
                } else {
                    spans[n].comma..spans[n].end
                };
                sql.replace_range(range, "");
                dropped = Some(n);
            }
        }
        // Build a replacement in isolation. Explicit indexes are rebound against
        // its new column positions; their existing SQL and automatic names stay.
        let mut builder = Self::with_limits(self.limits);
        builder.schema_reload = true;
        let prepared = builder.prepare(&sql)?;
        builder.run(
            &prepared.statement,
            context,
            None,
            &mut query::Runtime::default(),
        )?;
        for index in &self.state.tables[id].indexes {
            context.fuel.spend()?;
            if let Some(sql) = &index.sql {
                let prepared = builder.prepare(sql)?;
                builder.run(
                    &prepared.statement,
                    context,
                    None,
                    &mut query::Runtime::default(),
                )?;
            }
        }
        let new_id = builder.index(name)?;
        let mut table = builder.state.tables.remove(new_id);
        let expected = match action {
            Alter::Drop(_) => self.state.tables[id].columns.len() - 1,
            Alter::Add { .. } => self.state.tables[id].columns.len() + 1,
            _ => self.state.tables[id].columns.len(),
        };
        if table.columns.len() != expected {
            return Err(error("ALTER definition has an unexpected column count"));
        }
        let mut bytes = 0usize;
        for (&key, values) in &self.state.tables[id].rows {
            context.fuel.spend()?;
            let mut values = values.clone();
            if let Some(n) = dropped {
                values.remove(n);
            } else if matches!(action, Alter::Add { .. }) {
                values.push(value.clone());
            }
            bytes = bytes
                .checked_add(values_size(&values)?)
                .ok_or(Error::Limit("altered table bytes"))?;
            if bytes > self.limits.max_database_bytes {
                return Err(Error::Limit("altered table bytes"));
            }
            table.rows.insert(key, values);
        }
        if validate {
            validate_rows(&table, context)?;
        }
        self.state.tables[id] = table;
        if dropped.is_some() {
            self.alter_validate_views(context)?;
        }
        Ok(QueryResult {
            columns: result_columns,
            rows: Vec::new(),
            changes: 0,
        })
    }
    fn alter_validate_views(&self, context: &mut Eval<'_>) -> Result<()> {
        for i in 0..self.state.views.len() {
            context.fuel.spend()?;
            self.view_fields(i, context)?;
        }
        Ok(())
    }
}
fn validate_rows(table: &StoredTable, context: &mut Eval<'_>) -> Result<()> {
    if table
        .generated
        .as_ref()
        .is_some_and(|s| s.read_depth.iter().any(Option::is_none))
    {
        return Err(error("generated column loop"));
    }
    let fields = table.fields(&table.name);
    let checks = table
        .checks
        .iter()
        .chain(table.columns.iter().flat_map(|c| &c.checks))
        .map(|e| eval::bind(e, &fields, &[], false))
        .collect::<Result<Vec<_>>>()?;
    for (&id, values) in &table.rows {
        context.fuel.spend()?;
        strict::values(table, values)?;
        for (i, column) in table.columns.iter().enumerate() {
            if column.not_null || (table.strict && column.virtual_column()) {
                let value = table.read_column(i, id, values, context)?;
                if column.not_null && scalar::null(&value) {
                    return Err(Error::Constraint("NOT NULL constraint failed".into()));
                }
            }
        }
        let row = table.row(id, values);
        for check in &checks {
            if !context.condition(check, &row, true, true)? {
                return Err(Error::Constraint("CHECK constraint failed".into()));
            }
        }
    }
    Ok(())
}

fn real_column(table: &StoredTable, name: &str) -> Result<usize> {
    table
        .columns
        .iter()
        .position(|c| c.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| error(format!("no such column: {name}")))
}

fn row_dependent(expr: &Expr) -> bool {
    matches!(
        expr.kind,
        ExprKind::Slot(..) | ExprKind::Generated(_) | ExprKind::Outer(..)
    ) || expr.children().iter().any(|e| row_dependent(e))
}
