//! STRICT storage classes and the reference's statement-journal decisions.
use super::*;

pub(super) fn declaration(table: &mut StoredTable) -> Result<()> {
    if table.strict {
        for column in &mut table.columns {
            // Native metadata dequotes the first token of a quoted type, but
            // STRICT validates the full declaration before that normalization.
            if !column.strict_type {
                return Err(error(format!(
                    "invalid STRICT type for {}.{}",
                    table.name, column.name
                )));
            }
            match column.declared_type.as_str() {
                "INT" | "INTEGER" | "REAL" | "TEXT" | "BLOB" => {}
                "ANY" => column.affinity = Affinity::Blob,
                _ => {
                    return Err(error(format!(
                        "invalid STRICT type for {}.{}",
                        table.name, column.name
                    )))
                }
            }
        }
    }
    Ok(())
}

pub(super) fn primary_key(table: &mut StoredTable) {
    if table.strict {
        for &i in &table.primary_key {
            if Some(i) != table.alias() && !table.columns[i].not_null {
                table.columns[i].not_null = true;
                table.columns[i].not_null_conflict = Conflict::Abort;
            }
        }
    }
}

/// Affinity is applied before this check. ANY deliberately has BLOB affinity.
pub(super) fn values(table: &StoredTable, values: &[Value]) -> Result<()> {
    if table.strict {
        for (column, value) in table.columns.iter().zip(values) {
            let valid = scalar::null(value)
                || match column.declared_type.as_str() {
                    "ANY" => true,
                    "INT" | "INTEGER" => matches!(value, Value::Integer(_)),
                    "REAL" => matches!(value, Value::Real(_)),
                    "TEXT" => matches!(value, Value::Text(_)),
                    "BLOB" => matches!(value, Value::Blob(_)),
                    _ => false,
                };
            if !valid {
                return Err(Error::Datatype(format!(
                    "{}.{} requires {}",
                    table.name, column.name, column.declared_type
                )));
            }
        }
    }
    Ok(())
}

/// In SQLite 3.53.4 OP_TypeCheck does not itself request a statement journal.
/// An error inside an explicit transaction therefore retains earlier writes
/// unless another emitted constraint can ABORT. It still resets changes() to
/// zero, unlike ON CONFLICT FAIL. Autocommit rolls back the implicit transaction.
pub(super) fn journal(
    table: &StoredTable,
    change: conflict::Change<'_>,
    policy: Conflict,
    checks: &[Expr],
    upserts: &[upsert::Bound],
    fuel: &mut Fuel,
) -> Result<bool> {
    if !table.strict && !change.rowid {
        return Ok(true);
    }
    for (i, column) in table.columns.iter().enumerate() {
        fuel.spend()?;
        if column.not_null
            && Some(i) != table.alias()
            && change.column(table, i)
            && matches!(
                policy.resolve(column.not_null_conflict),
                Conflict::Abort | Conflict::Replace
            )
        {
            return Ok(true);
        }
    }
    if matches!(
        policy.resolve(Conflict::Abort),
        Conflict::Abort | Conflict::Replace
    ) && checks.iter().any(|expr| change.expression(table, expr))
    {
        return Ok(true);
    }
    let keys = change
        .rowid
        .then_some((upsert::Key::Rowid, table.key_conflict))
        .into_iter()
        .chain(
            table
                .indexes
                .iter()
                .enumerate()
                .filter(|(_, index)| index.unique && change.index(table, index))
                .map(|(i, index)| (upsert::Key::Index(i), index.conflict)),
        );
    for (key, default) in keys {
        fuel.spend()?;
        if let Some(clause) = upsert::handler(upserts, key, fuel)? {
            if let upsert::Action::Update { assignments, .. } = &upserts[clause].action {
                let columns: Vec<_> = assignments.iter().map(|(i, _)| *i).collect();
                let update = conflict::Change::update(table, &columns);
                if journal(table, update, Conflict::Abort, checks, &[], fuel)? {
                    return Ok(true);
                }
            }
        } else if policy.resolve(default) == Conflict::Abort {
            return Ok(true);
        }
    }
    Ok(false)
}
