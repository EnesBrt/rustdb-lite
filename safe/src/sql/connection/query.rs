//! Scoped query sources and compound execution. Native SQLite is never invoked.
use super::*;
use alloc::{boxed::Box, collections::BTreeSet, rc::Rc};
mod expressions;
mod metadata;
mod recursive;
use metadata::{ColumnType, CompoundTypes};
use parser::{CommonTable, Compound, Query, QueryCore, Source};

pub(super) struct Scope {
    tables: Vec<Rc<CommonTable>>,
    parent: Option<Rc<Scope>>,
    id: usize,
}
impl Scope {
    pub(super) fn extend(
        parent: Option<Rc<Self>>,
        tables: &[CommonTable],
        runtime: &mut Runtime,
    ) -> Result<Option<Rc<Self>>> {
        if tables.is_empty() {
            Ok(parent)
        } else {
            runtime.next_scope = runtime
                .next_scope
                .checked_add(1)
                .ok_or(Error::Limit("query scopes"))?;
            Ok(Some(Rc::new(Self {
                tables: tables.iter().cloned().map(Rc::new).collect(),
                parent,
                id: runtime.next_scope,
            })))
        }
    }
    fn find(scope: &Option<Rc<Self>>, name: &str) -> Option<(Rc<CommonTable>, Rc<Self>)> {
        let mut at = scope.clone();
        while let Some(scope) = at {
            if let Some(table) = scope
                .tables
                .iter()
                .find(|t| t.name.eq_ignore_ascii_case(name))
            {
                return Some((table.clone(), scope.clone()));
            }
            at = scope.parent.clone();
        }
        None
    }
}
#[derive(Default)]
pub(super) struct Runtime {
    pub(super) rename: Option<RenameTrace>,
    // Declaration, lexical scope and RETURNING program; the value flag marks
    // explicit MATERIALIZED results shared with the data-changing program.
    cache: BTreeMap<(usize, usize, usize), (Rc<Data>, bool)>,
    next_scope: usize,
    active: Vec<usize>,
    depth: usize,
    materialized_bytes: usize,
    recursive: Vec<(usize, usize)>,
    outer: Vec<expressions::Frame>,
    outer_reads: BTreeSet<(usize, usize)>,
    tables_read: BTreeSet<String>,
    scalar_cache: BTreeMap<(usize, usize), Rc<Vec<Vec<Value>>>>,
    field_scopes: Vec<Rc<[Field]>>,
    views: Vec<usize>,
    pub(super) existence: Option<usize>,
    returning_table: Option<String>,
    pub(super) returning_site: usize,
}
pub(super) struct RenameTrace {
    pub(super) table: Option<String>,
    pub(super) column: Option<usize>,
    pub(super) fix_quotes: bool,
    pub(super) edits: BTreeMap<usize, usize>,
    pub(super) strings: BTreeMap<usize, (usize, String)>,
    pending: Vec<(Box<Query>, Option<Rc<Scope>>)>,
    seen: BTreeSet<usize>,
}
impl RenameTrace {
    pub(super) fn new(table: Option<&str>, column: Option<usize>, fix_quotes: bool) -> Self {
        Self {
            table: table.map(Into::into),
            column,
            fix_quotes,
            edits: BTreeMap::new(),
            strings: BTreeMap::new(),
            pending: Vec::new(),
            seen: BTreeSet::new(),
        }
    }
    pub(super) fn mark(&mut self, location: parser::Location) {
        if let Some((start, end)) = location.0 {
            self.edits.insert(start, end);
        }
    }
    pub(super) fn reference(&mut self, expr: &Expr, shadowed: bool) {
        if self.column.is_some() {
            self.mark(expr.location);
        } else if !shadowed {
            if let ExprKind::Column {
                qualifier_location, ..
            } = &expr.kind
            {
                self.mark(*qualifier_location);
            }
        }
    }
}
impl Runtime {
    pub(super) fn inherit_materialized(&mut self, other: &Self, fuel: &mut Fuel) -> Result<()> {
        self.next_scope = self.next_scope.max(other.next_scope);
        for (key, (data, materialized)) in &other.cache {
            fuel.spend()?;
            if *materialized {
                self.cache
                    .entry(*key)
                    .or_insert_with(|| (data.clone(), true));
            }
        }
        Ok(())
    }
    pub(super) fn for_returning(&self, table: &str) -> Self {
        Self {
            next_scope: self.next_scope,
            returning_table: Some(table.into()),
            ..Self::default()
        }
    }
    fn returning_reads(&self, query: &Query, scope: &Option<Rc<Scope>>) -> bool {
        let Some(table) = &self.returning_table else {
            return false;
        };
        // SQLite marks the directly referenced SELECT, not its enclosing
        // expressions, derived sources, views or CTEs. The final compound term
        // supplies the SELECT node associated with a scalar expression.
        let Some(QueryCore::Select(select)) = query.cores.last() else {
            return false;
        };
        select.sources.iter().any(|source| {
            source.query.is_none()
                && source.name.eq_ignore_ascii_case(table)
                && (source.qualified
                    || (!query
                        .with
                        .iter()
                        .any(|cte| cte.name.eq_ignore_ascii_case(table))
                        && Scope::find(scope, table).is_none()))
        })
    }
}
#[derive(Clone)]
pub(super) struct Data {
    pub(super) result: QueryResult,
    pub(super) fields: Vec<Field>,
    pub(super) projection: Vec<Expr>,
    pub(super) column_types: Option<CompoundTypes>,
    pub(super) nested: Option<Vec<eval::NestedField>>,
}
impl Data {
    fn comparison_collations(&self) -> Vec<Option<Collation>> {
        self.column_types.as_ref().map_or_else(
            || self.projection.iter().map(eval::collation_hint).collect(),
            CompoundTypes::comparison_collations,
        )
    }
    fn source_fields(&self, alias: &str) -> Vec<Field> {
        let mut fields = self.fields(alias);
        // For a nested view/derived source SQLite follows the right-most
        // projection to find a declared type. Its affinity still comes from
        // the compound column, and can differ from that expression's affinity.
        for (field, expr) in fields.iter_mut().zip(&self.projection) {
            field.declared_type = eval::declared_type(expr, &self.fields);
        }
        fields
    }
    pub(super) fn fields(&self, alias: &str) -> Vec<Field> {
        let column_types = self.column_types.as_ref().map(CompoundTypes::columns);
        let mut names: Vec<String> = Vec::new();
        let mut used = BTreeSet::new();
        let mut suffixes: BTreeMap<String, usize> = BTreeMap::new();
        for name in &self.result.columns {
            let mut candidate = name.clone();
            while !used.insert(candidate.to_ascii_lowercase()) {
                let base = name
                    .rsplit_once(':')
                    .filter(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                    .map_or(name.as_str(), |(s, _)| s);
                let suffix = suffixes.entry(base.to_ascii_lowercase()).or_insert(1);
                candidate = format!("{base}:{suffix}");
                *suffix += 1;
            }
            names.push(candidate);
        }
        names
            .into_iter()
            .zip(&self.projection)
            .enumerate()
            .map(|(i, (name, e))| {
                let typ = column_types
                    .as_ref()
                    .map(|t| t[i].clone())
                    .unwrap_or_else(|| ColumnType::expression(e, &self.fields));
                Field {
                    rename_target: false,
                    nested: self.nested.as_ref().map(|fields| fields[i].clone()),
                    merged: Vec::new(),
                    generated: None,
                    table: alias.into(),
                    name,
                    affinity: typ.affinity,
                    collation: typ.collation,
                    hidden: false,
                    qualified_only: false,
                    unqualified_hidden: false,
                    declared_type: typ.declared_type,
                }
            })
            .collect()
    }
}
pub(super) enum SourceData<'a> {
    Table(&'a StoredTable, Option<Vec<i64>>),
    Query(Rc<Data>),
}
impl SourceData<'_> {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Table(t, _) => t.rows.len(),
            Self::Query(q) => q.result.rows.len(),
        }
    }
    pub(super) fn fields(&self, alias: &str) -> Vec<Field> {
        match self {
            Self::Table(t, _) => t.fields(alias),
            Self::Query(q) => q.source_fields(alias),
        }
    }
    pub(super) fn rows(&self) -> Box<dyn Iterator<Item = Vec<Value>> + '_> {
        match self {
            Self::Table(t, Some(ids)) => Box::new(ids.iter().map(|id| t.row(*id, &t.rows[id]))),
            Self::Table(t, None) => Box::new(t.rows.iter().map(|(id, v)| t.row(*id, v))),
            Self::Query(q) => Box::new(q.result.rows.iter().cloned()),
        }
    }
}
impl Runtime {
    pub(super) fn existence_here(&self) -> bool {
        self.existence == Some(self.depth)
    }
    fn own(&mut self, data: Data, limits: SqlLimits) -> Result<Rc<Data>> {
        let mut bytes = 0usize;
        for name in &data.result.columns {
            bytes = bytes
                .checked_add(name.len())
                .ok_or(Error::Limit("materialized query bytes"))?;
        }
        for row in &data.result.rows {
            bytes = bytes
                .checked_add(values_size(row)?)
                .ok_or(Error::Limit("materialized query bytes"))?;
        }
        self.materialized_bytes = self
            .materialized_bytes
            .checked_add(bytes)
            .ok_or(Error::Limit("materialized query bytes"))?;
        if self.materialized_bytes > limits.max_database_bytes {
            return Err(Error::Limit("materialized query bytes"));
        }
        Ok(Rc::new(data))
    }
}
impl Connection {
    pub(super) fn rename_references(
        &self,
        query: &Query,
        table: Option<&str>,
        column: Option<usize>,
        fix_quotes: bool,
        context: &mut Eval<'_>,
    ) -> Result<RenameTrace> {
        let mut runtime = Runtime {
            rename: Some(RenameTrace::new(table, column, fix_quotes)),
            ..Runtime::default()
        };
        self.query(query, context, None, &mut runtime, true)?;
        let mut trace = runtime
            .rename
            .take()
            .ok_or(Error::Corrupt("missing rename trace"))?;
        // SQLite also visits unused CTE declarations. Their semantic errors do
        // not invalidate a view, but successfully resolved references still
        // participate in the edit. Each declaration is visited at most once.
        while let Some((query, scope)) = trace.pending.pop() {
            context.fuel.spend()?;
            let mut runtime = Runtime {
                rename: Some(trace),
                ..Runtime::default()
            };
            if let Err(error @ Error::Limit(_)) =
                self.query(&query, context, scope, &mut runtime, true)
            {
                return Err(error);
            }
            trace = runtime
                .rename
                .take()
                .ok_or(Error::Corrupt("missing CTE rename trace"))?;
        }
        Ok(trace)
    }
    pub(super) fn view_query(
        &self,
        index: usize,
        context: &mut Eval<'_>,
        runtime: &mut Runtime,
        schema_only: bool,
    ) -> Result<Data> {
        let mut data = self.view_definition(index, context, runtime, schema_only)?;
        if let Some(columns) = &self.state.views[index].columns {
            if columns.len() != data.result.columns.len() {
                return Err(error("view column count does not match query"));
            }
            data.result.columns = columns.clone();
        }
        data.result.columns = data.fields("").into_iter().map(|f| f.name).collect();
        Ok(data)
    }
    pub(super) fn view_fields(&self, index: usize, context: &mut Eval<'_>) -> Result<Vec<Field>> {
        let mut data = self.view_definition(index, context, &mut Runtime::default(), true)?;
        if let Some(columns) = &self.state.views[index].columns {
            // SQLite exposes the declared names with empty types for a width
            // mismatch. Reading rows from that same view still fails.
            if columns.len() != data.result.columns.len() {
                data.projection = columns.iter().map(|_| Expr::literal(Value::Null)).collect();
                data.column_types = None;
            }
            data.result.columns = columns.clone();
        }
        Ok(data.fields(""))
    }
    fn view_definition(
        &self,
        index: usize,
        context: &mut Eval<'_>,
        runtime: &mut Runtime,
        schema_only: bool,
    ) -> Result<Data> {
        if runtime.views.contains(&index) {
            return Err(error("view is circularly defined"));
        }
        runtime.views.push(index);
        // Stored definitions have their own names and expression IDs. Neither
        // caller CTEs nor correlated rows/cached expressions enter this scope.
        let active = core::mem::take(&mut runtime.active);
        let recursive = core::mem::take(&mut runtime.recursive);
        let cache = core::mem::take(&mut runtime.cache);
        let scalars = core::mem::take(&mut runtime.scalar_cache);
        let outer = core::mem::take(&mut runtime.outer);
        let reads = core::mem::take(&mut runtime.outer_reads);
        let rename = runtime.rename.take();
        let view = &self.state.views[index];
        let result = self.query(&view.query, context, None, runtime, schema_only);
        runtime.active = active;
        runtime.recursive = recursive;
        runtime.cache = cache;
        runtime.scalar_cache = scalars;
        runtime.outer = outer;
        runtime.outer_reads = reads;
        runtime.rename = rename;
        runtime.views.pop();
        result
    }
    pub(super) fn query_source<'a>(
        &'a self,
        source: &Source,
        context: &mut Eval<'_>,
        scope: Option<Rc<Scope>>,
        runtime: &mut Runtime,
        schema_only: bool,
        row_cap: Option<usize>,
    ) -> Result<SourceData<'a>> {
        if let Some(query) = &source.query {
            let data = self.query(query, context, scope, runtime, schema_only)?;
            return Ok(SourceData::Query(runtime.own(data, context.limits)?));
        }
        if !source.qualified {
            if let Some((table, scope)) = Scope::find(&scope, &source.name) {
                if runtime
                    .recursive
                    .iter()
                    .rev()
                    .find(|(id, _)| *id == table.id)
                    .is_some_and(|(_, depth)| runtime.depth != *depth)
                {
                    return Err(error("recursive reference in a subquery"));
                }
                let cache_key = (
                    table.id,
                    scope.id,
                    if table.materialized != Some(false) {
                        0
                    } else {
                        runtime.returning_site
                    },
                );
                if let Some(data) = runtime.cache.get(&cache_key) {
                    return Ok(SourceData::Query(data.0.clone()));
                }
                if runtime.active.contains(&table.id) {
                    return Err(Error::Unsupported("recursive common table expressions"));
                }
                runtime.active.push(table.id);
                let recursive = recursive::has_direct_reference(&table);
                let reads = core::mem::take(&mut runtime.outer_reads);
                let data = if recursive {
                    self.recursive_query(
                        &table,
                        context,
                        Some(scope),
                        runtime,
                        schema_only,
                        row_cap,
                    )
                } else {
                    self.query(&table.query, context, Some(scope), runtime, schema_only)
                };
                let used = core::mem::replace(&mut runtime.outer_reads, reads);
                let correlated = !used.is_empty();
                runtime.outer_reads.extend(used);
                runtime.active.pop();
                let mut data = data?;
                if let Some(columns) = &table.columns {
                    if columns.len() != data.result.columns.len() {
                        return Err(error("common table column count does not match query"));
                    }
                    data.result.columns = columns.clone();
                }
                let data = runtime.own(data, context.limits)?;
                if !(schema_only || correlated || recursive && row_cap.is_some()) {
                    runtime
                        .cache
                        .insert(cache_key, (data.clone(), table.materialized == Some(true)));
                }
                return Ok(SourceData::Query(data));
            }
        }
        if let Some(index) = self.view_index(&source.name) {
            let data = self.view_query(index, context, runtime, schema_only)?;
            return Ok(SourceData::Query(runtime.own(data, context.limits)?));
        }
        runtime.tables_read.insert(source.name.to_ascii_lowercase());
        let table = &self.state.tables[self.index(&source.name)?];
        let ids = if table.without_rowid && !schema_only {
            Some(table.scan_ids(context)?)
        } else {
            None
        };
        Ok(SourceData::Table(table, ids))
    }

    pub(super) fn query(
        &self,
        query: &Query,
        context: &mut Eval<'_>,
        scope: Option<Rc<Scope>>,
        runtime: &mut Runtime,
        schema_only: bool,
    ) -> Result<Data> {
        if runtime.depth >= context.limits.max_expr_depth.min(32) {
            return Err(Error::Limit("query nesting"));
        }
        runtime.depth += 1;
        let result = self.query_inner(query, context, scope, runtime, schema_only);
        runtime.depth -= 1;
        result
    }
    fn query_inner(
        &self,
        query: &Query,
        context: &mut Eval<'_>,
        scope: Option<Rc<Scope>>,
        runtime: &mut Runtime,
        schema_only: bool,
    ) -> Result<Data> {
        context.fuel.spend()?;
        let scope = Scope::extend(scope, &query.with, runtime)?;
        if let Some(rename) = &mut runtime.rename {
            for table in &query.with {
                context.fuel.spend()?;
                if rename.seen.insert(table.id) {
                    rename.pending.push((table.query.clone(), scope.clone()));
                }
            }
        }
        if query.cores.len() == 1 {
            if let QueryCore::Select(select) = &query.cores[0] {
                let mut select = (**select).clone();
                select.order = query.order.clone();
                select.limit = query.limit.clone();
                select.offset = query.offset.clone();
                return self.simple_select(&select, context, scope, runtime, schema_only);
            }
            if let QueryCore::Values(values) = &query.cores[0] {
                let limit = self.expressions(scope.clone(), runtime).limit(
                    query.limit.as_ref(),
                    false,
                    schema_only,
                    context,
                )?;
                let offset = self
                    .expressions(scope.clone(), runtime)
                    .limit(query.offset.as_ref(), true, schema_only, context)?
                    .unwrap_or(0);
                return self.query_values(
                    values,
                    context,
                    scope,
                    runtime,
                    schema_only || limit == Some(0),
                    limit,
                    offset,
                );
            }
        }
        let limit = self.expressions(scope.clone(), runtime).limit(
            query.limit.as_ref(),
            false,
            schema_only,
            context,
        )?;
        let offset = self
            .expressions(scope.clone(), runtime)
            .limit(query.offset.as_ref(), true, schema_only, context)?
            .unwrap_or(0);
        let mut parts = Vec::new();
        let early_limit = if query.order.is_empty()
            && query.operators.iter().all(|op| *op == Compound::UnionAll)
        {
            limit.map(|n| n.saturating_add(offset))
        } else {
            None
        };
        let mut produced = 0usize;
        for core in &query.cores {
            let empty = schema_only || limit == Some(0);
            let part = if let Some(cap) = early_limit {
                let remaining = cap.saturating_sub(produced);
                match core {
                    QueryCore::Select(select) => {
                        let mut select = (**select).clone();
                        select.limit = Some(Expr::literal(Value::Integer(remaining as i64)));
                        self.simple_select(
                            &select,
                            context,
                            scope.clone(),
                            runtime,
                            empty || remaining == 0,
                        )?
                    }
                    QueryCore::Values(values) => self.query_values(
                        values,
                        context,
                        scope.clone(),
                        runtime,
                        empty || remaining == 0,
                        Some(remaining),
                        0,
                    )?,
                }
            } else {
                self.query_core(core, context, scope.clone(), runtime, empty)?
            };
            produced = produced.saturating_add(part.result.rows.len());
            if parts
                .first()
                .is_some_and(|first: &Data| first.result.columns.len() != part.result.columns.len())
            {
                return Err(error("compound SELECTs have different column counts"));
            }
            parts.push(part);
        }
        let order = compound_order(&query.order, &parts, &query.cores, |expr, fields| {
            if runtime.rename.is_some() {
                self.expressions(scope.clone(), runtime)
                    .bind(expr, fields, &[], true, context)?;
            }
            Ok(())
        })?;
        let mut types = CompoundTypes::default();
        for part in &parts {
            types.add_data(part, context)?;
        }
        let mut output = core::mem::take(&mut parts[0].result.rows);
        let mut collations = parts[0].comparison_collations();
        for (part, op) in parts.iter_mut().skip(1).zip(&query.operators) {
            for (left, right) in collations.iter_mut().zip(part.comparison_collations()) {
                if left.is_none() {
                    *left = right;
                }
            }
            output = combine(
                output,
                core::mem::take(&mut part.result.rows),
                *op,
                &collations,
                context,
                !order.is_empty(),
            )?;
        }
        let columns = core::mem::take(&mut parts[0].result.columns);
        // Scalar subqueries take expression affinity/type from the right-most
        // SELECT. Derived tables instead use the merged column metadata above.
        let last = parts.pop().ok_or_else(|| error("empty compound query"))?;
        if !order.is_empty() {
            let mut candidates = Vec::new();
            for values in output {
                let keys = evaluate_list(order.iter().map(|o| &o.expr), &values, None, context)?;
                candidates.push(Candidate { values, keys });
            }
            output = sort(candidates, &order, context.fuel, context.encoding)?
                .into_iter()
                .map(|r| r.values)
                .collect();
        }
        Ok(Data {
            nested: None,
            fields: last.fields,
            projection: last.projection,
            column_types: Some(types),
            result: QueryResult {
                columns,
                changes: 0,
                rows: output
                    .into_iter()
                    .skip(offset)
                    .take(limit.unwrap_or(usize::MAX))
                    .collect(),
            },
        })
    }
    fn query_core(
        &self,
        core: &QueryCore,
        context: &mut Eval<'_>,
        scope: Option<Rc<Scope>>,
        runtime: &mut Runtime,
        schema_only: bool,
    ) -> Result<Data> {
        match core {
            QueryCore::Select(select) => {
                self.simple_select(select, context, scope, runtime, schema_only)
            }
            QueryCore::Values(values) => {
                self.query_values(values, context, scope, runtime, schema_only, None, 0)
            }
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn query_values(
        &self,
        values: &[Vec<Expr>],
        context: &mut Eval<'_>,
        scope: Option<Rc<Scope>>,
        runtime: &mut Runtime,
        schema_only: bool,
        limit: Option<usize>,
        offset: usize,
    ) -> Result<Data> {
        let mut rows = Vec::new();
        let mut projection = Vec::new();
        let mut types = CompoundTypes::default();
        let mut bytes = 0;
        for (i, row) in values.iter().enumerate() {
            let bound = row
                .iter()
                .map(|e| {
                    self.expressions(scope.clone(), runtime)
                        .bind(e, &[], &[], false, context)
                })
                .collect::<Result<Vec<_>>>()?;
            types.add(&bound, &[], context)?;
            if !schema_only && i >= offset && limit.is_none_or(|n| rows.len() < n) {
                let values = if runtime.existence_here() {
                    vec![Value::Null; bound.len()]
                } else {
                    self.expressions(scope.clone(), runtime).values(
                        bound.iter(),
                        &[],
                        None,
                        context,
                    )?
                };
                push_row(&mut rows, values, &mut bytes, context.limits)?;
            }
            projection = bound;
        }
        Ok(Data {
            nested: None,
            fields: Vec::new(),
            result: QueryResult {
                columns: (1..=projection.len())
                    .map(|i| format!("column{i}"))
                    .collect(),
                rows,
                changes: 0,
            },
            projection,
            column_types: Some(types),
        })
    }
}

fn compound_order(
    order: &[Ordering],
    parts: &[Data],
    cores: &[QueryCore],
    mut matched: impl FnMut(&Expr, &[Field]) -> Result<()>,
) -> Result<Vec<Ordering>> {
    order
        .iter()
        .map(|term| {
            let mut expression = &term.expr;
            while let ExprKind::Collate(inner, _) = &expression.kind {
                expression = inner;
            }
            let mut found = None;
            if let Some(n) = positional_integer(expression) {
                if n < 1 || n as usize > parts[0].result.columns.len() {
                    return Err(error("ORDER BY term out of range"));
                }
                found = Some((n as usize - 1, &parts[0].projection[n as usize - 1]));
            } else {
                for (part, core) in parts.iter().zip(cores) {
                    if let ExprKind::Column {
                        qualifier: None,
                        name,
                        ..
                    } = &expression.kind
                    {
                        if let Some(i) = part
                            .result
                            .columns
                            .iter()
                            .position(|n| n.eq_ignore_ascii_case(name))
                        {
                            let explicit_alias = matches!(core, QueryCore::Select(select)
                                if select.items.iter().any(|item| item.alias.as_ref()
                                    .is_some_and(|alias| alias.eq_ignore_ascii_case(name))
                                    || item.expr.is_none() && part.fields.iter().any(|f|
                                        f.wildcard(item.star.as_deref()) && f.name.eq_ignore_ascii_case(name))));
                            if explicit_alias {
                                found = Some((i, &part.projection[i]));
                                break;
                            }
                        }
                    }
                    if let Ok(bound) = eval::bind(expression, &part.fields, &[], true) {
                        if let Some(i) = part.projection.iter().position(|e| *e == bound) {
                            matched(expression, &part.fields)?;
                            found = Some((i, &part.projection[i]));
                            break;
                        }
                    }
                }
            }
            let (slot, selected) = found
                .ok_or_else(|| error("ORDER BY term does not match a compound result column"))?;
            let mut expr = Expr {
                location: Default::default(),
                token: None,
                depth: 1,
                kind: ExprKind::Slot(
                    slot,
                    eval::expr_affinity(selected),
                    eval::collation(selected),
                ),
            };
            if let ExprKind::Collate(_, collation) = &term.expr.kind {
                expr = Expr {
                    location: Default::default(),
                    token: None,
                    depth: 2,
                    kind: ExprKind::Collate(Box::new(expr), *collation),
                };
            }
            Ok(Ordering {
                expr,
                descending: term.descending,
                nulls_first: term.nulls_first,
            })
        })
        .collect()
}
fn compare_rows(
    a: &[Value],
    b: &[Value],
    collations: &[Option<Collation>],
    encoding: Encoding,
) -> Result<Compare> {
    for ((a, b), collation) in a.iter().zip(b).zip(collations) {
        let cmp = scalar::compare_encoded(a, b, collation.unwrap_or(Collation::Binary), encoding)?;
        if cmp != Compare::Equal {
            return Ok(cmp);
        }
    }
    Ok(Compare::Equal)
}
fn combine(
    mut left: Vec<Vec<Value>>,
    mut right: Vec<Vec<Value>>,
    operator: Compound,
    collations: &[Option<Collation>],
    context: &mut Eval<'_>,
    ordered: bool,
) -> Result<Vec<Vec<Value>>> {
    if operator == Compound::UnionAll {
        let mut bytes = left.iter().try_fold(0usize, |n, row| {
            n.checked_add(values_size(row)?)
                .ok_or(Error::Limit("compound result bytes"))
        })?;
        for row in right {
            push_row(&mut left, row, &mut bytes, context.limits)?;
        }
        return Ok(left);
    }
    if ordered {
        left = deduplicate(left, collations, context, false)?;
        right = deduplicate(right, collations, context, false)?;
    }
    if operator == Compound::Union {
        let mut bytes = left.iter().try_fold(0usize, |n, row| {
            n.checked_add(values_size(row)?)
                .ok_or(Error::Limit("compound result bytes"))
        })?;
        for row in right.drain(..) {
            push_row(&mut left, row, &mut bytes, context.limits)?;
        }
    }
    let unique = deduplicate(left, collations, context, true)?;
    if operator == Compound::Union {
        return Ok(unique);
    }
    let mut output = Vec::new();
    for row in unique {
        let mut found = false;
        for other in &right {
            context.fuel.spend()?;
            if compare_rows(&row, other, collations, context.encoding)? == Compare::Equal {
                found = true;
                break;
            }
        }
        if found == (operator == Compound::Intersect) {
            output.push(row);
        }
    }
    Ok(output)
}
fn deduplicate(
    rows: Vec<Vec<Value>>,
    collations: &[Option<Collation>],
    context: &mut Eval<'_>,
    keep_last: bool,
) -> Result<Vec<Vec<Value>>> {
    let sorted = sort_by(rows, context.fuel, |a, b| {
        compare_rows(a, b, collations, context.encoding)
    })?;
    let mut unique: Vec<Vec<Value>> = Vec::new();
    let mut bytes = 0usize;
    for row in sorted {
        context.fuel.spend()?;
        if unique
            .last()
            .map(|last| compare_rows(last, &row, collations, context.encoding))
            .transpose()?
            == Some(Compare::Equal)
        {
            if !keep_last {
                continue;
            }
            let previous = unique.pop().ok_or_else(|| error("missing duplicate row"))?;
            bytes = bytes.saturating_sub(values_size(&previous)?);
        }
        push_row(&mut unique, row, &mut bytes, context.limits)?;
    }
    Ok(unique)
}
