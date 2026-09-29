# Constraint conflict policies

The supported rowid-table SQL now implements `ROLLBACK`, `ABORT`, `FAIL`,
`IGNORE`, and `REPLACE`. `INSERT OR ...` and `UPDATE OR ...` override schema
policies; `REPLACE INTO` is the INSERT OR REPLACE alias. PRIMARY KEY, UNIQUE,
and NOT NULL constraints can declare `ON CONFLICT` policies. Ordinary INSERT
and UPDATE inherit them, defaulting to ABORT.

| Policy | Constraint violation behavior |
| --- | --- |
| ABORT | Return an error and undo changes made by the current statement |
| FAIL | Return an error while retaining rows already changed by this statement |
| ROLLBACK | Return an error and undo the active transaction, including its savepoints; without a transaction, undo the statement |
| IGNORE | Skip the offending row and continue with subsequent rows |
| REPLACE | Delete conflicting primary/unique-key rows, then write the new row; substitute a NOT NULL column's default where available |

REPLACE uses ABORT if a NOT NULL violation has no usable default, or a CHECK
constraint fails. CHECK otherwise follows the statement policy. SQLite accepts
but ignores ON CONFLICT on a table CHECK constraint; the parser preserves this
behavior. It rejects ON CONFLICT on a column CHECK. Repeated equivalent key
constraints share one automatic index and must have compatible explicit policies.

NOT NULL defaults are substituted before CHECK and key validation. A second
pass checks substituted defaults that are themselves NULL. Key conflicts that
cause IGNORE/FAIL/ABORT/ROLLBACK are resolved before replacement deletions.
UPDATE captures candidate rowids first, reads their current values when updating,
and skips rows already removed by a previous replacement. Unchanged unique keys
do not require another scan of the table, including for UTF-16 images.

`changes()` and `total_changes()` count successful inserts/updates, including
the prefix retained by FAIL; replacement deletions do not add to these counters.
Rolling back does not subtract earlier successful statements from total_changes.
`last_insert_rowid()` retains the last successfully inserted rowid, even if that
insert is later undone by ABORT or ROLLBACK. Resource and evaluation errors abort
the statement regardless of its constraint policy.

## Persistence and API behavior

`Connection::execute` returns an error for FAIL even when rows were retained.
In autocommit mode, `JournaledConnection` persists that prefix before returning
the constraint error. Inside a transaction, it remains private until COMMIT or
outermost RELEASE. A constraint ROLLBACK discards pending changes and savepoints.

If persisting a FAIL prefix itself fails, the API returns the persistence error
and requires reopening, just as for other failed commits. Inspect the recovered
state before retrying. The snapshot CLI stops on errors before writing `--save`;
use the connection APIs to inspect and save retained changes explicitly.

## Evidence and remaining differences

Six Rust tests cover policy precedence, defaults, multiple conflicting keys,
updates, schema import/export, transaction/counter behavior, malformed prefixes,
and resource errors. Separate pager and Unix adapter tests exercise retained
prefix persistence, rolled-back transactions, and injected failures during prefix
commit. The native comparison script is:

```sh
python3 legacy/scripts/build_oracle.py
python3 safe/scripts/conflict_differential.py
```

It compares success/error outcomes, rows, value types, autocommit state, changes,
total_changes and last_insert_rowid after each statement, plus schema interchange
in all three text encodings. The Python test harness loads the pinned SQLite
3.53.4 reference library; the Rust implementation does not link to it. Exact
native error messages/codes are not covered by this comparison.

UPSERT (`ON CONFLICT ... DO UPDATE/NOTHING`) is described in [UPSERT.md](UPSERT.md).
Buffered RETURNING rows and error behavior are described in [RETURNING.md](RETURNING.md).
This does not implement AUTOINCREMENT, foreign keys, triggers, virtual tables, or the C API. Index access
paths and streaming INSERT SELECT execution remain pending. Query materialization
can change evaluation/error timing, and the lack of a planner can change which
rows precede a FAIL when native SQLite chooses a different scan order. Arbitrary
combinations of these policies with the full upstream SQL surface are not covered.
