//! STRICT storage classes and the reference's statement-journal decisions.
use super::*;

pub(super) fn declaration(table: &mut StoredTable) -> Result<()> {
    if table.strict {
        for column in &mut table.columns {
            // Native metadata dequotes the first token of a quoted type, but
            // STRICT validates the full declaration before that normalization.
            if !column.single_type_token {
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
            if column.virtual_column() {
                continue;
            }
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
    // OP_Function/OP_PureFunc request a statement journal even for a non-unique
    // index. This also applies to functions in generated columns, which every
    // write computes before constraint processing. SQLite implements these four
    // conditional functions inline, without emitting a function opcode.
    fn function(expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::Call { name, .. } if !["coalesce", "ifnull", "iif", "if"].contains(&name.to_ascii_lowercase().as_str()))
            || expr.children().iter().any(|e| function(e))
    }
    if table.generated.as_ref().is_some_and(|schema| {
        schema
            .columns
            .iter()
            .any(|c| c.expression.as_ref().is_some_and(function))
    }) {
        return Ok(true);
    }
    for index in &table.indexes {
        fuel.spend()?;
        if change.index(table, index)
            && (index
                .predicate
                .as_ref()
                .is_some_and(|p| function(&p.signature))
                || index.terms.iter().any(
                    |term| matches!(&term.key, IndexKey::Expression(e) if function(&e.signature)),
                ))
        {
            return Ok(true);
        }
    }
    for (i, column) in table.columns.iter().enumerate() {
        fuel.spend()?;
        if column.not_null
            && Some(i) != table.alias()
            && (column.generated.is_some() || change.column(table, i))
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
                let columns =
                    table.changed_columns(assignments.iter().map(|(i, _)| *i).collect(), fuel)?;
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
