//! Expression queries share execution fuel while retaining lexical row scopes.
use super::*;
use crate::sql::{
    eval::{BoundSubquery, Resolver, Subqueries},
    parser::{QueryMode, Subquery},
};

#[derive(Clone)]
pub(super) struct Frame {
    fields: Rc<[Field]>,
    row: Vec<Value>,
}
impl Runtime {
    fn capture_fields(&mut self, fields: &[Field], context: &mut Eval<'_>) -> Result<Rc<[Field]>> {
        for prior in &self.field_scopes {
            context.fuel.spend()?;
            if prior.len() != fields.len() {
                continue;
            }
            let mut same = true;
            for (a, b) in prior.iter().zip(fields) {
                context.fuel.spend()?;
                if a != b {
                    same = false;
                    break;
                }
            }
            if same {
                return Ok(prior.clone());
            }
        }
        let mut bytes = 0usize;
        for f in fields {
            bytes = bytes
                .checked_add(core::mem::size_of::<Field>())
                .and_then(|n| n.checked_add(f.extra_bytes()))
                .and_then(|n| n.checked_add(f.table.len()))
                .and_then(|n| n.checked_add(f.name.len()))
                .and_then(|n| n.checked_add(f.declared_type.len()))
                .ok_or(Error::Limit("query scope bytes"))?;
        }
        self.materialized_bytes = self
            .materialized_bytes
            .checked_add(bytes)
            .ok_or(Error::Limit("query scope bytes"))?;
        if self.materialized_bytes > context.limits.max_database_bytes {
            return Err(Error::Limit("query scope bytes"));
        }
        let fields: Rc<[Field]> = fields.into();
        self.field_scopes.push(fields.clone());
        Ok(fields)
    }
}

pub(in super::super) struct Expressions<'a> {
    db: &'a Connection,
    scope: Option<Rc<Scope>>,
    runtime: &'a mut Runtime,
    row_values: BTreeMap<usize, Rc<Vec<Vec<Value>>>>,
    row_value_bytes: usize,
}
impl Connection {
    pub(in super::super) fn expressions<'a>(
        &'a self,
        scope: Option<Rc<Scope>>,
        runtime: &'a mut Runtime,
    ) -> Expressions<'a> {
        Expressions {
            db: self,
            scope,
            runtime,
            row_values: BTreeMap::new(),
            row_value_bytes: 0,
        }
    }
}
impl Expressions<'_> {
    pub(in super::super) fn representative(
        &mut self,
        extrema: &[&Expr],
        rows: &[Vec<Value>],
        context: &mut Eval<'_>,
    ) -> Result<Option<usize>> {
        context.aggregate_representative(extrema, rows, self)
    }
    pub(in super::super) fn bind(
        &mut self,
        expr: &Expr,
        fields: &[Field],
        aliases: &[(String, Expr)],
        aggregate: bool,
        context: &mut Eval<'_>,
    ) -> Result<Expr> {
        eval::bind_with(
            expr,
            fields,
            aliases,
            aggregate,
            &mut Binding {
                expressions: self,
                context,
            },
        )
    }
    pub(in super::super) fn eval(
        &mut self,
        expr: &Expr,
        row: &[Value],
        group: Option<&[Vec<Value>]>,
        context: &mut Eval<'_>,
    ) -> Result<Value> {
        context.eval_with(expr, row, group, self)
    }
    pub(in super::super) fn values<'a>(
        &mut self,
        expr: impl Iterator<Item = &'a Expr>,
        row: &[Value],
        group: Option<&[Vec<Value>]>,
        context: &mut Eval<'_>,
    ) -> Result<Vec<Value>> {
        let mut values = Vec::new();
        let mut bytes = 0;
        for e in expr {
            let value = self.eval(e, row, group, context)?;
            push_value(&mut values, value, &mut bytes, context.limits)?;
        }
        Ok(values)
    }
    pub(in super::super) fn filter(
        &mut self,
        expr: Option<&Expr>,
        row: &[Value],
        context: &mut Eval<'_>,
    ) -> Result<bool> {
        expr.map(|e| {
            self.eval(e, row, None, context)
                .and_then(|v| scalar::truth(&v))
                .map(|v| v == Some(true))
        })
        .unwrap_or(Ok(true))
    }
    pub(in super::super) fn limit(
        &mut self,
        expr: Option<&Expr>,
        offset: bool,
        schema_only: bool,
        context: &mut Eval<'_>,
    ) -> Result<Option<usize>> {
        let Some(expr) = expr else { return Ok(None) };
        // LIMIT cannot see columns of either the current SELECT or outer SELECTs.
        let outer = core::mem::take(&mut self.runtime.outer);
        let result = (|| {
            let expr = self.bind(expr, &[], &[], false, context)?;
            if schema_only {
                return Ok(None);
            }
            let n = rowid(self.eval(&expr, &[], None, context)?)?;
            Ok(if n < 0 {
                if offset {
                    Some(0)
                } else {
                    None
                }
            } else {
                Some(usize::try_from(n).unwrap_or(usize::MAX))
            })
        })();
        self.runtime.outer = outer;
        result
    }
}
struct Binding<'a, 'b, 'c, 'd> {
    expressions: &'a mut Expressions<'b>,
    context: &'c mut Eval<'d>,
}
impl Resolver for Binding<'_, '_, '_, '_> {
    fn double_quoted_strings(&self) -> bool {
        true
    }
    fn literal(&mut self, expr: &Expr, name: &str) -> Result<()> {
        if let Some(rename) = &mut self.expressions.runtime.rename {
            if !rename.fix_quotes {
                return Ok(());
            }
            if let Some((start, end)) = expr.location.0 {
                self.context.fuel.spend()?;
                rename.strings.insert(start, (end, name.into()));
            }
        }
        Ok(())
    }
    fn resolved(&mut self, expr: &Expr, field: &Field) -> Result<()> {
        if field.rename_target {
            if let Some(rename) = &mut self.expressions.runtime.rename {
                self.context.fuel.spend()?;
                rename.reference(expr, false);
            }
        }
        Ok(())
    }
    fn column(
        &mut self,
        expr: &Expr,
        qualifier: Option<&str>,
        name: &str,
        mut shadowed: bool,
    ) -> Result<Option<Expr>> {
        for (frame_index, frame) in self.expressions.runtime.outer.iter().enumerate().rev() {
            if let Some(slot) = eval::resolve_field(&frame.fields, qualifier, name)? {
                let bound =
                    eval::field(&frame.fields, slot, Some(frame_index), qualifier.is_some())?;
                if frame.fields[slot].rename_target {
                    if let Some(rename) = &mut self.expressions.runtime.rename {
                        self.context.fuel.spend()?;
                        rename.reference(expr, shadowed);
                    }
                }
                fn columns(expr: &Expr, reads: &mut BTreeSet<(usize, usize)>) {
                    match &expr.kind {
                        ExprKind::Outer(frame, slot, ..) => {
                            reads.insert((*frame, *slot));
                        }
                        ExprKind::Generated(r) => {
                            if let Some(frame) = r.outer {
                                reads.insert((frame, r.offset + r.column));
                            }
                        }
                        _ => {
                            for child in expr.children() {
                                columns(child, reads);
                            }
                        }
                    }
                }
                columns(&bound, &mut self.expressions.runtime.outer_reads);
                return Ok(Some(bound));
            }
            shadowed |= qualifier
                .is_some_and(|q| frame.fields.iter().any(|f| f.table.eq_ignore_ascii_case(q)));
        }
        Ok(None)
    }
    fn query(&mut self, source: &Subquery, fields: &[Field]) -> Result<BoundSubquery> {
        let e = &mut self.expressions;
        let runtime = &mut e.runtime;
        if runtime.outer.len() >= 32 {
            return Err(Error::Limit("query nesting"));
        }
        let outer_depth = runtime.outer.len();
        let fields = runtime.capture_fields(fields, self.context)?;
        runtime.outer.push(Frame {
            fields: fields.clone(),
            row: Vec::new(),
        });
        let saved_cache = core::mem::take(&mut runtime.cache);
        let saved_reads = core::mem::take(&mut runtime.outer_reads);
        let saved_tables = core::mem::take(&mut runtime.tables_read);
        let result =
            e.db.query(&source.query, self.context, e.scope.clone(), runtime, true);
        let used = core::mem::replace(&mut runtime.outer_reads, saved_reads);
        let tables = core::mem::replace(&mut runtime.tables_read, saved_tables);
        runtime.tables_read.extend(tables.iter().cloned());
        let correlated = !used.is_empty() || runtime.returning_reads(&source.query, &e.scope);
        let dependencies = used
            .iter()
            .filter(|(frame, _)| *frame == outer_depth)
            .map(|(_, slot)| *slot)
            .collect();
        runtime
            .outer_reads
            .extend(used.into_iter().filter(|(frame, _)| *frame < outer_depth));
        runtime.cache = saved_cache;
        runtime.outer.pop();
        let data = result?;
        let columns: Vec<_> = data
            .projection
            .iter()
            .map(|e| eval::QueryColumn {
                affinity: eval::expr_affinity(e),
                collation: eval::collation_hint(e),
                explicit_collation: eval::explicit_collation(e),
                declared_type: eval::declared_type(e, &data.fields),
            })
            .collect();
        for column in &columns {
            runtime.materialized_bytes = runtime
                .materialized_bytes
                .checked_add(core::mem::size_of::<eval::QueryColumn>() + column.declared_type.len())
                .ok_or(Error::Limit("subquery column bytes"))?;
            if runtime.materialized_bytes > self.context.limits.max_database_bytes {
                return Err(Error::Limit("subquery column bytes"));
            }
        }
        Ok(BoundSubquery {
            source: source.clone(),
            fields,
            correlated,
            dependencies,
            tables: tables.into_iter().collect(),
            columns,
            affinity: data
                .projection
                .first()
                .map_or(Affinity::None, eval::expr_affinity),
            collation: data.projection.first().and_then(eval::collation_hint),
            declared_type: data
                .projection
                .first()
                .map_or_else(String::new, |e| eval::declared_type(e, &data.fields)),
        })
    }
}
impl Subqueries for Expressions<'_> {
    fn row_field(
        &mut self,
        bound: &BoundSubquery,
        column: usize,
        row: &[Value],
        context: &mut Eval<'_>,
    ) -> Result<Value> {
        let values = if let Some(values) = self.row_values.get(&bound.source.id) {
            values.clone()
        } else {
            let values = self.run(bound, row, context)?;
            for value in values.iter() {
                self.row_value_bytes = self
                    .row_value_bytes
                    .checked_add(values_size(value)?)
                    .ok_or(Error::Limit("assignment query bytes"))?;
                if self.row_value_bytes > context.limits.max_database_bytes {
                    return Err(Error::Limit("assignment query bytes"));
                }
            }
            self.row_values.insert(bound.source.id, values.clone());
            values
        };
        Ok(values
            .first()
            .and_then(|r| r.get(column))
            .cloned()
            .unwrap_or(Value::Null))
    }
    fn outer(&self, frame: usize, slot: usize) -> Result<Value> {
        self.runtime
            .outer
            .get(frame)
            .and_then(|f| f.row.get(slot))
            .cloned()
            .ok_or_else(|| error("missing outer query row"))
    }
    fn run(
        &mut self,
        bound: &BoundSubquery,
        row: &[Value],
        context: &mut Eval<'_>,
    ) -> Result<Rc<Vec<Vec<Value>>>> {
        if !bound.correlated {
            if let Some(rows) = self
                .runtime
                .scalar_cache
                .get(&(bound.source.id, self.runtime.returning_site))
            {
                return Ok(rows.clone());
            }
        }
        if self.runtime.outer.len() >= 32 {
            return Err(Error::Limit("query nesting"));
        }
        let outer_bytes = self
            .runtime
            .outer
            .iter()
            .try_fold(values_size(row)?, |n, frame| {
                n.checked_add(values_size(&frame.row)?)
                    .ok_or(Error::Limit("outer query row bytes"))
            })?;
        if outer_bytes > context.limits.max_database_bytes {
            return Err(Error::Limit("outer query row bytes"));
        }
        self.runtime.outer.push(Frame {
            fields: bound.fields.clone(),
            row: row.to_vec(),
        });
        let previous_scope = self.runtime.next_scope;
        let existence = self.runtime.existence;
        let result = (|| {
            let mut query = (*bound.source.query).clone();
            if bound.source.mode != QueryMode::Set {
                let limit = self.limit(query.limit.as_ref(), false, false, context)?;
                query.limit = Some(Expr::literal(Value::Integer(if limit == Some(0) {
                    0
                } else {
                    1
                })));
            }
            if bound.source.mode == QueryMode::Exists
                && query.operators.iter().all(|op| *op == Compound::UnionAll)
            {
                self.runtime.existence = Some(self.runtime.depth + 1);
                query.order.clear();
            }
            let data = self
                .db
                .query(&query, context, self.scope.clone(), self.runtime, false)?;
            if !bound.correlated {
                let data = self.runtime.own(data, context.limits)?;
                let data = Rc::try_unwrap(data).map_err(|_| error("shared subquery result"))?;
                let rows = Rc::new(data.result.rows);
                self.runtime
                    .scalar_cache
                    .insert((bound.source.id, self.runtime.returning_site), rows.clone());
                Ok(rows)
            } else {
                Ok(Rc::new(data.result.rows))
            }
        })();
        self.runtime.existence = existence;
        self.runtime
            .cache
            .retain(|(_, scope, _), _| *scope <= previous_scope);
        self.runtime.outer.pop();
        result
    }
}
