//! Row constraint decisions. Deletions are applied only after every check passes.
use super::*;
use alloc::collections::BTreeSet;

pub(super) enum Decision {
    Write(Vec<i64>),
    Ignore,
    Error(Conflict, String),
}
fn violation(policy: Conflict, message: String) -> Decision {
    if policy == Conflict::Ignore {
        Decision::Ignore
    } else {
        Decision::Error(policy, message)
    }
}
pub(super) struct Row<'a> {
    pub id: i64,
    pub values: &'a mut [Value],
    pub old: Option<i64>,
}
pub(super) fn check(
    table: &StoredTable,
    row: Row<'_>,
    policy: Conflict,
    checks: &[Expr],
    defaults: &[Option<Expr>],
    context: &mut Eval<'_>,
) -> Result<Decision> {
    // Defaults substituted by REPLACE are checked in a second pass, allowing
    // another first-pass NOT NULL IGNORE/FAIL rule to determine the outcome.
    for pass in 0..2 {
        for (i, column) in table.columns.iter().enumerate() {
            if !column.not_null || !scalar::null(&row.values[i]) {
                continue;
            }
            let mut action = policy.resolve(column.not_null_conflict);
            if pass == 1 && action != Conflict::Replace {
                continue;
            }
            if action == Conflict::Replace {
                if pass == 0 {
                    if let Some(default) = &defaults[i] {
                        row.values[i] =
                            scalar::affinity(context.eval(default, &[], None)?, column.affinity)?;
                        continue;
                    }
                }
                action = Conflict::Abort;
            }
            return Ok(violation(
                action,
                format!("NOT NULL: {}.{}", table.name, column.name),
            ));
        }
    }
    for value in row.values.iter() {
        if scalar::size(value) > context.limits.max_value_bytes {
            return Err(Error::Limit("SQL value size"));
        }
    }
    let values = table.row(row.id, row.values);
    for check in checks {
        if scalar::truth(&context.eval(check, &values, None)?)? == Some(false) {
            let action = policy.resolve(Conflict::Abort);
            return Ok(violation(
                if action == Conflict::Replace {
                    Conflict::Abort
                } else {
                    action
                },
                "CHECK".into(),
            ));
        }
    }
    let mut replaced = BTreeSet::new();
    let rowid_policy = policy.resolve(table.key_conflict);
    let rowid_collision = table.rows.contains_key(&row.id) && Some(row.id) != row.old;
    if rowid_collision && rowid_policy != Conflict::Replace {
        return Ok(violation(rowid_policy, format!("{}.rowid", table.name)));
    }
    // SQLite keeps REPLACE indexes after all other policies, with reverse
    // creation order inside each group. This prevents deletion before IGNORE.
    for replace in [false, true] {
        for index in table.indexes.iter().rev().filter(|i| i.unique) {
            let action = policy.resolve(index.conflict);
            if (action == Conflict::Replace) != replace {
                continue;
            }
            // Updating another column cannot introduce a duplicate of an
            // unchanged key. Avoid rescanning (and re-encoding) every stored
            // key, which is particularly expensive for long UTF-16 strings.
            if row
                .old
                .and_then(|id| table.rows.get(&id))
                .is_some_and(|old| {
                    index
                        .terms
                        .iter()
                        .all(|t| row.values[t.column] == old[t.column])
                })
            {
                continue;
            }
            if index
                .terms
                .iter()
                .any(|t| scalar::null(&row.values[t.column]))
            {
                continue;
            }
            for (prior_id, prior) in &table.rows {
                context.fuel.spend()?;
                if Some(*prior_id) == row.old || replaced.contains(prior_id) {
                    continue;
                }
                let mut equal = true;
                for term in &index.terms {
                    context.fuel.spend()?;
                    if scalar::compare_encoded(
                        &row.values[term.column],
                        &prior[term.column],
                        term.collation,
                        context.encoding,
                    )? != Compare::Equal
                    {
                        equal = false;
                        break;
                    }
                }
                if equal {
                    if action != Conflict::Replace {
                        return Ok(violation(action, format!("UNIQUE: {}", index.name)));
                    }
                    replaced.insert(*prior_id);
                }
            }
        }
    }
    if rowid_collision {
        replaced.insert(row.id);
    }
    Ok(Decision::Write(replaced.into_iter().collect()))
}
