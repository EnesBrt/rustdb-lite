//! Recursive CTE work queues. The working table contains one dequeued row.
use super::*;
use alloc::collections::VecDeque;

fn direct_references(core: &QueryCore, name: &str) -> usize {
    match core {
        QueryCore::Select(select) => select
            .sources
            .iter()
            .filter(|s| s.query.is_none() && !s.qualified && s.name.eq_ignore_ascii_case(name))
            .count(),
        QueryCore::Values(_) => 0,
    }
}
pub(super) fn has_direct_reference(table: &CommonTable) -> bool {
    !table
        .query
        .with
        .iter()
        .any(|t| t.name.eq_ignore_ascii_case(&table.name))
        && table
            .query
            .cores
            .iter()
            .any(|c| direct_references(c, &table.name) > 0)
}
struct Queue {
    rows: VecDeque<Candidate>,
    seen: Vec<Vec<Value>>,
    bytes: usize,
    seen_bytes: usize,
    distinct: bool,
    collations: Vec<Option<Collation>>,
    order: Vec<Ordering>,
}
impl Queue {
    fn push(
        &mut self,
        values: Vec<Value>,
        context: &mut Eval<'_>,
        output_bytes: usize,
    ) -> Result<()> {
        context.fuel.spend()?;
        if self.distinct {
            for prior in &self.seen {
                context.fuel.spend()?;
                if compare_rows(prior, &values, &self.collations, context.encoding)?
                    == Compare::Equal
                {
                    return Ok(());
                }
            }
            push_row(
                &mut self.seen,
                values.clone(),
                &mut self.seen_bytes,
                context.limits,
            )?;
        }
        let keys = evaluate_list(self.order.iter().map(|o| &o.expr), &values, None, context)?;
        let bytes = values_size(&values)?
            .checked_add(values_size(&keys)?)
            .ok_or(Error::Limit("recursive queue bytes"))?;
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or(Error::Limit("recursive queue bytes"))?;
        let live = self
            .bytes
            .checked_add(self.seen_bytes)
            .and_then(|n| n.checked_add(output_bytes))
            .ok_or(Error::Limit("recursive query bytes"))?;
        if self.rows.len() >= context.limits.max_rows || live > context.limits.max_database_bytes {
            return Err(Error::Limit("recursive query rows/bytes"));
        }
        self.rows.push_back(Candidate { values, keys });
        Ok(())
    }
    fn pop(&mut self, context: &mut Eval<'_>) -> Result<Option<Vec<Value>>> {
        let mut first = 0;
        if !self.order.is_empty() {
            for i in 1..self.rows.len() {
                context.fuel.spend()?;
                if compare_keys(
                    &self.rows[i].keys,
                    &self.rows[first].keys,
                    &self.order,
                    context.encoding,
                )? == Compare::Less
                {
                    first = i;
                }
            }
        }
        let Some(row) = self.rows.remove(first) else {
            return Ok(None);
        };
        self.bytes = self
            .bytes
            .saturating_sub(values_size(&row.values)? + values_size(&row.keys)?);
        Ok(Some(row.values))
    }
}
impl Connection {
    pub(in super::super) fn recursive_query(
        &self,
        table: &CommonTable,
        context: &mut Eval<'_>,
        scope: Option<Rc<Scope>>,
        runtime: &mut Runtime,
        schema_only: bool,
        row_cap: Option<usize>,
    ) -> Result<Data> {
        if runtime.depth >= context.limits.max_expr_depth.min(32) {
            return Err(Error::Limit("query nesting"));
        }
        runtime.depth += 1;
        let result =
            self.recursive_query_inner(table, context, scope, runtime, schema_only, row_cap);
        runtime.depth -= 1;
        result
    }
    fn recursive_query_inner(
        &self,
        table: &CommonTable,
        context: &mut Eval<'_>,
        scope: Option<Rc<Scope>>,
        runtime: &mut Runtime,
        schema_only: bool,
        row_cap: Option<usize>,
    ) -> Result<Data> {
        let query = &table.query;
        let start = query
            .cores
            .iter()
            .position(|c| direct_references(c, &table.name) > 0)
            .ok_or_else(|| error("missing recursive term"))?;
        if start == 0 {
            return Err(error("recursive query needs an initial SELECT"));
        }
        let operator = query.operators[start - 1];
        if !matches!(operator, Compound::Union | Compound::UnionAll)
            || query.operators[start..].iter().any(|op| *op != operator)
        {
            return Err(error("recursive terms require the same UNION operator"));
        }
        for core in &query.cores[start..] {
            if direct_references(core, &table.name) != 1 {
                return Err(error(
                    "recursive table must occur once in each recursive SELECT",
                ));
            }
            if let QueryCore::Select(select) = core {
                if !select.group.is_empty() || select.having.is_some() {
                    return Err(error("recursive aggregate queries are not supported"));
                }
            }
        }
        let cache_key = (
            table.id,
            scope.as_ref().map_or(0, |s| s.id),
            if table.materialized != Some(false) {
                0
            } else {
                runtime.returning_site
            },
        );
        let scope = Scope::extend(scope, &query.with, runtime)?;
        let limit = self.expressions(scope.clone(), runtime).limit(
            query.limit.as_ref(),
            false,
            schema_only,
            context,
        )?;
        let limit = match (limit, row_cap) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let offset = self
            .expressions(scope.clone(), runtime)
            .limit(query.offset.as_ref(), true, schema_only, context)?
            .unwrap_or(0);
        let schema_only = schema_only || limit == Some(0);
        let initial = Query {
            with: Vec::new(),
            cores: query.cores[..start].to_vec(),
            operators: query.operators[..start - 1].to_vec(),
            order: Vec::new(),
            limit: None,
            offset: None,
        };
        let mut data = self.query(&initial, context, scope.clone(), runtime, schema_only)?;
        let columns = table
            .columns
            .clone()
            .unwrap_or_else(|| data.result.columns.clone());
        if columns.len() != data.result.columns.len() {
            return Err(error("common table column count does not match query"));
        }
        let mut shape = Data {
            nested: None,
            fields: data.fields.clone(),
            projection: data.projection.clone(),
            column_types: data.column_types.clone(),
            result: QueryResult {
                columns: columns.clone(),
                rows: Vec::new(),
                changes: 0,
            },
        };
        runtime
            .cache
            .insert(cache_key, (Rc::new(shape.clone()), false));
        runtime.recursive.push((table.id, runtime.depth));
        let result = (|| {
            // ORDER BY may refer to an alias/expression in any initial SELECT,
            // including one whose metadata the combined anchor no longer has.
            let mut parts = Vec::new();
            for (index, core) in query.cores.iter().enumerate() {
                let part = self.query_core(core, context, scope.clone(), runtime, true)?;
                if part.result.columns.len() != columns.len() {
                    return Err(error("recursive SELECTs have different column counts"));
                }
                if index >= start && part.projection.iter().any(eval::has_aggregate) {
                    return Err(error("recursive aggregate queries are not supported"));
                }
                parts.push(part);
            }
            let order = compound_order(&query.order, &parts, &query.cores, |expr, fields| {
                if runtime.rename.is_some() {
                    self.expressions(scope.clone(), runtime).bind(
                        expr,
                        fields,
                        &[],
                        true,
                        context,
                    )?;
                }
                Ok(())
            })?;
            let mut types = CompoundTypes::default();
            for part in &parts {
                types.add_data(part, context)?;
            }
            let collations = types.comparison_collations();
            data.column_types = Some(types);
            shape.column_types = data.column_types.clone();
            if let Some(last) = parts.last() {
                data.fields = last.fields.clone();
                data.projection = last.projection.clone();
                shape.fields = data.fields.clone();
                shape.projection = data.projection.clone();
            }
            if schema_only {
                data.result.rows.clear();
                return Ok(());
            }
            let mut queue = Queue {
                rows: VecDeque::new(),
                seen: Vec::new(),
                bytes: 0,
                seen_bytes: 0,
                distinct: operator == Compound::Union,
                collations,
                order,
            };
            for row in core::mem::take(&mut data.result.rows) {
                queue.push(row, context, 0)?;
            }
            let mut output = Vec::new();
            let mut output_bytes = 0;
            let mut skipped = 0;
            while let Some(row) = queue.pop(context)? {
                context.fuel.spend()?;
                if skipped < offset {
                    skipped += 1;
                } else {
                    push_row(&mut output, row.clone(), &mut output_bytes, context.limits)?;
                    if limit.is_some_and(|n| output.len() >= n) {
                        break;
                    }
                }
                let mut working = shape.clone();
                working.result.rows.push(row);
                runtime.cache.insert(cache_key, (Rc::new(working), false));
                for core in &query.cores[start..] {
                    let generated =
                        self.query_core(core, context, scope.clone(), runtime, false)?;
                    for row in generated.result.rows {
                        queue.push(row, context, output_bytes)?;
                    }
                }
            }
            data.result.rows = output;
            Ok(())
        })();
        runtime.recursive.pop();
        runtime.cache.remove(&cache_key);
        result?;
        Ok(data)
    }
}
