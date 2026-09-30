//! Row-valued expressions remain expression operands, never stored SQL values.
use super::*;
use alloc::rc::Rc;

pub fn width(expr: &Expr) -> usize {
    match &expr.kind {
        ExprKind::Vector(values) => values.len(),
        ExprKind::BoundSubquery(q) if q.source.mode != QueryMode::Exists => q.columns.len(),
        _ => 1,
    }
}
pub fn scalar(expr: &Expr) -> Result<()> {
    if width(expr) != 1 {
        return Err(error("row value misused"));
    }
    Ok(())
}
fn same(a: &Expr, b: &Expr) -> Result<()> {
    if width(a) != width(b) {
        return Err(error("row value column count mismatch"));
    }
    Ok(())
}
pub fn comparison(op: Binary) -> bool {
    matches!(
        op,
        Binary::Equal
            | Binary::NotEqual
            | Binary::Less
            | Binary::LessEqual
            | Binary::Greater
            | Binary::GreaterEqual
            | Binary::Is
            | Binary::IsNot
    )
}
pub fn validate(expr: &Expr) -> Result<()> {
    match &expr.kind {
        ExprKind::BoundSubquery(_) => {}
        ExprKind::RowField(query, column, count) => {
            if width(query) != *count || *column >= *count {
                return Err(error("assignment column count mismatch"));
            }
        }
        ExprKind::Binary(op, a, b) if comparison(*op) => same(a, b)?,
        ExprKind::Between(x, a, b, _) => {
            same(x, a)?;
            same(x, b)?;
        }
        ExprKind::InQuery(x, q, _) => same(x, q)?,
        ExprKind::In(x, values, _) => {
            if !values.is_empty() {
                scalar(x)?;
            }
            for value in values {
                scalar(value)?;
            }
        }
        ExprKind::Case(base, arms, other) => {
            for (when, then) in arms {
                if let Some(base) = base {
                    same(base, when)?;
                } else {
                    scalar(when)?;
                }
                scalar(then)?;
            }
            if let Some(other) = other {
                scalar(other)?;
            }
        }
        _ => {
            for child in expr.children() {
                scalar(child)?;
            }
        }
    }
    Ok(())
}

pub(super) struct Input<'a> {
    expr: &'a Expr,
    query: Option<Rc<Vec<Vec<Value>>>>,
    values: Vec<Option<Value>>,
    bytes: usize,
}
impl<'a> Input<'a> {
    pub(super) fn new(
        expr: &'a Expr,
        row: &[Value],
        context: &mut Eval<'_>,
        queries: &mut dyn Subqueries,
    ) -> Result<Self> {
        let query = match &expr.kind {
            ExprKind::BoundSubquery(q) if q.source.mode == QueryMode::Scalar => {
                Some(queries.run(q, row, context)?)
            }
            _ => None,
        };
        Ok(Self {
            expr,
            query,
            values: alloc::vec![None; width(expr)],
            bytes: 0,
        })
    }
    fn part(&self, column: usize) -> Expr {
        match &self.expr.kind {
            ExprKind::Vector(values) => values[column].clone(),
            ExprKind::BoundSubquery(q) if q.columns.len() > 1 => {
                let meta = &q.columns[column];
                let base = Expr {
                    location: Default::default(),
                    kind: if let Some(collation) = meta.collation {
                        ExprKind::Slot(column, meta.affinity, collation)
                    } else {
                        ExprKind::Cast(Box::new(Expr::literal(Value::Null)), meta.affinity)
                    },
                    depth: 1,
                    token: None,
                };
                if let Some(collation) = meta.explicit_collation {
                    Expr {
                        location: Default::default(),
                        kind: ExprKind::Collate(Box::new(base), collation),
                        depth: 2,
                        token: None,
                    }
                } else {
                    base
                }
            }
            _ => self.expr.clone(),
        }
    }
    fn value(
        &mut self,
        column: usize,
        row: &[Value],
        group: Option<&[Vec<Value>]>,
        context: &mut Eval<'_>,
        queries: &mut dyn Subqueries,
    ) -> Result<Value> {
        if let Some(value) = &self.values[column] {
            return Ok(value.clone());
        }
        let value = if let Some(query) = &self.query {
            query
                .first()
                .and_then(|r| r.get(column))
                .cloned()
                .unwrap_or(Value::Null)
        } else {
            context.eval_with(&self.part(column), row, group, queries)?
        };
        self.bytes = self
            .bytes
            .checked_add(scalar::size(&value) + core::mem::size_of::<Value>())
            .ok_or(Error::Limit("row value bytes"))?;
        if self.bytes > context.limits.max_database_bytes {
            return Err(Error::Limit("row value bytes"));
        }
        self.values[column] = Some(value.clone());
        Ok(value)
    }
    pub(super) fn materialize(
        &mut self,
        row: &[Value],
        group: Option<&[Vec<Value>]>,
        context: &mut Eval<'_>,
        queries: &mut dyn Subqueries,
    ) -> Result<()> {
        for column in 0..self.values.len() {
            self.value(column, row, group, context, queries)?;
        }
        Ok(())
    }
}
impl Eval<'_> {
    pub(super) fn compare_rows(
        &mut self,
        op: Binary,
        left: &mut Input<'_>,
        right: &mut Input<'_>,
        row: &[Value],
        group: Option<&[Vec<Value>]>,
        queries: &mut dyn Subqueries,
    ) -> Result<Value> {
        let equality = matches!(
            op,
            Binary::Equal | Binary::NotEqual | Binary::Is | Binary::IsNot
        );
        let null_equal = matches!(op, Binary::Is | Binary::IsNot);
        let mut unknown = false;
        for column in 0..left.values.len() {
            self.fuel.spend()?;
            let a = left.value(column, row, group, self, queries)?;
            let b = right.value(column, row, group, self, queries)?;
            let cmp = if scalar::null(&a) || scalar::null(&b) {
                if null_equal {
                    if scalar::null(&a) == scalar::null(&b) {
                        Ordering::Equal
                    } else {
                        Ordering::Less
                    }
                } else {
                    if !equality {
                        return Ok(Value::Null);
                    }
                    unknown = true;
                    continue;
                }
            } else {
                let ae = left.part(column);
                let be = right.part(column);
                scalar::compare_affinity(
                    a,
                    b,
                    expr_affinity(&ae),
                    expr_affinity(&be),
                    comparison_collation(&ae, &be),
                    self.encoding,
                )?
            };
            if cmp != Ordering::Equal {
                return Ok(boolean(Some(match op {
                    Binary::NotEqual | Binary::IsNot => true,
                    Binary::Less | Binary::LessEqual => cmp == Ordering::Less,
                    Binary::Greater | Binary::GreaterEqual => cmp == Ordering::Greater,
                    _ => false,
                })));
            }
        }
        Ok(boolean(if unknown {
            None
        } else {
            Some(matches!(
                op,
                Binary::Equal | Binary::Is | Binary::LessEqual | Binary::GreaterEqual
            ))
        }))
    }
    pub(super) fn in_query(
        &mut self,
        x: &Expr,
        query: &BoundSubquery,
        not: bool,
        row: &[Value],
        group: Option<&[Vec<Value>]>,
        queries: &mut dyn Subqueries,
    ) -> Result<Value> {
        let mut left = Input::new(x, row, self, queries)?;
        left.materialize(row, group, self, queries)?;
        let rows = queries.run(query, row, self)?;
        let mut unknown = false;
        for values in rows.iter() {
            self.fuel.spend()?;
            let mut equal = true;
            let mut row_unknown = false;
            for (column, value) in values.iter().enumerate() {
                let lhs = left.value(column, row, group, self, queries)?;
                if scalar::null(&lhs) || scalar::null(value) {
                    row_unknown = true;
                    continue;
                }
                let meta = &query.columns[column];
                let rhs = Expr {
                    location: Default::default(),
                    kind: ExprKind::Slot(
                        column,
                        meta.affinity,
                        meta.collation.unwrap_or(Collation::Binary),
                    ),
                    depth: 1,
                    token: None,
                };
                let lhs_expr = left.part(column);
                let affinity = scalar::comparison_affinity(expr_affinity(&lhs_expr), meta.affinity);
                if scalar::compare_encoded(
                    &scalar::affinity(lhs, affinity)?,
                    &scalar::affinity(value.clone(), affinity)?,
                    comparison_collation(&lhs_expr, &rhs),
                    self.encoding,
                )? != Ordering::Equal
                {
                    equal = false;
                    break;
                }
            }
            if equal && !row_unknown {
                return Ok(boolean(Some(!not)));
            }
            unknown |= equal && row_unknown;
        }
        Ok(boolean(if unknown { None } else { Some(not) }))
    }
}
