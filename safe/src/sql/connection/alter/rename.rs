//! Rewrite resolved table references, preserving aliases, CTEs and source text.
use super::*;
use parser::Location;

struct Rewrite<'a> {
    old: &'a str,
    columns: &'a [String],
    edits: BTreeMap<usize, usize>,
    fuel: &'a mut Fuel,
}
impl Rewrite<'_> {
    fn mark(&mut self, location: Location) -> Result<()> {
        self.fuel.spend()?;
        let (start, end) = location
            .0
            .ok_or(Error::Corrupt("missing schema location"))?;
        self.edits.insert(start, end);
        Ok(())
    }
    fn expression(&mut self, expr: &Expr) -> Result<()> {
        self.fuel.spend()?;
        if let ExprKind::Column {
            qualifier: Some(name),
            qualifier_location,
            name: column,
            ..
        } = &expr.kind
        {
            if name.eq_ignore_ascii_case(self.old)
                && self.columns.iter().any(|c| c.eq_ignore_ascii_case(column))
            {
                self.mark(*qualifier_location)?;
            }
        }
        for child in expr.children() {
            self.expression(child)?;
        }
        Ok(())
    }
    fn finish(self, sql: &str, new: &str, limits: SqlLimits) -> Result<String> {
        let replacement = format!("\"{}\"", new.replace('"', "\"\""));
        let mut output = String::new();
        let mut at = 0;
        for (start, end) in self.edits {
            self.fuel.spend()?;
            let prefix = sql
                .get(at..start)
                .ok_or(Error::Corrupt("overlapping schema tokens"))?;
            if output
                .len()
                .saturating_add(prefix.len())
                .saturating_add(replacement.len())
                > limits.max_sql_bytes
            {
                return Err(Error::Limit("renamed schema bytes"));
            }
            output.push_str(prefix);
            output.push_str(&replacement);
            at = end;
        }
        let suffix = sql
            .get(at..)
            .ok_or(Error::Corrupt("schema token outside SQL"))?;
        if output.len().saturating_add(suffix.len()) > limits.max_sql_bytes {
            return Err(Error::Limit("renamed schema bytes"));
        }
        output.push_str(suffix);
        Ok(output)
    }
}
fn declaration(sql: &str, index: bool, limits: SqlLimits) -> Result<Location> {
    let tokens = lex(sql, limits)?;
    let at = if index {
        tokens
            .iter()
            .position(|t| matches!(&t.kind, Kind::Word(w, false) if w.eq_ignore_ascii_case("ON")))
            .and_then(|n| tokens.get(n + 1))
    } else {
        tokens
            .iter()
            .position(|t| t.kind == Kind::Symbol("("))
            .and_then(|n| n.checked_sub(1))
            .and_then(|n| tokens.get(n))
    }
    .ok_or(Error::Corrupt("schema declaration missing"))?;
    Ok(Location(Some((at.start, at.end))))
}
impl Connection {
    pub(super) fn rename_table(
        &mut self,
        id: usize,
        new: &str,
        context: &mut Eval<'_>,
    ) -> Result<QueryResult> {
        self.new_relation(new, false)?;
        self.alter_validate_views(context)?;
        let old = &self.state.tables[id];
        let name = old.name.clone();
        let mut names: Vec<_> = old.columns.iter().map(|c| c.name.clone()).collect();
        if !old.without_rowid {
            names.extend(["rowid", "_rowid_", "oid"].map(String::from));
        }
        let prepared = self.prepare(&old.sql)?;
        let Statement::Create {
            columns,
            constraints,
            ..
        } = prepared.statement
        else {
            return Err(Error::Corrupt("table declaration missing"));
        };
        let mut rewrite = Rewrite {
            old: &name,
            columns: &names,
            edits: BTreeMap::new(),
            fuel: context.fuel,
        };
        rewrite.mark(declaration(&old.sql, false, self.limits)?)?;
        for expr in
            columns
                .iter()
                .flat_map(|c| c.checks.iter())
                .chain(constraints.iter().filter_map(|c| {
                    if let TableConstraint::Check(e) = c {
                        Some(e)
                    } else {
                        None
                    }
                }))
        {
            rewrite.expression(expr)?;
        }
        let sql = rewrite.finish(&old.sql, new, self.limits)?;
        let mut builder = Self::with_limits(self.limits);
        builder.schema_reload = true;
        let parsed = builder.prepare(&sql)?;
        builder.run(
            &parsed.statement,
            context,
            None,
            &mut query::Runtime::default(),
        )?;
        for index in &old.indexes {
            if let Some(sql) = &index.sql {
                let mut rewrite = Rewrite {
                    old: &name,
                    columns: &names,
                    edits: BTreeMap::new(),
                    fuel: context.fuel,
                };
                rewrite.mark(declaration(sql, true, self.limits)?)?;
                let parsed = builder.prepare(sql)?;
                if let Statement::CreateIndex {
                    predicate: Some(predicate),
                    ..
                } = &parsed.statement
                {
                    rewrite.expression(predicate)?;
                }
                let sql = rewrite.finish(sql, new, self.limits)?;
                let parsed = builder.prepare(&sql)?;
                builder.run(
                    &parsed.statement,
                    context,
                    None,
                    &mut query::Runtime::default(),
                )?;
            }
        }
        let new_id = builder.index(new)?;
        let mut table = builder.state.tables.remove(new_id);
        for (key, values) in &old.rows {
            context.fuel.spend()?;
            table.rows.insert(*key, values.clone());
        }
        let mut views = Vec::new();
        for view in &self.state.views {
            let parsed = builder.prepare(&view.sql)?;
            let Statement::CreateView { query, .. } = parsed.statement else {
                return Err(Error::Corrupt("view declaration missing"));
            };
            let edits = self
                .rename_references(&query, Some(&name), None, false, context)?
                .edits;
            let rewrite = Rewrite {
                old: &name,
                columns: &names,
                edits,
                fuel: context.fuel,
            };
            let sql = rewrite.finish(&view.sql, new, self.limits)?;
            let parsed = builder.prepare(&sql)?;
            let Statement::CreateView { query, .. } = parsed.statement else {
                return Err(Error::Corrupt("renamed view declaration missing"));
            };
            views.push((sql, query));
        }
        self.state.tables[id] = table;
        for (view, (sql, query)) in self.state.views.iter_mut().zip(views) {
            view.sql = sql;
            view.query = query;
        }
        if let Ok(sequence) = self.index(sequence::NAME) {
            for row in self.state.tables[sequence].rows.values_mut() {
                context.fuel.spend()?;
                if matches!(row.first(), Some(Value::Text(text)) if text.bytes == name.as_bytes()) {
                    row[0] = Value::Text(Text::utf8(new));
                }
            }
        }
        self.alter_validate_views(context)?;
        Ok(QueryResult::changed(0))
    }
}
