//! USING/NATURAL binding keeps physical qualified fields separate from the
//! value exposed by unqualified references and an unqualified wildcard.
use super::*;
use alloc::boxed::Box;

pub(super) struct NestedProjection {
    pub columns: Vec<String>,
    pub expressions: Vec<Expr>,
    pub origins: Vec<eval::NestedField>,
}
pub(super) fn nested_projection(
    fields: &[Field],
    sources: &[(&parser::Source, usize, usize)],
    context: &mut Eval<'_>,
) -> Result<NestedProjection> {
    let mut result = NestedProjection {
        columns: Vec::new(),
        expressions: Vec::new(),
        origins: Vec::new(),
    };
    let mut bytes = 0usize;
    let mut constraints = Vec::new();
    for (source, start, width) in sources {
        constraints.push(names(
            &fields[..start + width],
            *start,
            source,
            context.fuel,
        )?);
    }
    let mut push = |name: String, expr: Expr, mut origin: eval::NestedField| -> Result<()> {
        context.fuel.spend()?;
        if origin.using
            && result
                .origins
                .iter()
                .any(|n| n.using && n.column.eq_ignore_ascii_case(&origin.column))
        {
            origin.no_expand = true;
        }
        bytes = bytes
            .checked_add(
                name.len()
                    + origin.table.len()
                    + origin.column.len()
                    + core::mem::size_of::<Expr>()
                    + core::mem::size_of::<eval::NestedField>(),
            )
            .ok_or(Error::Limit("nested join columns"))?;
        if bytes > context.limits.max_database_bytes || result.columns.len() >= 2000 {
            return Err(Error::Limit("nested join columns"));
        }
        result.columns.push(name);
        result.expressions.push(expr);
        result.origins.push(origin);
        Ok(())
    };
    for (i, (_, start, width)) in sources.iter().enumerate() {
        let next = constraints.get(i + 1).map_or(&[][..], Vec::as_slice);
        for name in next {
            let expression = eval::bind(
                &Expr {
                    location: Default::default(),
                    depth: 1,
                    token: None,
                    kind: ExprKind::Column {
                        qualifier: None,
                        qualifier_location: Default::default(),
                        name: name.clone(),
                        quoted: false,
                        double_quoted: false,
                    },
                },
                fields,
                &[],
                false,
            )?;
            push(
                name.clone(),
                expression,
                eval::NestedField {
                    table: String::new(),
                    column: name.clone(),
                    rowid: false,
                    no_expand: false,
                    using: true,
                    group: 0,
                },
            )?;
        }
        for (slot, field) in fields.iter().enumerate().take(start + width).skip(*start) {
            if field.nested.as_ref().is_some_and(|n| n.rowid) {
                continue;
            }
            let name = if field.hidden {
                let Some(alias) = ["_ROWID_", "ROWID", "OID"].into_iter().find(|name| {
                    !fields[*start..start + width]
                        .iter()
                        .any(|f| !f.hidden && f.name.eq_ignore_ascii_case(name))
                }) else {
                    continue;
                };
                String::from(alias)
            } else {
                field.name.clone()
            };
            let mut origin = field.nested.clone().unwrap_or_else(|| eval::NestedField {
                table: field.table.clone(),
                column: field.name.clone(),
                rowid: field.hidden,
                no_expand: false,
                using: false,
                group: 0,
            });
            origin.using = false;
            origin.no_expand |= constraints[i]
                .iter()
                .chain(next)
                .any(|n| n.eq_ignore_ascii_case(&name));
            push(name, eval::generated::field(field, slot, None)?, origin)?;
        }
    }
    Ok(result)
}

pub(super) fn references_after(expr: &Expr, width: usize) -> bool {
    match &expr.kind {
        ExprKind::Slot(i, ..) => *i >= width,
        ExprKind::Generated(r) => r.outer.is_none() && r.offset + r.column >= width,
        ExprKind::BoundSubquery(q) => q.dependencies.iter().any(|i| *i >= width),
        _ => expr.children().iter().any(|e| references_after(e, width)),
    }
}

fn names(
    fields: &[Field],
    start: usize,
    source: &parser::Source,
    fuel: &mut Fuel,
) -> Result<Vec<String>> {
    let names = if source.natural {
        let mut names = Vec::new();
        for right in &fields[start..] {
            if right.hidden {
                continue;
            }
            for left in &fields[..start] {
                fuel.spend()?;
                if !left.hidden && left.name.eq_ignore_ascii_case(&right.name) {
                    names.push(right.name.clone());
                    break;
                }
            }
        }
        names
    } else {
        source.using.clone().unwrap_or_default()
    };
    Ok(names)
}
pub(super) fn using(
    fields: &mut [Field],
    start: usize,
    source: &parser::Source,
    has_right: bool,
    fuel: &mut Fuel,
) -> Result<Vec<Expr>> {
    let names = names(fields, start, source, fuel)?;
    let mut constraints = Vec::new();
    for name in names {
        fuel.spend()?;
        let mut left = Vec::new();
        let mut right = None;
        for (i, field) in fields.iter().enumerate() {
            fuel.spend()?;
            if !field.hidden && field.name.eq_ignore_ascii_case(&name) {
                if i < start {
                    left.push(i);
                } else if right.is_none() {
                    right = Some(i);
                }
            }
        }
        let (Some(first), Some(right)) = (left.first().copied(), right) else {
            return Err(error(format!(
                "USING column missing from a join side: {name}"
            )));
        };
        if has_right && left.iter().skip(1).any(|i| !fields[*i].qualified_only) {
            return Err(error(format!("ambiguous USING column: {name}")));
        }
        // Native queries containing RIGHT/FULL joins compare a coalescence of
        // all preceding USING columns. Older INNER/LEFT-only queries select the
        // first physical column, even with ambiguous preceding names.
        let lhs = if has_right {
            eval::coalesced(
                left.iter()
                    .map(|i| eval::generated::field(&fields[*i], *i, None))
                    .collect::<Result<_>>()?,
            )?
        } else {
            eval::generated::field(&fields[first], first, None)?
        };
        let rhs = eval::generated::field(&fields[right], right, None)?;
        constraints.push(Expr {
            location: Default::default(),
            depth: 1 + lhs.depth.max(rhs.depth),
            token: None,
            kind: ExprKind::Binary(parser::Binary::Equal, Box::new(lhs), Box::new(rhs)),
        });
        fields[right].qualified_only = true;
        if fields[right]
            .nested
            .as_ref()
            .is_some_and(|n| n.using || !source.right)
        {
            for field in &mut fields[start..] {
                if field
                    .nested
                    .as_ref()
                    .is_some_and(|n| n.column.eq_ignore_ascii_case(&name))
                {
                    field.unqualified_hidden = true;
                }
            }
        }
        if source.right {
            if source.left {
                if fields[first].merged.is_empty() {
                    fields[first].merged.push(first);
                }
                if !fields[first].merged.contains(&right) {
                    fields[first].merged.push(right);
                }
            } else {
                fields[first].merged = vec![right];
            }
        }
    }
    Ok(constraints)
}
