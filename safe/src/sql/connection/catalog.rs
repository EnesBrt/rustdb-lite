//! PRAGMA table_list over the implemented main schema and empty temp schema.
use super::*;

impl Connection {
    pub(super) fn table_list(
        &self,
        schema: Option<&str>,
        filter: Option<&str>,
        context: &mut Eval<'_>,
    ) -> Result<QueryResult> {
        let matches = |name: &str| filter.is_none_or(|f| f.eq_ignore_ascii_case(name));
        let mut entries = Vec::new();
        if schema.is_none_or(|s| s.eq_ignore_ascii_case("main")) {
            for table in &self.state.tables {
                context.fuel.spend()?;
                if matches(&table.name) {
                    entries.push((
                        "main",
                        table.name.as_str(),
                        "table",
                        table.columns.len(),
                        table.strict,
                    ));
                }
            }
            for (i, view) in self.state.views.iter().enumerate() {
                context.fuel.spend()?;
                if matches(&view.name) {
                    // Native table_list reports zero columns for an unresolved
                    // view; resource limits still fail explicitly in this API.
                    let columns = match self.view_fields(i, context) {
                        Ok(fields) => fields.len(),
                        Err(Error::Sql(_) | Error::Unsupported(_)) => 0,
                        Err(error) => return Err(error),
                    };
                    entries.push(("main", view.name.as_str(), "view", columns, false));
                }
            }
            if matches("sqlite_master") {
                entries.push(("main", "sqlite_schema", "table", 5, false));
            }
        }
        if schema.is_none_or(|s| s.eq_ignore_ascii_case("temp")) && matches("sqlite_temp_master") {
            entries.push(("temp", "sqlite_temp_schema", "table", 5, false));
        }
        let text = |s: &str| Value::Text(Text::utf8(s));
        let mut rows = Vec::new();
        let mut bytes = 0;
        for (schema, name, kind, columns, strict) in entries {
            context.fuel.spend()?;
            let row = vec![
                text(schema),
                text(name),
                text(kind),
                Value::Integer(columns as i64),
                Value::Integer(0),
                Value::Integer(i64::from(strict)),
            ];
            if row
                .iter()
                .any(|v| scalar::size(v) > context.limits.max_value_bytes)
            {
                return Err(Error::Limit("metadata value bytes"));
            }
            push_row(&mut rows, row, &mut bytes, context.limits)?;
        }
        Ok(QueryResult {
            columns: ["schema", "name", "type", "ncol", "wr", "strict"]
                .into_iter()
                .map(String::from)
                .collect(),
            rows,
            changes: 0,
        })
    }
}
