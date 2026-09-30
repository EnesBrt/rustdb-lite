//! Stable, fuel-bounded sorting shared by query and aggregate orderings.
use super::{
    error,
    eval::{self, Fuel},
    parser::Ordering,
    scalar,
};
use crate::{Encoding, Result, Value};
use alloc::vec::Vec;
use core::cmp::Ordering as Compare;

pub(super) fn compare_keys(
    a: &[Value],
    b: &[Value],
    order: &[Ordering],
    encoding: Encoding,
) -> Result<Compare> {
    for ((a, b), term) in a.iter().zip(b).zip(order) {
        let cmp = if scalar::null(a) != scalar::null(b) {
            let nulls_first = term.nulls_first.unwrap_or(!term.descending);
            if scalar::null(a) == nulls_first {
                Compare::Less
            } else {
                Compare::Greater
            }
        } else {
            let cmp = scalar::compare_encoded(a, b, eval::collation(&term.expr), encoding)?;
            if term.descending {
                cmp.reverse()
            } else {
                cmp
            }
        };
        if cmp != Compare::Equal {
            return Ok(cmp);
        }
    }
    Ok(Compare::Equal)
}
pub(super) fn sort_by<T>(
    rows: Vec<T>,
    fuel: &mut Fuel,
    compare: impl Fn(&T, &T) -> Result<Compare>,
) -> Result<Vec<T>> {
    let mut indices: Vec<usize> = (0..rows.len()).collect();
    let mut buffer = indices.clone();
    let mut width = 1;
    while width < indices.len() {
        let mut start = 0;
        while start < indices.len() {
            let middle = (start + width).min(indices.len());
            let end = (middle + width).min(indices.len());
            let (mut a, mut b, mut at) = (start, middle, start);
            while a < middle || b < end {
                fuel.spend()?;
                if b == end
                    || (a < middle
                        && compare(&rows[indices[a]], &rows[indices[b]])? != Compare::Greater)
                {
                    buffer[at] = indices[a];
                    a += 1;
                } else {
                    buffer[at] = indices[b];
                    b += 1;
                }
                at += 1;
            }
            start = end;
        }
        core::mem::swap(&mut indices, &mut buffer);
        width = width.saturating_mul(2);
    }
    let mut rows: Vec<Option<T>> = rows.into_iter().map(Some).collect();
    indices
        .into_iter()
        .map(|i| {
            rows[i]
                .take()
                .ok_or_else(|| error("invalid sort permutation"))
        })
        .collect()
}
