//! USING/NATURAL binding keeps physical qualified fields separate from the
//! value exposed by unqualified references and an unqualified wildcard.
use super::*;
use alloc::boxed::Box;

pub(super) fn using(
    fields: &mut [Field],
    start: usize,
    source: &parser::Source,
    has_right: bool,
    fuel: &mut Fuel,
) -> Result<Vec<Expr>> {
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
            depth: 1 + lhs.depth.max(rhs.depth),
            token: None,
            kind: ExprKind::Binary(parser::Binary::Equal, Box::new(lhs), Box::new(rhs)),
        });
        fields[right].qualified_only = true;
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
