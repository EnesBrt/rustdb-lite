//! UPSERT target resolution and updates of a conflicting row.
use super::*;
use alloc::{collections::BTreeSet, rc::Rc};
use parser::{Upsert, UpsertAction};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Key {
    Rowid,
    Index(usize),
}
pub(super) struct Bound {
    pub target: Option<Key>,
    action: Action,
}
enum Action {
    Nothing,
    Update {
        assignments: Vec<(usize, Expr)>,
        filter: Option<Expr>,
    },
}
pub(super) fn handler(clauses: &[Bound], key: Key, fuel: &mut Fuel) -> Result<Option<usize>> {
    for (i, clause) in clauses.iter().enumerate() {
        fuel.spend()?;
        if clause.target.is_none_or(|target| target == key) {
            return Ok(Some(i));
        }
    }
    Ok(None)
}
fn target(table: &StoredTable, fields: &[Field], terms: &[Expr], fuel: &mut Fuel) -> Result<Key> {
    let mut resolved = Vec::new();
    for expr in terms {
        fuel.spend()?;
        resolved.push(eval::bind(expr, fields, &[], false)?);
    }
    let terms = resolved;
    let rowid = |slot| slot == table.columns.len() || Some(slot) == table.alias();
    if matches!(terms.as_slice(), [Expr { kind: ExprKind::Slot(slot, _, _), .. }] if rowid(*slot)) {
        return Ok(Key::Rowid);
    }
    // Native matching ignores key order and ASC/DESC. An omitted COLLATE
    // matches the indexed column regardless of the index's collation.
    for replace in [false, true] {
        for (i, index) in table.indexes.iter().enumerate().rev() {
            fuel.spend()?;
            if !index.unique
                || index.terms.len() != terms.len()
                || (index.conflict == Conflict::Replace) != replace
            {
                continue;
            }
            let mut matches = true;
            for key in &index.terms {
                let mut found = false;
                for term in &terms {
                    fuel.spend()?;
                    let (expr, collation) = match &term.kind {
                        ExprKind::Collate(expr, collation) => (expr.as_ref(), Some(*collation)),
                        _ => (term, None),
                    };
                    // The reference resolves an IPK reference to rowid, but
                    // retains its declared column number in a composite index.
                    // Those do not match in sqlite3UpsertAnalyzeTarget().
                    if matches!(expr.kind, ExprKind::Slot(slot, _, _) if slot == key.column && !rowid(slot))
                        && collation.is_none_or(|c| c == key.collation)
                    {
                        found = true;
                        break;
                    }
                }
                if !found {
                    matches = false;
                    break;
                }
            }
            if matches {
                return Ok(Key::Index(i));
            }
        }
    }
    Err(error(
        "ON CONFLICT clause does not match a PRIMARY KEY or UNIQUE constraint",
    ))
}
pub(super) struct Incoming<'a> {
    pub old_id: i64,
    pub id: i64,
    pub values: &'a [Value],
}
impl Connection {
    pub(super) fn bind_upserts(
        &self,
        table: &StoredTable,
        alias: &str,
        clauses: &[Upsert],
        scope: Option<Rc<query::Scope>>,
        runtime: &mut query::Runtime,
        context: &mut Eval<'_>,
    ) -> Result<Vec<Bound>> {
        let fields = table.fields(alias);
        let mut update_fields = fields.clone();
        // A real destination named/aliased excluded shadows the pseudo-table.
        if !alias.eq_ignore_ascii_case("excluded") {
            update_fields.extend(table.fields("excluded").into_iter().map(|mut f| {
                f.qualified_only = true;
                f
            }));
        }
        let mut bound: Vec<Bound> = Vec::new();
        let mut seen = BTreeSet::new();
        for clause in clauses {
            context.fuel.spend()?;
            let target = clause
                .target
                .as_ref()
                .map(|terms| target(table, &fields, terms, context.fuel))
                .transpose()?;
            if let Some(expr) = &clause.target_where {
                self.expressions(scope.clone(), runtime).bind(
                    expr,
                    &fields,
                    &[],
                    false,
                    context,
                )?;
            }
            // SQLite ignores the action of redundant clauses, even if that
            // action would contain an unresolved column or function.
            let duplicate = target.is_some_and(|key| !seen.insert(key));
            let action = match &clause.action {
                UpsertAction::Nothing => Action::Nothing,
                _ if duplicate => Action::Nothing,
                UpsertAction::Update {
                    assignments,
                    filter,
                } => {
                    let assignments = assignments
                        .iter()
                        .map(|(name, expr)| {
                            Ok((
                                table.column_index(name)?,
                                self.expressions(scope.clone(), runtime).bind(
                                    expr,
                                    &update_fields,
                                    &[],
                                    false,
                                    context,
                                )?,
                            ))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    let filter = filter
                        .as_ref()
                        .map(|expr| {
                            self.expressions(scope.clone(), runtime).bind(
                                expr,
                                &update_fields,
                                &[],
                                false,
                                context,
                            )
                        })
                        .transpose()?;
                    Action::Update {
                        assignments,
                        filter,
                    }
                }
            };
            bound.push(Bound { target, action });
        }
        Ok(bound)
    }
    pub(super) fn upsert_row(
        &self,
        table: &StoredTable,
        clause: &Bound,
        incoming: Incoming<'_>,
        scope: Option<Rc<query::Scope>>,
        runtime: &mut query::Runtime,
        context: &mut Eval<'_>,
    ) -> Result<Option<(i64, Vec<Value>)>> {
        let Action::Update {
            assignments,
            filter,
        } = &clause.action
        else {
            return Ok(None);
        };
        let mut values = table
            .rows
            .get(&incoming.old_id)
            .ok_or(Error::Corrupt("UPSERT row missing"))?
            .clone();
        let mut row = table.row(incoming.old_id, &values);
        row.extend(table.row(incoming.id, incoming.values));
        if values_size(&row)? > context.limits.max_database_bytes {
            return Err(Error::Limit("UPSERT row bytes"));
        }
        if !self
            .expressions(scope.clone(), runtime)
            .filter(filter.as_ref(), &row, context)?
        {
            return Ok(None);
        }
        let mut id = incoming.old_id;
        for (column, expr) in assignments {
            let value = self
                .expressions(scope.clone(), runtime)
                .eval(expr, &row, None, context)?;
            if *column == values.len() || Some(*column) == table.alias() {
                id = rowid(value)?;
            } else {
                values[*column] = value;
            }
            if values_size(&values)? > context.limits.max_database_bytes {
                return Err(Error::Limit("UPSERT row bytes"));
            }
        }
        if let Some(alias) = table.alias() {
            values[alias] = Value::Integer(id);
        }
        for (value, column) in values.iter_mut().zip(&table.columns) {
            *value = scalar::affinity(core::mem::replace(value, Value::Null), column.affinity)?;
        }
        match conflict::check(
            table,
            conflict::Row {
                id,
                values: &mut values,
                old: Some(incoming.old_id),
            },
            Conflict::Abort,
            &checks(table)?,
            &defaults(table)?,
            &[],
            context,
        )? {
            conflict::Decision::Write(_) => Ok(Some((id, values))),
            conflict::Decision::Error(_, message) => Err(Error::Constraint(message)),
            _ => Err(Error::Corrupt("unexpected UPSERT update resolution")),
        }
    }
}
