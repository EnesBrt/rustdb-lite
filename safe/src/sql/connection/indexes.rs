//! Column/expression keys and partial-index membership. All evaluation uses the
//! safe expression engine, with the predicate checked before computing any key.
use super::*;
use alloc::boxed::Box;

pub(super) fn same_expression(table: &StoredTable, a: &Expr, b: &Expr) -> bool {
    compare_expression(table, a, b) == 0
}
pub(super) fn matches_target(
    table: &StoredTable,
    target: &Expr,
    key: &IndexExpression,
    collation: Collation,
) -> bool {
    if matches!(key.signature.kind, ExprKind::Collate(..)) {
        compare_expression(table, target, &key.signature) < 2
    } else {
        let wrapped = Expr {
            kind: ExprKind::Collate(Box::new(key.signature.clone()), collation),
            depth: key.signature.depth + 1,
            token: None,
        };
        compare_expression(table, target, &wrapped) < 2
    }
}
// SQLite's schema matcher distinguishes exact equality (0), one top-level
// COLLATE wrapper (1), and different expressions (2). Nested differences cannot
// be ignored. In particular, expression keys are not algebraically simplified.
fn compare_expression(table: &StoredTable, a: &Expr, b: &Expr) -> u8 {
    let small = |e: &Expr| match e.kind {
        ExprKind::Literal(Value::Integer(n))
            if e.token.is_none() && (0..=i64::from(i32::MAX)).contains(&n) =>
        {
            Some(n)
        }
        _ => None,
    };
    if small(a).is_some() || small(b).is_some() {
        return if small(a) == small(b) { 0 } else { 2 };
    }
    if core::mem::discriminant(&a.kind) != core::mem::discriminant(&b.kind) {
        if let ExprKind::Collate(e, _) = &a.kind {
            if compare_expression(table, e, b) < 2 {
                return 1;
            }
        }
        if let ExprKind::Collate(e, _) = &b.kind {
            if compare_expression(table, a, e) < 2 {
                return 1;
            }
        }
        return 2;
    }
    if a.token != b.token {
        return 2;
    }
    let normalize = |i| {
        if Some(i) == table.alias() {
            table.columns.len()
        } else {
            i
        }
    };
    let same = match (&a.kind, &b.kind) {
        (ExprKind::Slot(x, _, _), ExprKind::Slot(y, _, _)) => normalize(*x) == normalize(*y),
        (ExprKind::Literal(x), ExprKind::Literal(y)) => x == y,
        (ExprKind::Boolean(x), ExprKind::Boolean(y)) => x == y,
        (ExprKind::Parameter(x), ExprKind::Parameter(y)) => x == y,
        (ExprKind::Unary(x, _), ExprKind::Unary(y, _)) => x == y,
        (ExprKind::Binary(x, ..), ExprKind::Binary(y, ..)) => x == y,
        (ExprKind::Between(_, _, _, x), ExprKind::Between(_, _, _, y)) => x == y,
        (ExprKind::In(_, xs, x), ExprKind::In(_, ys, y)) => x == y && xs.len() == ys.len(),
        (ExprKind::Case(x, xs, xx), ExprKind::Case(y, ys, yy)) => {
            x.is_some() == y.is_some() && xs.len() == ys.len() && xx.is_some() == yy.is_some()
        }
        (ExprKind::Cast(_, x), ExprKind::Cast(_, y)) => x == y,
        (ExprKind::Collate(_, x), ExprKind::Collate(_, y)) => x == y,
        (
            ExprKind::Call {
                name: x,
                args: xs,
                star: xx,
                distinct: xxx,
                order: xo,
                filter: xf,
            },
            ExprKind::Call {
                name: y,
                args: ys,
                star: yy,
                distinct: yyy,
                order: yo,
                filter: yf,
            },
        ) => {
            x.eq_ignore_ascii_case(y)
                && xs.len() == ys.len()
                && xx == yy
                && xxx == yyy
                && xo.len() == yo.len()
                && xf.is_some() == yf.is_some()
                && xo
                    .iter()
                    .zip(yo)
                    .all(|(a, b)| a.descending == b.descending && a.nulls_first == b.nulls_first)
        }
        (ExprKind::MinMagnitude, ExprKind::MinMagnitude) => true,
        _ => false,
    };
    if !same {
        return 2;
    }
    if a.children()
        .iter()
        .zip(b.children())
        .all(|(x, y)| compare_expression(table, x, y) == 0)
    {
        0
    } else {
        2
    }
}

pub(super) fn declaration(
    table: &StoredTable,
    name: &str,
    columns: &[parser::IndexExpression],
    predicate: Option<&Expr>,
    unique: bool,
    sql: &str,
    fuel: &mut Fuel,
) -> Result<StoredIndex> {
    let predicate = predicate.map(|e| bind(table, e, true, fuel)).transpose()?;
    let mut terms = Vec::new();
    for column in columns {
        let mut expr = column.expr.clone();
        // SQLite's historical indexed-column grammar accepts a single-quoted
        // name in this position, including one outer COLLATE wrapper.
        let target = match &mut expr.kind {
            ExprKind::Collate(e, _) => e.as_mut(),
            _ => &mut expr,
        };
        if let ExprKind::Literal(Value::Text(name)) = &target.kind {
            target.kind = ExprKind::Column {
                qualifier: None,
                name: name.to_string()?,
                quoted: false,
            };
        }
        let bound = bind(table, &expr, false, fuel)?;
        let mut plain = &bound.signature;
        while let ExprKind::Collate(child, _) = &plain.kind {
            plain = child;
        }
        let key = match plain.kind {
            ExprKind::Slot(i, _, _) => IndexKey::Column(i),
            _ => IndexKey::Expression(Box::new(bound)),
        };
        let collation_name = match (&column.collation_name, &key) {
            (Some(name), _) => name.clone(),
            (_, IndexKey::Column(i)) => table.columns[*i].collation_name.clone(),
            _ => "BINARY".into(),
        };
        terms.push(IndexTerm {
            key,
            collation: Collation::parse(&collation_name)?,
            collation_name,
            descending: column.descending,
        });
    }
    Ok(StoredIndex {
        name: name.into(),
        sql: Some(sql.into()),
        terms,
        predicate,
        unique,
        primary: false,
        conflict: Conflict::Default,
    })
}
fn validate(expr: &Expr, predicate: bool, fuel: &mut Fuel) -> Result<()> {
    fuel.spend()?;
    if matches!(
        expr.kind,
        ExprKind::Parameter(_) | ExprKind::Subquery(_) | ExprKind::BoundSubquery(_)
    ) || (!predicate
        && matches!(
            expr.kind,
            ExprKind::Column {
                qualifier: Some(_),
                ..
            }
        ))
        || matches!(&expr.kind, ExprKind::Call { name, .. } if ["changes", "total_changes", "last_insert_rowid"].iter().any(|n| name.eq_ignore_ascii_case(n)))
    {
        return Err(error("invalid or non-deterministic index expression"));
    }
    for child in expr.children() {
        validate(child, predicate, fuel)?;
    }
    Ok(())
}
fn bind(
    table: &StoredTable,
    expr: &Expr,
    predicate: bool,
    fuel: &mut Fuel,
) -> Result<IndexExpression> {
    validate(expr, predicate, fuel)?;
    let mut fields = table.fields(&table.name);
    if !predicate {
        fields.truncate(table.columns.len());
    }
    let value = eval::bind_generated(expr, &fields)?;
    for field in &mut fields {
        field.generated = None;
    }
    let signature = eval::bind_generated(expr, &fields)?;
    Ok(IndexExpression { value, signature })
}
impl IndexTerm {
    pub(super) fn column(&self) -> Result<usize> {
        match self.key {
            IndexKey::Column(i) => Ok(i),
            _ => Err(Error::Corrupt("expression in column-only index layout")),
        }
    }
    pub(super) fn value(
        &self,
        table: &StoredTable,
        id: i64,
        values: &[Value],
        context: &mut Eval<'_>,
    ) -> Result<Value> {
        match &self.key {
            IndexKey::Column(i) => table.read_column(*i, id, values, context),
            IndexKey::Expression(e) => context.eval(&e.value, &table.row(id, values), None),
        }
    }
}
impl StoredIndex {
    pub(super) fn includes(
        &self,
        table: &StoredTable,
        id: i64,
        values: &[Value],
        context: &mut Eval<'_>,
    ) -> Result<bool> {
        match &self.predicate {
            Some(p) => Ok(
                scalar::truth(&context.eval(&p.value, &table.row(id, values), None)?)?
                    == Some(true),
            ),
            None => Ok(true),
        }
    }
    pub(super) fn values(
        &self,
        table: &StoredTable,
        id: i64,
        values: &[Value],
        context: &mut Eval<'_>,
    ) -> Result<Vec<Value>> {
        let mut result = Vec::new();
        let mut bytes = 0usize;
        for term in &self.terms {
            context.fuel.spend()?;
            let value = term.value(table, id, values, context)?;
            push_value(&mut result, value, &mut bytes, context.limits)?;
        }
        Ok(result)
    }
}
