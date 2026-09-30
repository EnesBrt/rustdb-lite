use super::{
    error,
    parser::{Binary, Expr, ExprKind, QueryMode, Subquery, Unary},
    scalar::{self, Affinity, Collation},
    SqlLimits,
};
use crate::{Encoding, Error, Result, Text, Value};
use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
    vec::Vec,
};
use core::cmp::Ordering;

mod aggregates;
pub mod generated;
mod patterns;
pub mod rows;
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub nested: Option<NestedField>,
    pub generated: Option<generated::Reference>,
    /// Physical columns supplying an unqualified USING/NATURAL value.
    /// One slot selects a RIGHT value; multiple slots form a FULL coalesce.
    pub merged: Vec<usize>,
    pub table: String,
    pub name: String,
    pub affinity: Affinity,
    pub collation: Collation,
    pub hidden: bool,
    /// Pseudo-table fields such as UPSERT's excluded require a qualifier.
    pub qualified_only: bool,
    pub unqualified_hidden: bool,
    pub declared_type: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct NestedField {
    pub table: String,
    pub column: String,
    pub rowid: bool,
    pub no_expand: bool,
    pub using: bool,
    pub group: usize,
}
impl Field {
    pub fn extra_bytes(&self) -> usize {
        self.merged.len() * core::mem::size_of::<usize>()
            + self
                .nested
                .as_ref()
                .map_or(0, |n| n.table.len() + n.column.len())
    }
    pub fn wildcard(&self, qualifier: Option<&str>) -> bool {
        if self.hidden {
            return false;
        }
        if let Some(n) = &self.nested {
            !n.rowid
                && qualifier.map_or(!n.no_expand && !self.qualified_only, |q| {
                    q.eq_ignore_ascii_case(&n.table)
                })
        } else {
            (!self.qualified_only || qualifier.is_some())
                && qualifier.is_none_or(|q| q.eq_ignore_ascii_case(&self.table))
        }
    }
}
/// Parenthesized FROM sources retain original names and expose their generated
/// column names through a group alias. An explicit USING output ends the name
/// search within that group, as in the native nested-FROM resolver.
pub fn resolve_field(
    fields: &[Field],
    qualifier: Option<&str>,
    name: &str,
) -> Result<Option<usize>> {
    let rowid = ["rowid", "_rowid_", "oid"]
        .iter()
        .any(|n| n.eq_ignore_ascii_case(name));
    let mut real = Vec::new();
    let mut hidden = Vec::new();
    let mut at = 0;
    while at < fields.len() {
        let f = &fields[at];
        if let Some(origin) = &f.nested {
            let end = at
                + fields[at..]
                    .iter()
                    .take_while(|f| f.nested.as_ref().is_some_and(|n| n.group == origin.group))
                    .count();
            let mut found = Vec::new();
            for (i, f) in fields.iter().enumerate().take(end).skip(at) {
                let n = f
                    .nested
                    .as_ref()
                    .ok_or(Error::Corrupt("nested field group"))?;
                if (f.qualified_only || f.unqualified_hidden) && qualifier.is_none() {
                    continue;
                }
                if qualifier.is_some_and(|q| !q.eq_ignore_ascii_case(&n.table)) {
                    continue;
                }
                if n.rowid {
                    if rowid {
                        hidden.push(i);
                    }
                } else if n.column.eq_ignore_ascii_case(name) {
                    found.push(i);
                    if n.using {
                        break;
                    }
                }
            }
            if found.is_empty()
                && qualifier
                    .is_some_and(|q| !f.table.is_empty() && q.eq_ignore_ascii_case(&f.table))
            {
                found.extend((at..end).filter(|i| fields[*i].name.eq_ignore_ascii_case(name)));
            }
            real.extend(found);
            at = end;
        } else {
            if (!(f.qualified_only || f.unqualified_hidden) || qualifier.is_some())
                && qualifier.is_none_or(|q| q.eq_ignore_ascii_case(&f.table))
            {
                if f.hidden {
                    if rowid {
                        hidden.push(at);
                    }
                } else if f.name.eq_ignore_ascii_case(name) {
                    real.push(at);
                }
            }
            at += 1;
        }
    }
    let found = if real.is_empty() { hidden } else { real };
    if found.len() > 1 {
        return Err(error(format!("ambiguous column: {name}")));
    }
    Ok(found.first().copied())
}
pub fn field(fields: &[Field], slot: usize, outer: Option<usize>, qualified: bool) -> Result<Expr> {
    let f = fields.get(slot).ok_or(Error::Corrupt("query field slot"))?;
    if qualified || f.merged.is_empty() {
        return generated::field(f, slot, outer);
    }
    let args = f
        .merged
        .iter()
        .map(|i| {
            generated::field(
                fields.get(*i).ok_or(Error::Corrupt("merged field slot"))?,
                *i,
                outer,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    coalesced(args)
}
pub fn coalesced(mut args: Vec<Expr>) -> Result<Expr> {
    if args.len() == 1 {
        return Ok(args.remove(0));
    }
    if args.is_empty() {
        return Err(Error::Corrupt("empty merged field"));
    }
    let depth = 1 + args.iter().map(|e| e.depth).max().unwrap_or(0);
    if depth > 64 {
        return Err(Error::Limit("merged expression depth"));
    }
    Ok(Expr {
        depth,
        token: None,
        kind: ExprKind::Merged(args),
    })
}
#[derive(Clone, Debug, PartialEq)]
pub struct BoundSubquery {
    pub source: Subquery,
    pub fields: alloc::rc::Rc<[Field]>,
    pub affinity: Affinity,
    pub collation: Option<Collation>,
    pub correlated: bool,
    /// Columns read from the immediate containing SELECT.
    pub dependencies: Vec<usize>,
    pub tables: Vec<String>,
    pub declared_type: String,
    pub columns: Vec<QueryColumn>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct QueryColumn {
    pub affinity: Affinity,
    pub collation: Option<Collation>,
    pub explicit_collation: Option<Collation>,
    pub declared_type: String,
}
pub trait Resolver {
    fn double_quoted_strings(&self) -> bool {
        false
    }
    fn column(&mut self, qualifier: Option<&str>, name: &str) -> Result<Option<Expr>>;
    fn query(&mut self, source: &Subquery, fields: &[Field]) -> Result<BoundSubquery>;
}
pub trait Subqueries {
    fn row_field(
        &mut self,
        query: &BoundSubquery,
        column: usize,
        row: &[Value],
        context: &mut Eval<'_>,
    ) -> Result<Value> {
        Ok(self
            .run(query, row, context)?
            .first()
            .and_then(|r| r.get(column))
            .cloned()
            .unwrap_or(Value::Null))
    }
    fn slot(&mut self, slot: usize, row: &[Value], _: &mut Eval<'_>) -> Result<Value> {
        Ok(row.get(slot).cloned().unwrap_or(Value::Null))
    }
    fn outer(&self, frame: usize, slot: usize) -> Result<Value>;
    fn run(
        &mut self,
        query: &BoundSubquery,
        row: &[Value],
        context: &mut Eval<'_>,
    ) -> Result<alloc::rc::Rc<Vec<Vec<Value>>>>;
}
struct NoQueries;
struct GeneratedBinding;
impl Resolver for GeneratedBinding {
    fn double_quoted_strings(&self) -> bool {
        true
    }
    fn column(&mut self, _: Option<&str>, _: &str) -> Result<Option<Expr>> {
        Ok(None)
    }
    fn query(&mut self, _: &Subquery, _: &[Field]) -> Result<BoundSubquery> {
        Err(error("subquery in generated column"))
    }
}
pub fn bind_generated(expr: &Expr, fields: &[Field]) -> Result<Expr> {
    bind_with(expr, fields, &[], false, &mut GeneratedBinding)
}
impl Resolver for NoQueries {
    fn column(&mut self, _: Option<&str>, _: &str) -> Result<Option<Expr>> {
        Ok(None)
    }
    fn query(&mut self, _: &Subquery, _: &[Field]) -> Result<BoundSubquery> {
        Err(error("subqueries prohibited in this expression"))
    }
}
impl Subqueries for NoQueries {
    fn outer(&self, _: usize, _: usize) -> Result<Value> {
        Err(error("outer column outside query"))
    }
    fn run(
        &mut self,
        _: &BoundSubquery,
        _: &[Value],
        _: &mut Eval<'_>,
    ) -> Result<alloc::rc::Rc<Vec<Vec<Value>>>> {
        Err(error("subquery outside query execution"))
    }
}
pub struct Fuel {
    remaining: usize,
}
impl Fuel {
    pub fn new(n: usize) -> Self {
        Self { remaining: n }
    }
    pub fn spend(&mut self) -> Result<()> {
        if self.remaining == 0 {
            return Err(Error::Limit("SQL execution steps"));
        }
        self.remaining -= 1;
        Ok(())
    }
}
pub fn aggregate(name: &str, n: usize) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "count" | "sum" | "total" | "avg" | "group_concat" | "string_agg"
    ) || (n == 1 && (name.eq_ignore_ascii_case("min") || name.eq_ignore_ascii_case("max")))
}
pub fn has_aggregate(expr: &Expr) -> bool {
    matches!(&expr.kind,ExprKind::Call{name,args,..} if aggregate(name,args.len()))
        || expr.children().iter().any(|x| has_aggregate(x))
}
fn validate_call(name: &str, n: usize, star: bool, distinct: bool) -> Result<()> {
    let name = name.to_ascii_lowercase();
    let valid = match name.as_str() {
        "count" => n <= 1,
        "sum" | "total" | "avg" => n == 1,
        "min" | "max" => n >= 1,
        "group_concat" => n == 1 || n == 2,
        "string_agg" => n == 2,
        "typeof" | "length" | "octet_length" | "hex" | "lower" | "upper" | "abs" | "unicode" => {
            n == 1
        }
        "ifnull" | "nullif" | "instr" | "glob" => n == 2,
        "replace" => n == 3,
        "substr" | "substring" => n == 2 || n == 3,
        "trim" | "ltrim" | "rtrim" | "unhex" => n == 1 || n == 2,
        "coalesce" => n >= 2,
        "iif" | "if" => n >= 2,
        "char" => true,
        "changes" | "total_changes" | "last_insert_rowid" => n == 0,
        "like" => n == 2 || n == 3,
        _ => return Err(error(format!("no such function: {name}"))),
    };
    if !valid || (star && name != "count") || (distinct && (!aggregate(&name, n) || n != 1)) {
        return Err(error(format!("invalid arguments for {name}")));
    }
    Ok(())
}
pub fn bind(
    expr: &Expr,
    fields: &[Field],
    aliases: &[(String, Expr)],
    allow_aggregate: bool,
) -> Result<Expr> {
    bind_with(expr, fields, aliases, allow_aggregate, &mut NoQueries)
}
pub fn bind_with(
    expr: &Expr,
    fields: &[Field],
    aliases: &[(String, Expr)],
    allow_aggregate: bool,
    resolver: &mut dyn Resolver,
) -> Result<Expr> {
    fn inner(
        expr: &Expr,
        fields: &[Field],
        aliases: &[(String, Expr)],
        allow: bool,
        inside: bool,
        resolver: &mut dyn Resolver,
    ) -> Result<Expr> {
        let mut rec = |x: &Expr| inner(x, fields, aliases, allow, inside, resolver);
        let kind = match &expr.kind {
            ExprKind::Column {
                qualifier,
                name,
                quoted,
            } => {
                if let Some(i) = resolve_field(fields, qualifier.as_deref(), name)? {
                    return field(fields, i, None, qualifier.is_some());
                } else if qualifier.is_none() {
                    if let Some((_, value)) =
                        aliases.iter().find(|(n, _)| n.eq_ignore_ascii_case(name))
                    {
                        return inner(value, fields, &[], allow, inside, resolver);
                    }
                    if let Some(outer) = resolver.column(None, name)? {
                        return Ok(outer);
                    }
                    if !quoted
                        && (name.eq_ignore_ascii_case("true") || name.eq_ignore_ascii_case("false"))
                    {
                        ExprKind::Boolean(name.eq_ignore_ascii_case("true"))
                    } else if *quoted && resolver.double_quoted_strings() {
                        ExprKind::Literal(Value::Text(Text::utf8(name)))
                    } else {
                        return Err(error(format!("no such column: {name}")));
                    }
                } else if let Some(outer) = resolver.column(qualifier.as_deref(), name)? {
                    return Ok(outer);
                } else {
                    return Err(error(format!("no such column: {name}")));
                }
            }
            ExprKind::Subquery(query) => {
                ExprKind::BoundSubquery(Box::new(resolver.query(query, fields)?))
            }
            ExprKind::Vector(values) => {
                ExprKind::Vector(values.iter().map(&mut rec).collect::<Result<_>>()?)
            }
            ExprKind::RowField(query, column, width) => {
                ExprKind::RowField(Box::new(rec(query)?), *column, *width)
            }
            ExprKind::InQuery(x, query, not) => {
                ExprKind::InQuery(Box::new(rec(x)?), Box::new(rec(query)?), *not)
            }
            ExprKind::Unary(op, x) => ExprKind::Unary(*op, Box::new(rec(x)?)),
            ExprKind::Binary(op, a, b) => {
                ExprKind::Binary(*op, Box::new(rec(a)?), Box::new(rec(b)?))
            }
            ExprKind::Between(x, a, b, n) => {
                ExprKind::Between(Box::new(rec(x)?), Box::new(rec(a)?), Box::new(rec(b)?), *n)
            }
            ExprKind::In(x, values, n) => ExprKind::In(
                Box::new(rec(x)?),
                values.iter().map(rec).collect::<Result<_>>()?,
                *n,
            ),
            ExprKind::Case(base, arms, other) => ExprKind::Case(
                base.as_ref().map(|x| rec(x).map(Box::new)).transpose()?,
                arms.iter()
                    .map(|(a, b)| Ok((rec(a)?, rec(b)?)))
                    .collect::<Result<_>>()?,
                other.as_ref().map(|x| rec(x).map(Box::new)).transpose()?,
            ),
            ExprKind::Cast(x, a) => ExprKind::Cast(Box::new(rec(x)?), *a),
            ExprKind::Collate(x, c) => ExprKind::Collate(Box::new(rec(x)?), *c),
            ExprKind::Merged(args) => {
                ExprKind::Merged(args.iter().map(&mut rec).collect::<Result<_>>()?)
            }
            ExprKind::Call {
                name,
                args,
                star,
                distinct,
                order,
                filter,
            } => {
                validate_call(name, args.len(), *star, *distinct)?;
                let agg = aggregate(name, args.len());
                if !agg && (!order.is_empty() || filter.is_some()) {
                    return Err(error("FILTER and ORDER BY require an aggregate"));
                }
                if agg && (!allow || inside) {
                    return Err(error("misuse of aggregate function"));
                }
                ExprKind::Call {
                    name: name.to_ascii_lowercase(),
                    args: args
                        .iter()
                        .map(|x| inner(x, fields, aliases, allow, inside || agg, resolver))
                        .collect::<Result<_>>()?,
                    star: *star,
                    distinct: *distinct,
                    order: order
                        .iter()
                        .map(|o| {
                            Ok(super::parser::Ordering {
                                expr: inner(
                                    &o.expr,
                                    fields,
                                    aliases,
                                    allow,
                                    inside || agg,
                                    resolver,
                                )?,
                                descending: o.descending,
                                nulls_first: o.nulls_first,
                            })
                        })
                        .collect::<Result<_>>()?,
                    filter: filter
                        .as_ref()
                        .map(|f| {
                            inner(f, fields, aliases, allow, inside || agg, resolver).map(Box::new)
                        })
                        .transpose()?,
                }
            }
            _ => expr.kind.clone(),
        };
        let mut result = Expr {
            kind,
            depth: 1,
            token: expr.token.clone(),
        };
        rows::validate(&result)?;
        if let ExprKind::Call { name, args, .. } = &result.kind {
            fn reference(e: &Expr, outer: bool) -> bool {
                (if outer {
                    matches!(e.kind, ExprKind::Outer(..))
                        || matches!(&e.kind, ExprKind::Generated(r) if r.outer.is_some())
                } else {
                    matches!(e.kind, ExprKind::Slot(..))
                        || matches!(&e.kind, ExprKind::Generated(r) if r.outer.is_none())
                }) || e.children().iter().any(|e| reference(e, outer))
            }
            if aggregate(name, args.len())
                && result.children().iter().any(|e| reference(e, true))
                && !result.children().iter().any(|e| reference(e, false))
            {
                return Err(Error::Unsupported("aggregate owned by an outer query"));
            }
        }
        result.depth = if let ExprKind::Generated(r) = &result.kind {
            r.schema.read_depth[r.column].ok_or_else(|| error("generated column loop"))?
        } else {
            1 + result.children().iter().map(|e| e.depth).max().unwrap_or(0)
        };
        if result.depth > 64 {
            return Err(Error::Limit("resolved expression depth"));
        }
        Ok(result)
    }
    let result = inner(expr, fields, aliases, allow_aggregate, false, resolver)?;
    rows::scalar(&result)?;
    Ok(result)
}
pub fn expr_affinity(expr: &Expr) -> Affinity {
    match &expr.kind {
        ExprKind::Generated(r) => r.metadata().affinity,
        ExprKind::Slot(_, a, _) | ExprKind::Outer(_, _, a, _, _) | ExprKind::Cast(_, a) => *a,
        ExprKind::BoundSubquery(q) if q.source.mode == QueryMode::Scalar => q.affinity,
        ExprKind::Collate(x, _) => expr_affinity(x),
        ExprKind::Merged(args) => args.first().map_or(Affinity::None, expr_affinity),
        _ => Affinity::None,
    }
}
pub fn affinity_type(affinity: Affinity) -> &'static str {
    match affinity {
        Affinity::Text => "TEXT",
        Affinity::Integer => "INT",
        Affinity::Numeric | Affinity::FlexNumeric => "NUM",
        Affinity::Real => "REAL",
        _ => "",
    }
}
pub fn declared_type(expr: &Expr, fields: &[Field]) -> String {
    let raw = match &expr.kind {
        ExprKind::Generated(r) => r.metadata().declared_type.clone(),
        ExprKind::Slot(i, _, _) => fields
            .get(*i)
            .map_or_else(String::new, |f| f.declared_type.clone()),
        ExprKind::Outer(_, _, _, _, typ) => typ.clone(),
        ExprKind::BoundSubquery(q) if q.source.mode == QueryMode::Scalar => q.declared_type.clone(),
        _ => String::new(),
    };
    type_for_affinity(raw, expr_affinity(expr))
}
pub fn type_for_affinity(raw: String, affinity: Affinity) -> String {
    if !raw.is_empty() && Affinity::from_type(&raw) == affinity {
        raw
    } else if affinity == Affinity::Blob {
        "BLOB".into()
    } else {
        affinity_type(affinity).into()
    }
}
pub fn explicit_collation(expr: &Expr) -> Option<Collation> {
    if let ExprKind::Collate(_, c) = expr.kind {
        return Some(c);
    }
    // Aggregate FILTER and ORDER BY have their own comparisons. Their explicit
    // collations do not become the collation of the aggregate's result.
    if let ExprKind::Call { args, .. } = &expr.kind {
        return args.iter().find_map(explicit_collation);
    }
    expr.children().into_iter().find_map(explicit_collation)
}
pub fn collation(expr: &Expr) -> Collation {
    collation_hint(expr).unwrap_or(Collation::Binary)
}
pub fn collation_hint(expr: &Expr) -> Option<Collation> {
    explicit_collation(expr).or_else(|| match &expr.kind {
        ExprKind::Generated(r) => Some(r.metadata().collation),
        ExprKind::Slot(_, _, c) | ExprKind::Outer(_, _, _, c, _) => Some(*c),
        ExprKind::Unary(Unary::Plus, x) | ExprKind::Cast(x, _) => collation_hint(x),
        ExprKind::Merged(args) => args.first().and_then(collation_hint),
        _ => None,
    })
}
fn comparison_collation(a: &Expr, b: &Expr) -> Collation {
    explicit_collation(a)
        .or_else(|| explicit_collation(b))
        .unwrap_or_else(|| {
            fn implicit(e: &Expr) -> Option<Collation> {
                match &e.kind {
                    ExprKind::Generated(r) => Some(r.metadata().collation),
                    ExprKind::Slot(_, _, c) | ExprKind::Outer(_, _, _, c, _) => Some(*c),
                    ExprKind::Unary(Unary::Plus, x) | ExprKind::Cast(x, _) => implicit(x),
                    ExprKind::Merged(args) => args.first().and_then(implicit),
                    _ => None,
                }
            }
            implicit(a)
                .or_else(|| implicit(b))
                .unwrap_or(Collation::Binary)
        })
}
pub struct Eval<'a> {
    pub params: &'a [Value],
    pub encoding: Encoding,
    pub limits: SqlLimits,
    pub fuel: &'a mut Fuel,
    pub changes: usize,
    pub total_changes: u64,
    pub last_rowid: i64,
}
impl Eval<'_> {
    pub fn eval(
        &mut self,
        expr: &Expr,
        row: &[Value],
        group: Option<&[Vec<Value>]>,
    ) -> Result<Value> {
        self.eval_with(expr, row, group, &mut NoQueries)
    }
    pub fn eval_with(
        &mut self,
        expr: &Expr,
        row: &[Value],
        group: Option<&[Vec<Value>]>,
        queries: &mut dyn Subqueries,
    ) -> Result<Value> {
        if expr.depth > self.limits.max_expr_depth.min(64) {
            return Err(Error::Limit("SQL expression depth"));
        }
        self.fuel.spend()?;
        let value = match &expr.kind {
            ExprKind::Literal(v) => v.clone(),
            ExprKind::MinMagnitude => Value::Real(9223372036854775808.0),
            ExprKind::Merged(args) => {
                let mut value = Value::Null;
                for arg in args {
                    value = self.eval_with(arg, row, group, queries)?;
                    if !scalar::null(&value) {
                        break;
                    }
                }
                value
            }
            ExprKind::Boolean(b) => Value::Integer(i64::from(*b)),
            ExprKind::Column { .. } | ExprKind::Subquery(_) => {
                return Err(error("unbound expression"))
            }
            ExprKind::Vector(_) => return Err(error("row value misused")),
            ExprKind::RowField(query, column, _) => {
                let ExprKind::BoundSubquery(q) = &query.kind else {
                    return Err(Error::Corrupt("row assignment query"));
                };
                queries.row_field(q, *column, row, self)?
            }
            ExprKind::Outer(frame, slot, _, _, _) => queries.outer(*frame, *slot)?,
            ExprKind::Generated(reference) => reference.read(row, queries, self)?,
            ExprKind::BoundSubquery(q) => {
                let rows = queries.run(q, row, self)?;
                match q.source.mode {
                    QueryMode::Scalar => rows
                        .first()
                        .and_then(|r| r.first())
                        .cloned()
                        .unwrap_or(Value::Null),
                    QueryMode::Exists => Value::Integer(i64::from(!rows.is_empty())),
                    QueryMode::Set => return Err(error("set query used as scalar expression")),
                }
            }
            ExprKind::InQuery(x, query, not) => {
                let ExprKind::BoundSubquery(q) = &query.kind else {
                    return Err(error("unbound IN query"));
                };
                self.in_query(x, q, *not, row, group, queries)?
            }
            ExprKind::Slot(i, _, _) => queries.slot(*i, row, self)?,
            ExprKind::Parameter(i) => self.params.get(*i).cloned().unwrap_or(Value::Null),
            ExprKind::Collate(x, _) => self.eval_with(x, row, group, queries)?,
            ExprKind::Cast(x, a) => {
                scalar::cast_encoded(self.eval_with(x, row, group, queries)?, *a, self.encoding)?
            }
            ExprKind::Unary(op, x) => {
                let v = self.eval_with(x, row, group, queries)?;
                match op {
                    Unary::Plus => v,
                    Unary::Not => boolean(scalar::truth(&v)?.map(|b| !b)),
                    Unary::BitNot => {
                        if scalar::null(&v) {
                            Value::Null
                        } else {
                            Value::Integer(!scalar::integer(&v)?)
                        }
                    }
                    Unary::Minus if min_magnitude(x) => Value::Integer(i64::MIN),
                    Unary::Minus => match scalar::numeric(&v)? {
                        Value::Integer(n) => n
                            .checked_neg()
                            .map(Value::Integer)
                            .unwrap_or_else(|| scalar::real(-(n as f64))),
                        Value::Real(n) => scalar::real(-n),
                        _ => Value::Null,
                    },
                }
            }
            ExprKind::Binary(op, a, b) => {
                if rows::width(a) > 1 {
                    let mut a = rows::Input::new(a, row, self, queries)?;
                    let mut b = rows::Input::new(b, row, self, queries)?;
                    return self.compare_rows(*op, &mut a, &mut b, row, group, queries);
                }
                let av = self.eval_with(a, row, group, queries)?;
                let bv = self.eval_with(b, row, group, queries)?;
                self.binary(*op, a, b, av, bv)?
            }
            ExprKind::Between(x, a, b, not) => {
                if rows::width(x) > 1 {
                    let mut x = rows::Input::new(x, row, self, queries)?;
                    x.materialize(row, group, self, queries)?;
                    let mut a = rows::Input::new(a, row, self, queries)?;
                    let low = self.compare_rows(
                        Binary::GreaterEqual,
                        &mut x,
                        &mut a,
                        row,
                        group,
                        queries,
                    )?;
                    let mut b = rows::Input::new(b, row, self, queries)?;
                    let high =
                        self.compare_rows(Binary::LessEqual, &mut x, &mut b, row, group, queries)?;
                    return Ok(boolean(
                        and(scalar::truth(&low)?, scalar::truth(&high)?).map(|v| v ^ not),
                    ));
                }
                let xv = self.eval_with(x, row, group, queries)?;
                let av = self.eval_with(a, row, group, queries)?;
                let bv = self.eval_with(b, row, group, queries)?;
                let low = self.binary(Binary::GreaterEqual, x, a, xv.clone(), av)?;
                let high = self.binary(Binary::LessEqual, x, b, xv, bv)?;
                let value = and(scalar::truth(&low)?, scalar::truth(&high)?);
                boolean(value.map(|b| b ^ not))
            }
            ExprKind::In(x, values, not) => {
                if values.is_empty() {
                    return Ok(boolean(Some(*not)));
                }
                let xv = self.eval_with(x, row, group, queries)?;
                let mut found = false;
                let mut null_seen = scalar::null(&xv);
                for value in values {
                    let v = self.eval_with(value, row, group, queries)?;
                    if scalar::null(&v) {
                        null_seen = true;
                    } else if !scalar::null(&xv)
                        && scalar::compare_affinity(
                            xv.clone(),
                            v,
                            expr_affinity(x),
                            Affinity::None,
                            comparison_collation(x, value),
                            self.encoding,
                        )? == Ordering::Equal
                    {
                        found = true;
                        break;
                    }
                }
                boolean(if found {
                    Some(!not)
                } else if values.is_empty() {
                    Some(*not)
                } else if null_seen {
                    None
                } else {
                    Some(*not)
                })
            }
            ExprKind::Case(base, arms, other) => {
                if let Some(base) = base.as_ref().filter(|b| rows::width(b) > 1) {
                    let mut value = rows::Input::new(base, row, self, queries)?;
                    value.materialize(row, group, self, queries)?;
                    for (when, then) in arms {
                        let mut when = rows::Input::new(when, row, self, queries)?;
                        if scalar::truth(&self.compare_rows(
                            Binary::Equal,
                            &mut value,
                            &mut when,
                            row,
                            group,
                            queries,
                        )?)? == Some(true)
                        {
                            return self.eval_with(then, row, group, queries);
                        }
                    }
                    return other
                        .as_ref()
                        .map(|e| self.eval_with(e, row, group, queries))
                        .unwrap_or(Ok(Value::Null));
                }
                let base_value = base
                    .as_ref()
                    .map(|b| self.eval_with(b, row, group, queries))
                    .transpose()?;
                let mut selected = None;
                for (when, then) in arms {
                    let v = self.eval_with(when, row, group, queries)?;
                    let matches = if let (Some(base), Some(bv)) = (base, base_value.as_ref()) {
                        scalar::truth(&self.binary(Binary::Equal, base, when, bv.clone(), v)?)?
                            == Some(true)
                    } else {
                        scalar::truth(&v)? == Some(true)
                    };
                    if matches {
                        selected = Some(then);
                        break;
                    }
                }
                if let Some(expr) = selected.or(other.as_deref()) {
                    self.eval_with(expr, row, group, queries)?
                } else {
                    Value::Null
                }
            }
            ExprKind::Call { name, args, .. } => {
                if aggregate(name, args.len()) {
                    self.aggregate(
                        expr,
                        group.ok_or_else(|| error("aggregate outside grouping"))?,
                        queries,
                    )?
                } else if name == "coalesce" || name == "ifnull" {
                    let mut value = Value::Null;
                    for arg in args {
                        value = self.eval_with(arg, row, group, queries)?;
                        if !scalar::null(&value) {
                            break;
                        }
                    }
                    value
                } else if name == "iif" || name == "if" {
                    let mut selected = None;
                    for pair in args.chunks_exact(2) {
                        if scalar::truth(&self.eval_with(&pair[0], row, group, queries)?)?
                            == Some(true)
                        {
                            selected = Some(&pair[1]);
                            break;
                        }
                    }
                    if selected.is_none() && args.len() % 2 == 1 {
                        selected = args.last();
                    }
                    selected
                        .map(|x| self.eval_with(x, row, group, queries))
                        .transpose()?
                        .unwrap_or(Value::Null)
                } else {
                    let mut values = Vec::new();
                    let mut bytes = 0usize;
                    for arg in args {
                        let value = self.eval_with(arg, row, group, queries)?;
                        bytes = bytes
                            .checked_add(scalar::size(&value) + core::mem::size_of::<Value>())
                            .ok_or(Error::Limit("function argument bytes"))?;
                        if bytes > self.limits.max_database_bytes {
                            return Err(Error::Limit("function argument bytes"));
                        }
                        values.push(value);
                    }
                    self.function(name, args, &values)?
                }
            }
        };
        if scalar::size(&value) > self.limits.max_value_bytes {
            return Err(Error::Limit("SQL value size"));
        }
        Ok(match value {
            Value::Real(n) if n.is_nan() => Value::Null,
            v => v,
        })
    }
    fn binary(&mut self, op: Binary, a: &Expr, b: &Expr, av: Value, bv: Value) -> Result<Value> {
        match op {
            Binary::And => return Ok(boolean(and(scalar::truth(&av)?, scalar::truth(&bv)?))),
            Binary::Or => {
                let (a, b) = (scalar::truth(&av)?, scalar::truth(&bv)?);
                return Ok(boolean(if a == Some(true) || b == Some(true) {
                    Some(true)
                } else if a == Some(false) && b == Some(false) {
                    Some(false)
                } else {
                    None
                }));
            }
            Binary::Is | Binary::IsNot => {
                if let ExprKind::Boolean(want) = b.kind {
                    return Ok(boolean(Some(
                        (scalar::truth(&av)? == Some(want)) ^ (op == Binary::IsNot),
                    )));
                }
                if scalar::null(&av) || scalar::null(&bv) {
                    return Ok(boolean(Some(
                        (scalar::null(&av) && scalar::null(&bv)) ^ (op == Binary::IsNot),
                    )));
                }
            }
            _ => {}
        }
        if scalar::null(&av) || scalar::null(&bv) {
            return Ok(Value::Null);
        }
        if matches!(
            op,
            Binary::Equal
                | Binary::NotEqual
                | Binary::Less
                | Binary::LessEqual
                | Binary::Greater
                | Binary::GreaterEqual
                | Binary::Is
                | Binary::IsNot
        ) {
            let cmp = scalar::compare_affinity(
                av,
                bv,
                expr_affinity(a),
                expr_affinity(b),
                comparison_collation(a, b),
                self.encoding,
            )?;
            return Ok(boolean(Some(match op {
                Binary::Equal | Binary::Is => cmp == Ordering::Equal,
                Binary::NotEqual | Binary::IsNot => cmp != Ordering::Equal,
                Binary::Less => cmp == Ordering::Less,
                Binary::LessEqual => cmp != Ordering::Greater,
                Binary::Greater => cmp == Ordering::Greater,
                _ => cmp != Ordering::Less,
            })));
        }
        scalar::arithmetic(op, av, bv, self.limits.max_value_bytes)
    }
    fn function(&mut self, name: &str, args: &[Expr], v: &[Value]) -> Result<Value> {
        let string = |s: String| Value::Text(Text::utf8(&s));
        let value = match name {
            "changes" => Value::Integer(self.changes as i64),
            "total_changes" => Value::Integer(self.total_changes as i64),
            "last_insert_rowid" => Value::Integer(self.last_rowid),
            "typeof" => string(
                match v[0] {
                    Value::Null => "null",
                    Value::Integer(_) => "integer",
                    Value::Real(_) => "real",
                    Value::Text(_) => "text",
                    Value::Blob(_) => "blob",
                }
                .into(),
            ),
            "coalesce" | "ifnull" => unreachable!(),
            "nullif" => {
                if !scalar::null(&v[0])
                    && !scalar::null(&v[1])
                    && scalar::compare_encoded(
                        &v[0],
                        &v[1],
                        comparison_collation(&args[0], &args[1]),
                        self.encoding,
                    )? == Ordering::Equal
                {
                    Value::Null
                } else {
                    v[0].clone()
                }
            }
            "hex" => {
                let bytes = if matches!(v[0], Value::Null) {
                    Vec::new()
                } else if let Value::Text(t) = &v[0] {
                    t.transcode(self.encoding)?.bytes
                } else {
                    scalar::text_bytes(&v[0])?
                };
                if bytes.len() > self.limits.max_value_bytes / 2 {
                    return Err(Error::Limit("SQL value size"));
                }
                string(bytes.iter().map(|b| format!("{b:02X}")).collect())
            }
            "char" => {
                let mut s = String::new();
                for value in v {
                    let n = scalar::integer(value)?;
                    s.push(
                        u32::try_from(n)
                            .ok()
                            .and_then(char::from_u32)
                            .unwrap_or('\u{fffd}'),
                    );
                }
                string(s)
            }
            "like" | "glob" => self.pattern(name, &v[0], &v[1], v.get(2))?,
            _ if v.iter().any(scalar::null) => Value::Null,
            "length" => Value::Integer(if let Value::Blob(b) = &v[0] {
                b.len()
            } else {
                scalar::text(&v[0])?
                    .split('\0')
                    .next()
                    .unwrap_or("")
                    .chars()
                    .count()
            } as i64),
            "octet_length" => Value::Integer(match &v[0] {
                Value::Blob(b) => b.len(),
                v => Text::utf8(&scalar::text(v)?)
                    .transcode(self.encoding)?
                    .bytes
                    .len(),
            } as i64),
            "lower" | "upper" => {
                let mut bytes = scalar::text_bytes(&v[0])?;
                if name == "lower" {
                    bytes.make_ascii_lowercase();
                } else {
                    bytes.make_ascii_uppercase();
                }
                Value::Text(Text {
                    bytes,
                    encoding: Encoding::Utf8,
                })
            }
            "unicode" => scalar::text(&v[0])?
                .chars()
                .next()
                .map(|c| Value::Integer(i64::from(u32::from(c))))
                .unwrap_or(Value::Null),
            "abs" => match &v[0] {
                Value::Integer(n) => {
                    Value::Integer(n.checked_abs().ok_or_else(|| error("integer overflow"))?)
                }
                _ => scalar::real(scalar::float(&scalar::numeric(&v[0])?).abs()),
            },
            "unhex" => {
                let bytes = scalar::text_bytes(&v[0])?;
                let ignore = if v.len() == 2 {
                    scalar::text_bytes(&v[1])?
                } else {
                    Vec::new()
                };
                let mut digits = Vec::new();
                let mut invalid = false;
                for b in bytes {
                    if let Some(n) = char::from(b).to_digit(16) {
                        digits.push(n as u8);
                    } else if !ignore.contains(&b) {
                        invalid = true;
                        break;
                    }
                }
                if invalid || digits.len() % 2 != 0 {
                    Value::Null
                } else {
                    Value::Blob(
                        digits
                            .chunks_exact(2)
                            .map(|pair| (pair[0] << 4) | pair[1])
                            .collect(),
                    )
                }
            }
            "min" | "max" => {
                let mut result = v[0].clone();
                let c = args
                    .iter()
                    .find_map(explicit_collation)
                    .unwrap_or_else(|| collation(&args[0]));
                for value in &v[1..] {
                    let cmp = scalar::compare_encoded(value, &result, c, self.encoding)?;
                    if (name == "min" && cmp == Ordering::Less)
                        || (name == "max" && cmp == Ordering::Greater)
                    {
                        result = value.clone();
                    }
                }
                result
            }
            "trim" | "ltrim" | "rtrim" => {
                let s = scalar::text(&v[0])?;
                let chars: Vec<char> = if v.len() == 2 {
                    scalar::text(&v[1])?
                } else {
                    " ".into()
                }
                .chars()
                .collect();
                let s = if name != "rtrim" {
                    s.trim_start_matches(|c| chars.contains(&c))
                } else {
                    &s
                };
                let s = if name != "ltrim" {
                    s.trim_end_matches(|c| chars.contains(&c))
                } else {
                    s
                };
                string(s.to_string())
            }
            "replace" => {
                let s = scalar::text(&v[0])?;
                let from = scalar::text(&v[1])?;
                let to = scalar::text(&v[2])?;
                if from.is_empty() {
                    string(s)
                } else {
                    let count = s.matches(&from).count();
                    let added = count
                        .checked_mul(to.len())
                        .and_then(|n| n.checked_add(s.len() - count * from.len()))
                        .ok_or(Error::Limit("SQL value size"))?;
                    if added > self.limits.max_value_bytes {
                        return Err(Error::Limit("SQL value size"));
                    }
                    string(s.replace(&from, &to))
                }
            }
            "instr" => {
                if let (Value::Blob(a), Value::Blob(b)) = (&v[0], &v[1]) {
                    Value::Integer(if b.is_empty() {
                        1
                    } else {
                        a.windows(b.len())
                            .position(|x| x == b)
                            .map(|n| n as i64 + 1)
                            .unwrap_or(0)
                    })
                } else {
                    let a = scalar::text(&v[0])?;
                    let b = scalar::text(&v[1])?;
                    Value::Integer(
                        a.find(&b)
                            .map(|n| a[..n].chars().count() as i64 + 1)
                            .unwrap_or(0),
                    )
                }
            }
            "substr" | "substring" => {
                let start = scalar::integer(&v[1])?;
                let length = if v.len() == 3 {
                    scalar::integer(&v[2])?
                } else {
                    i64::MAX
                };
                if let Value::Blob(b) = &v[0] {
                    let (a, z) = substring_range(b.len(), start, length);
                    Value::Blob(b[a..z].to_vec())
                } else {
                    let s = scalar::text(&v[0])?;
                    let chars: Vec<char> = s.split('\0').next().unwrap_or("").chars().collect();
                    let (a, z) = substring_range(chars.len(), start, length);
                    string(chars[a..z].iter().collect())
                }
            }
            _ => return Err(error(format!("unimplemented function: {name}"))),
        };
        Ok(value)
    }
}
fn substring_range(size: usize, start: i64, length: i64) -> (usize, usize) {
    let size = size as i128;
    let start = start as i128;
    let mut count = length as i128;
    let mut at = if start < 0 {
        size + start
    } else if start > 0 {
        start - 1
    } else {
        0
    };
    if start == 0 && count > 0 {
        count -= 1;
    }
    if at < 0 {
        if count > 0 {
            count = (count + at).max(0);
        }
        at = 0;
    }
    let (from, to) = if count < 0 {
        ((at + count).max(0), at)
    } else {
        (at, at + count)
    };
    (from.clamp(0, size) as usize, to.clamp(0, size) as usize)
}
fn boolean(value: Option<bool>) -> Value {
    value
        .map(|b| Value::Integer(i64::from(b)))
        .unwrap_or(Value::Null)
}
fn min_magnitude(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::MinMagnitude => true,
        ExprKind::Unary(Unary::Plus, x) => min_magnitude(x),
        _ => false,
    }
}
fn and(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    if a == Some(false) || b == Some(false) {
        Some(false)
    } else if a == Some(true) && b == Some(true) {
        Some(true)
    } else {
        None
    }
}

fn compensated_add(sum: &mut f64, correction: &mut f64, n: f64) {
    let next = *sum + n;
    if sum.abs() > n.abs() {
        *correction += (*sum - next) + n;
    } else {
        *correction += (n - next) + *sum;
    }
    *sum = next;
}
