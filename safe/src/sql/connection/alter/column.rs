//! Bound column renames and schema-wide double-quoted literal normalization.
use super::*;
use parser::Location;

#[derive(Default)]
struct Edits {
    names: BTreeMap<usize, usize>,
    strings: BTreeMap<usize, (usize, String)>,
}
impl Edits {
    fn mark(&mut self, location: Location) -> Result<()> {
        let (start, end) = location
            .0
            .ok_or(Error::Corrupt("missing column location"))?;
        self.names.insert(start, end);
        Ok(())
    }
    fn apply(self, sql: &str, new: &str, quoted: bool, context: &mut Eval<'_>) -> Result<String> {
        let quoted_name = format!("\"{}\"", new.replace('"', "\"\""));
        let mut edits = self
            .strings
            .into_iter()
            .map(|(start, (end, value))| {
                let mut value = format!("'{}'", value.replace('\'', "''"));
                if sql.as_bytes().get(end) == Some(&b'\'') {
                    value.push(' ');
                }
                (start, (end, value))
            })
            .collect::<BTreeMap<_, _>>();
        for (start, end) in self.names {
            context.fuel.spend()?;
            let old = sql
                .as_bytes()
                .get(start)
                .ok_or(Error::Corrupt("column token outside SQL"))?;
            let use_quotes = quoted || matches!(old, b'\'' | b'"' | b'[' | b'`');
            let mut value = if use_quotes {
                quoted_name.clone()
            } else {
                new.into()
            };
            if use_quotes && sql.as_bytes().get(end) == Some(&b'"') {
                value.push(' ');
            }
            if edits.insert(start, (end, value)).is_some() {
                return Err(Error::Corrupt("conflicting column edits"));
            }
        }
        let mut output = String::new();
        let mut at = 0;
        for (start, (end, value)) in edits {
            context.fuel.spend()?;
            let prefix = sql
                .get(at..start)
                .ok_or(Error::Corrupt("overlapping column edits"))?;
            if output
                .len()
                .saturating_add(prefix.len())
                .saturating_add(value.len())
                > context.limits.max_sql_bytes
            {
                return Err(Error::Limit("renamed schema bytes"));
            }
            output.push_str(prefix);
            output.push_str(&value);
            at = end;
        }
        let suffix = sql
            .get(at..)
            .ok_or(Error::Corrupt("column token outside SQL"))?;
        if output.len().saturating_add(suffix.len()) > context.limits.max_sql_bytes {
            return Err(Error::Limit("renamed schema bytes"));
        }
        output.push_str(suffix);
        Ok(output)
    }
}
struct Resolver<'a> {
    edits: &'a mut Edits,
    quotes: bool,
    fuel: &'a mut Fuel,
}
impl eval::Resolver for Resolver<'_> {
    fn double_quoted_strings(&self) -> bool {
        self.quotes
    }
    fn resolved(&mut self, expr: &Expr, field: &Field) -> Result<()> {
        self.fuel.spend()?;
        if field.rename_target {
            self.edits.mark(expr.location)?;
        }
        Ok(())
    }
    fn literal(&mut self, expr: &Expr, name: &str) -> Result<()> {
        self.fuel.spend()?;
        let (start, end) = expr
            .location
            .0
            .ok_or(Error::Corrupt("missing quoted literal location"))?;
        self.edits.strings.insert(start, (end, name.into()));
        Ok(())
    }
    fn column(&mut self, _: &Expr, _: Option<&str>, _: &str, _: bool) -> Result<Option<Expr>> {
        Ok(None)
    }
    fn query(&mut self, _: &parser::Subquery, _: &[Field]) -> Result<eval::BoundSubquery> {
        Err(error("subquery in schema expression"))
    }
}
#[derive(Clone, Copy)]
struct Rename<'a> {
    table: usize,
    column: usize,
    new: &'a str,
    quoted: bool,
}
impl Connection {
    pub(super) fn normalize_schema_literals(&mut self, context: &mut Eval<'_>) -> Result<()> {
        self.rewrite_column_schema(None, context)
    }
    pub(super) fn rename_column(
        &mut self,
        id: usize,
        old: &str,
        new: &str,
        quoted: bool,
        context: &mut Eval<'_>,
    ) -> Result<QueryResult> {
        let selected = real_column(&self.state.tables[id], old)?;
        if self.state.tables[id]
            .columns
            .iter()
            .enumerate()
            .any(|(i, c)| i != selected && c.name.eq_ignore_ascii_case(new))
        {
            return Err(error(format!("duplicate column name: {new}")));
        }
        self.rewrite_column_schema(
            Some(Rename {
                table: id,
                column: selected,
                new,
                quoted,
            }),
            context,
        )?;
        Ok(QueryResult::changed(0))
    }
    pub(super) fn validate_table_names(
        &self,
        table: &StoredTable,
        context: &mut Eval<'_>,
    ) -> Result<()> {
        self.rewrite_table_schema(table, None, false, context)
            .map(|_| ())
    }
    fn rewrite_table_schema(
        &self,
        old_table: &StoredTable,
        target: Option<Rename<'_>>,
        normalize: bool,
        context: &mut Eval<'_>,
    ) -> Result<(String, Vec<String>)> {
        let (new, quoted) = target.map_or(("", false), |r| (r.new, r.quoted));
        let mut edits = Edits::default();
        let mut fields = old_table.fields(&old_table.name);
        for (i, field) in fields.iter_mut().enumerate() {
            field.generated = None;
            field.rename_target = target.is_some_and(|r| {
                i == r.column || i == old_table.columns.len() && old_table.alias() == Some(r.column)
            });
        }
        let parsed = self.prepare(&old_table.sql)?;
        let Statement::Create {
            columns,
            constraints,
            ..
        } = parsed.statement
        else {
            return Err(Error::Corrupt("missing table definition"));
        };
        if let Some(target) = target {
            edits.mark(columns[target.column].location)?;
        }
        for constraint in &constraints {
            if let TableConstraint::Key { columns, .. } = constraint {
                for column in columns {
                    context.fuel.spend()?;
                    if target.is_some_and(|r| {
                        column
                            .name
                            .eq_ignore_ascii_case(&old_table.columns[r.column].name)
                    }) {
                        edits.mark(column.location)?;
                    }
                }
            }
        }
        for expr in columns
            .iter()
            .flat_map(|c| &c.checks)
            .chain(constraints.iter().filter_map(|c| {
                if let TableConstraint::Check(e) = c {
                    Some(e)
                } else {
                    None
                }
            }))
        {
            eval::bind_with(
                expr,
                &fields,
                &[],
                false,
                &mut Resolver {
                    edits: &mut edits,
                    quotes: normalize,
                    fuel: context.fuel,
                },
            )?;
        }
        for column in &columns {
            if let Some((expr, _)) = &column.generated {
                eval::bind_with(
                    expr,
                    &fields[..old_table.columns.len()],
                    &[],
                    false,
                    &mut Resolver {
                        edits: &mut edits,
                        quotes: normalize,
                        fuel: context.fuel,
                    },
                )?;
            }
        }
        let sql = edits.apply(&old_table.sql, new, quoted, context)?;
        let mut indexes = Vec::new();
        for old_index in &old_table.indexes {
            let Some(sql) = &old_index.sql else { continue };
            let parsed = self.prepare(sql)?;
            let Statement::CreateIndex {
                columns, predicate, ..
            } = parsed.statement
            else {
                return Err(Error::Corrupt("missing index definition"));
            };
            let mut edits = Edits::default();
            for column in columns {
                let mut expr = column.expr;
                let value = if let ExprKind::Collate(e, _) = &mut expr.kind {
                    e.as_mut()
                } else {
                    &mut expr
                };
                if let ExprKind::Literal(Value::Text(name)) = &value.kind {
                    value.kind = ExprKind::Column {
                        qualifier: None,
                        qualifier_location: Default::default(),
                        name: name.to_string()?,
                        quoted: true,
                        double_quoted: false,
                    };
                }
                eval::bind_with(
                    &expr,
                    &fields[..old_table.columns.len()],
                    &[],
                    false,
                    &mut Resolver {
                        edits: &mut edits,
                        quotes: false,
                        fuel: context.fuel,
                    },
                )?;
            }
            if let Some(predicate) = predicate {
                eval::bind_with(
                    &predicate,
                    &fields,
                    &[],
                    false,
                    &mut Resolver {
                        edits: &mut edits,
                        quotes: normalize,
                        fuel: context.fuel,
                    },
                )?;
            }
            indexes.push(edits.apply(sql, new, quoted, context)?);
        }
        Ok((sql, indexes))
    }
    fn rewrite_column_schema(
        &mut self,
        rename: Option<Rename<'_>>,
        context: &mut Eval<'_>,
    ) -> Result<()> {
        let table_name = rename.map(|r| self.state.tables[r.table].name.clone());
        let (new, quoted) = rename.map_or(("", false), |r| (r.new, r.quoted));
        let mut tables = Vec::new();
        for (index, old_table) in self.state.tables.iter().enumerate() {
            context.fuel.spend()?;
            let target = rename.filter(|r| r.table == index);
            let (sql, indexes) = self.rewrite_table_schema(old_table, target, true, context)?;
            if sql == old_table.sql
                && indexes
                    .iter()
                    .eq(old_table.indexes.iter().filter_map(|i| i.sql.as_ref()))
            {
                tables.push(old_table.clone());
                continue;
            }
            let mut builder = Self::with_limits(self.limits);
            builder.schema_reload = true;
            let parsed = builder.prepare(&sql)?;
            builder.run(
                &parsed.statement,
                context,
                None,
                &mut query::Runtime::default(),
            )?;
            for sql in indexes {
                let parsed = builder.prepare(&sql)?;
                builder.run(
                    &parsed.statement,
                    context,
                    None,
                    &mut query::Runtime::default(),
                )?;
            }
            let at = builder.index(&old_table.name)?;
            let mut table = builder.state.tables.remove(at);
            for (id, values) in &old_table.rows {
                context.fuel.spend()?;
                table.rows.insert(*id, values.clone());
            }
            tables.push(table);
        }
        let mut views = Vec::new();
        for view in &self.state.views {
            let parsed = self.prepare(&view.sql)?;
            let Statement::CreateView { query, .. } = parsed.statement else {
                return Err(Error::Corrupt("missing view definition"));
            };
            let trace = self.rename_references(
                &query,
                table_name.as_deref(),
                rename.map(|r| r.column),
                true,
                context,
            )?;
            let sql = Edits {
                names: trace.edits,
                strings: trace.strings,
            }
            .apply(&view.sql, new, quoted, context)?;
            let parsed = self.prepare(&sql)?;
            let Statement::CreateView { query, .. } = parsed.statement else {
                return Err(Error::Corrupt("missing renamed view"));
            };
            let mut view = view.clone();
            view.sql = sql;
            view.query = query;
            views.push(view);
        }
        self.state.tables = tables;
        self.state.views = views;
        self.alter_validate_views(context)
    }
}
