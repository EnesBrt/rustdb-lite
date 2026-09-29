# AUTOINCREMENT and sqlite_sequence

The safe engine implements AUTOINCREMENT on the supported rowid tables. Column
`INTEGER PRIMARY KEY AUTOINCREMENT` and single-column table
`PRIMARY KEY(id AUTOINCREMENT)` declarations are accepted. Other declared types,
composite keys and the non-aliasing column `INTEGER PRIMARY KEY DESC` form are
rejected. The table-constraint DESC form still aliases rowid, matching SQLite.

```sql
CREATE TABLE items(id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT UNIQUE);
INSERT INTO items(name) VALUES('Rust') RETURNING id; -- 1
DELETE FROM items;
INSERT INTO items(name) VALUES('SQLite') RETURNING id; -- 2
SELECT name,seq FROM sqlite_sequence;
```

## Allocation and statement behavior

- Automatically generated identifiers exceed both the table's current largest
  rowid and its saved sequence value. Deleting rows does not lower that value.
  Explicit identifiers remain allowed, including zero and negative values.
- A statement tracks attempted insertion rowids before constraint checks.
  Successful IGNORE and UPSERT statements can therefore consume identifiers for
  rows they skip or update. Updates of existing rowids do not directly advance
  the sequence; a later automatic allocation also checks the table's actual rows.
- The sequence is written after successful INSERT execution, including a
  zero-row INSERT. RETURNING expressions observe the prior sequence contents.
  Internal sequence writes do not change changes(), total_changes() or
  last_insert_rowid(). Explicit SQL writes to sqlite_sequence count normally.
- Constraint FAIL keeps its successful table-write prefix but does not run the
  final sequence write. The next allocation accounts for those retained rows.
  ABORT restores the statement, and ROLLBACK/savepoints restore sequence state
  with the other table contents.
- Exhausting the signed 64-bit rowid range returns `Error::Full`, overriding
  conflict policies and rolling back the active transaction and savepoints.
  Earlier completed statements still contribute to total_changes(), and the
  last successfully inserted rowid remains observable even after rollback.

## System table and images

The first AUTOINCREMENT declaration creates the ordinary two-column
`sqlite_sequence(name,seq)` table. It is available to SELECT, INSERT, UPDATE,
DELETE, RETURNING and the implemented schema-inspection pragmas. Users cannot
create, drop or index it. Dropping an AUTOINCREMENT table removes all matching
sequence entries but retains the system table itself.

Explicit edits can create duplicates and values of different storage types.
Allocation uses the first row with an exact text name, converts its saved value
to an integer, and replaces it only when the tracked value increases. A name's
case and storage class matter. These behaviors are included in native tests;
sequence edits can change the usual non-reuse guarantee.

Snapshots preserve the system table's canonical declaration and rows, including
empty sequence tables, UTF-8/UTF-16 encodings and auto-vacuum layouts. Import
rejects missing sequence schemas and altered system-table declarations rather
than silently inventing replacement high-water values. The public ImageBuilder
still refuses reserved table names; the SQL layer has a restricted internal path
for its validated sequence table.

## Evidence and remaining scope

Six Rust tests cover prepared bindings, conflict/counter timing, FULL rollback,
manual sequence edits, savepoints, malformed declarations, resource bounds and
encoded snapshots. A Unix test covers retained FAIL writes, reopening and FULL
transaction rollback without changing the previously committed file.

```sh
python3 legacy/scripts/build_oracle.py
python3 safe/scripts/sequence_differential.py
python3 platform/scripts/file_differential.py
```

The sequence comparison contains 268 scenarios against SQLite 3.53.4, including
typed values and counters, native/Rust image interchange, a Rust-created schema,
empty system tables and rejection of four corrupted schema variants. Nine file
cases cover native readers across encodings and auto-vacuum modes.

Ordinary-table random rowid allocation after i64::MAX remains unimplemented. This
also limits sequence-row creation if someone explicitly assigns that rowid to
sqlite_sequence itself. Triggers, attached/temporary schemas, foreign keys,
WITHOUT ROWID SQL tables and C API compatibility remain unfinished.
STRICT rowid tables and their type-error behavior are covered in [STRICT.md](STRICT.md).
The engine still rebuilds complete images; incremental B-tree updates and native
planner parity are separate outstanding work.
