# STRICT tables

The safe SQL engine accepts `CREATE TABLE ... (...) STRICT` for its supported
rowid and [WITHOUT ROWID](WITHOUT_ROWID.md) tables. Columns must declare `INT`, `INTEGER`, `REAL`, `TEXT`, `BLOB` or
`ANY`. Missing and other declared types are rejected. The declaration survives
database-image export/import; this adds no C dependency or engine fallback.

```sql
CREATE TABLE readings(id INTEGER PRIMARY KEY, value REAL, original ANY) STRICT;
INSERT INTO readings VALUES(NULL, '12.5', '00012');
SELECT value, typeof(value), original, typeof(original) FROM readings;
-- 12.5 | real | 00012 | text
PRAGMA table_list(readings);
-- schema=main, type=table, ncol=3, wr=0, strict=1
```

## Types and constraints

Ordinary affinity conversions run before storage-class validation. Numeric text
can become an integer or real; integral reals can become integers; numbers can
become text. Invalid numeric text and incompatible BLOB/text values cause an
error. NULL remains allowed unless a NOT NULL or primary-key rule prohibits it.
`ANY` preserves the supplied storage class and contents, including numeric-looking
text, BLOBs and embedded NULs. An ordinary non-STRICT `ANY` column retains its
existing numeric-affinity behavior. Numeric affinity follows the reference's
NUL-terminated conversion behavior; `ANY` does not perform this conversion.

Non-rowid primary-key columns become implicitly NOT NULL. An `INTEGER PRIMARY
KEY` rowid alias still accepts NULL on INSERT to allocate a rowid. `INT PRIMARY
KEY` and the column-form `INTEGER PRIMARY KEY DESC` do not alias rowid.

NOT NULL checks and REPLACE defaults precede type validation. Type validation
precedes an applicable CHECK or index check, or occurs after rowid conflict
resolution when neither ran. This ordering matters for IGNORE and UPSERT DO
NOTHING. INSERT, UPDATE and UPSERT DO UPDATE use the same checks; updates only
recheck NOT NULL, CHECK and index constraints affected by assigned columns.

## Errors and transaction state

Incompatible STRICT values and rowid assignments return `Error::Datatype`.
This Rust API does not expose SQLite's numeric error codes or exact messages.
A datatype error is not skipped by OR IGNORE and does not select the statement's
OR FAIL/ROLLBACK behavior. Type errors reset `changes()` to zero, leave
`total_changes()` at its previous value, and retain the last successfully inserted
rowid even when its insertion is undone.

The pinned SQLite 3.53.4 reference does not make `OP_TypeCheck` itself request a
statement journal. Within an explicit transaction, earlier writes in the same
statement can therefore survive a datatype error. Another emitted constraint
capable of ABORT, including constraints in a DO UPDATE action, causes statement
rollback instead. Autocommit rolls back the implicit transaction. The safe engine
models those decisions for the implemented constraints. Rowid type mismatches
follow the same journal behavior in ordinary tables as well.

For example, after `CREATE TABLE t(x INT) STRICT; BEGIN;`, executing
`INSERT INTO t VALUES(1),('bad')` returns a datatype error and leaves the first
row visible, with zero changes counted. COMMIT persists it; ROLLBACK or a prior
savepoint can undo it. The journaled API marks such retained data dirty even
though `changes()` is zero. Resource errors always restore the statement.

## Catalog and verification

`PRAGMA [main.|temp.]table_list` returns `schema`, `name`, `type`, `ncol`, `wr`
and `strict`, optionally filtering by name with parentheses or `=`. It includes
main tables/views/system tables and the empty temp schema. It does not enable
temporary-table DDL. Catalog row order is unspecified. SQLite's historical
filter names `sqlite_master` and `sqlite_temp_master` produce the displayed
names `sqlite_schema` and `sqlite_temp_schema`.

Eight Rust tests cover prepared values, declaration rejection, primary keys,
transaction/savepoint counters, conflict ordering, resource limits, catalog flags
and 72 page-size/encoding/auto-vacuum roundtrips. A Unix adapter test covers
commit/reopen of a retained prefix and unchanged files after aborted statements.

```sh
python3 legacy/scripts/build_oracle.py
python3 safe/scripts/strict_differential.py
python3 platform/scripts/file_differential.py
```

The STRICT script compares 2,608 declaration/value/conflict/transaction/catalog
and image scenarios against SQLite 3.53.4. Comparisons include typed values,
counters and transaction state after each statement. File tests cover all three
encodings, all eight page sizes and both auto-vacuum modes. Nine additional native
file scenarios verify persistence with a connection kept open across Rust writes.

## Remaining limits

Foreign keys, triggers, virtual
tables, table-valued pragmas and complete upstream SQL compatibility remain
unfinished. SQLite integrity/quick-check pragmas are not implemented. Import
parses declarations and stored values but is not a comprehensive validation of
constraints in a corrupt database; native integrity checks validate test outputs.

For an unresolved view, a filtered table_list reports zero columns. Enumerating
valid and invalid views together can produce different column counts from native
SQLite because its parser error state and cached metadata affect later views.
The safe engine resolves each view independently; this difference remains open.
The reference also leaves missing index entries after a datatype error in an
explicit transaction executing `INSERT OR REPLACE` on a colliding rowid before
its index type check. The safe engine defers replacement deletions and rebuilds
indexes when exporting, so it does not reproduce that corrupt-index state.
Index access paths and exact physical error side effects remain outside this
implementation's compatibility claims.
Materialized-query evaluation and error timing retain the limits documented in
[QUERIES.md](QUERIES.md), [UPSERT.md](UPSERT.md) and [RETURNING.md](RETURNING.md).
Passing this subset does not establish complete STRICT or SQLite compatibility.

Generated columns are now supported within the implemented expression subset;
see [GENERATED.md](GENERATED.md) for their evaluation, constraints and file layout.
