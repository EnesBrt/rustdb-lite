//! Row constraint decisions. Deletions are applied only after every check passes.
use super::*;
use alloc::collections::BTreeSet;

pub(super) enum Decision {
    Write(Vec<i64>),
    Ignore,
    Error(Conflict, String),
    Upsert { clause: usize, id: i64 },
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
    pub change: Change<'a>,
}
#[derive(Clone, Copy)]
pub(super) struct Change<'a> {
    pub columns: Option<&'a [usize]>,
    pub rowid: bool,
}
impl<'a> Change<'a> {
    pub fn update(table: &StoredTable, columns: &'a [usize]) -> Self {
        Self {
            columns: Some(columns),
            rowid: columns
                .iter()
                .any(|i| *i == table.columns.len() || Some(*i) == table.alias()),
        }
    }
    pub fn column(self, table: &StoredTable, i: usize) -> bool {
        self.columns.is_none_or(|columns| columns.contains(&i))
            || (self.rowid && (Some(i) == table.alias() || i == table.columns.len()))
    }
    pub fn expression(self, table: &StoredTable, expr: &Expr) -> bool {
        self.columns.is_none()
            || matches!(expr.kind, ExprKind::Slot(i, _, _) if self.column(table, i))
            || matches!(&expr.kind, ExprKind::Generated(r) if self.column(table, r.column))
            || expr.children().iter().any(|e| self.expression(table, e))
    }
    pub fn index(self, table: &StoredTable, index: &StoredIndex) -> bool {
        self.rowid
            || index
                .predicate
                .as_ref()
                .is_some_and(|p| self.expression(table, &p.signature))
            || index.terms.iter().any(|t| match &t.key {
                IndexKey::Column(i) => self.column(table, *i),
                IndexKey::Expression(e) => self.expression(table, &e.signature),
            })
            || (table.without_rowid && table.primary_key.iter().any(|i| self.column(table, *i)))
    }
}
pub(super) fn check(
    table: &StoredTable,
    row: Row<'_>,
    policy: Conflict,
    checks: &[Expr],
    defaults: &[Option<Expr>],
    upserts: &[upsert::Bound],
    context: &mut Eval<'_>,
) -> Result<Decision> {
    // Defaults substituted by REPLACE are checked in a second pass, allowing
    // another first-pass NOT NULL IGNORE/FAIL rule to determine the outcome.
    let mut substituted = false;
    for pass in 0..2 {
        if pass == 1 && substituted {
            table.compute_generated(row.values, context)?;
        }
        for (i, column) in table.columns.iter().enumerate() {
            if !column.not_null
                || Some(i) == table.alias()
                || (column.generated.is_some() && pass == 0)
                || (column.generated.is_none() && !row.change.column(table, i))
                || !scalar::null(&row.values[i])
            {
                continue;
            }
            let mut action = policy.resolve(column.not_null_conflict);
            if pass == 1 && action != Conflict::Replace && column.generated.is_none() {
                continue;
            }
            if action == Conflict::Replace {
                if pass == 0 {
                    if let Some(default) = &defaults[i] {
                        row.values[i] =
                            scalar::affinity(context.eval(default, &[], None)?, column.affinity)?;
                        substituted = true;
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
    let mut type_checked = false;
    for check in checks {
        if !row.change.expression(table, check) {
            continue;
        }
        if !type_checked {
            strict::values(table, row.values)?;
            type_checked = true;
        }
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
    let mut order = Vec::new();
    let mut seen = BTreeSet::new();
    for clause in upserts {
        context.fuel.spend()?;
        if let Some(key) = clause.target {
            if seen.insert(key) {
                order.push(key);
            }
        }
    }
    let mut add = |key| {
        if seen.insert(key) {
            order.push(key);
        }
    };
    // Explicit targets run in clause order before other unique constraints.
    // Remaining indexes follow SQLite's non-REPLACE/REPLACE schema order.
    if rowid_policy != Conflict::Replace
        || policy == Conflict::Replace
        || upsert::handler(upserts, upsert::Key::Rowid, context.fuel)?.is_some()
    {
        add(upsert::Key::Rowid);
    }
    for replace in [false, true] {
        for (i, index) in table.indexes.iter().enumerate().rev() {
            context.fuel.spend()?;
            if (index.conflict == Conflict::Replace) == replace {
                add(upsert::Key::Index(i));
            }
        }
    }
    add(upsert::Key::Rowid);
    for key in order {
        context.fuel.spend()?;
        let (action, message, collisions) = match key {
            upsert::Key::Rowid => {
                if !row.change.rowid {
                    continue;
                }
                let collision = table.rows.contains_key(&row.id)
                    && Some(row.id) != row.old
                    && !replaced.contains(&row.id);
                (
                    rowid_policy,
                    format!("{}.rowid", table.name),
                    if collision { vec![row.id] } else { Vec::new() },
                )
            }
            upsert::Key::Index(i) => {
                let index = &table.indexes[i];
                if !row.change.index(table, index) {
                    continue;
                }
                if !type_checked {
                    strict::values(table, row.values)?;
                    type_checked = true;
                }
                if !index.includes(table, row.id, row.values, context)? {
                    continue;
                }
                let key_values = index.values(table, row.id, row.values, context)?;
                if !index.unique {
                    continue;
                }
                // Only full column indexes can skip comparing identical inputs.
                // Entering a partial index can create a duplicate even when its
                // key values did not change.
                if (index.predicate.is_none()
                    && row
                        .old
                        .and_then(|id| table.rows.get(&id))
                        .is_some_and(|old| {
                            index.terms.iter().all(|t| match t.key {
                                IndexKey::Column(i) => {
                                    !table.columns[i].virtual_column() && row.values[i] == old[i]
                                }
                                _ => false,
                            })
                        }))
                    || key_values.iter().any(scalar::null)
                {
                    continue;
                }
                let mut collisions = Vec::new();
                for (prior_id, prior) in &table.rows {
                    context.fuel.spend()?;
                    if Some(*prior_id) == row.old || replaced.contains(prior_id) {
                        continue;
                    }
                    if !index.includes(table, *prior_id, prior, context)? {
                        continue;
                    }
                    let mut equal = true;
                    for (term, value) in index.terms.iter().zip(&key_values) {
                        context.fuel.spend()?;
                        if scalar::compare_encoded(
                            value,
                            &term.value(table, *prior_id, prior, context)?,
                            term.collation,
                            context.encoding,
                        )? != Compare::Equal
                        {
                            equal = false;
                            break;
                        }
                    }
                    if equal {
                        collisions.push(*prior_id);
                    }
                }
                (
                    policy.resolve(index.conflict),
                    format!("UNIQUE: {}", index.name),
                    collisions,
                )
            }
        };
        for id in collisions {
            if let Some(clause) = upsert::handler(upserts, key, context.fuel)? {
                return Ok(Decision::Upsert { clause, id });
            }
            if action != Conflict::Replace {
                return Ok(violation(action, message));
            }
            replaced.insert(id);
        }
    }
    if !type_checked {
        strict::values(table, row.values)?;
    }
    Ok(Decision::Write(replaced.into_iter().collect()))
}
