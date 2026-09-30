# ALTER TABLE column edits

The safe engine implements `ALTER TABLE [main.]name ADD [COLUMN] definition`
and `ALTER TABLE [main.]name DROP [COLUMN] name` for ordinary rowid and WITHOUT
ROWID tables. This is part of the ongoing rewrite, not complete ALTER TABLE
compatibility.

```sql
CREATE TABLE items(id INTEGER PRIMARY KEY, label TEXT, obsolete BLOB);
INSERT INTO items(label) VALUES('Rust');
ALTER TABLE items ADD quantity INT NOT NULL DEFAULT 1;
ALTER TABLE items ADD doubled INT AS(quantity*2);
ALTER TABLE items DROP obsolete;
SELECT * FROM items;
```

Column parsing is shared with CREATE TABLE. Edits preserve the surrounding
schema text, quoted names and comments, then rebuild the table and index
metadata through the same safe schema validator. Existing row identities,
AUTOINCREMENT sequence values and stored generated values are retained. Explicit
index expressions and predicates bind to the new column positions. Prepared
queries resolve against the current schema when executed.

## Adding a column

PRIMARY KEY and UNIQUE additions are rejected. On a nonempty table, an ordinary
NOT NULL column needs an eligible default and a STORED generated column cannot
be added. Empty tables can accept nonconstant defaults, NOT NULL without a
default and STORED generated columns, as in the pinned SQLite 3.53.4 reference.
Future INSERTs still enforce the definition and evaluate their normal defaults.

Existing records receive literal, signed or CAST defaults with the declared
column affinity. Numeric literal spelling can matter: a TEXT default of `1.20e2`
is read as `'1.20e2'` for an old record, while a later INSERT evaluates the numeric
expression and stores `'120.0'`. Image import uses this same missing-field rule.
STRICT ANY preserves the default's storage class. Nonconstant expressions such
as `(1+2)` cannot populate old records.

New CHECK constraints, generated NOT NULL constraints and STRICT declarations
validate existing rows. CHECK permits NULL. The statement returns no rows, but
some successful additions expose a validation-expression column name, matching
the pinned reference's result metadata. DDL does not increment changes(),
total_changes() or last_insert_rowid().

Schema reload can preserve a virtual generated cycle introduced by ADD; reading
the cycle and writes fail with bounded errors, while unrelated ordinary reads
remain possible. CREATE TABLE still rejects virtual cycles. A validation scan
can reject a cycle even on an empty table.

## Dropping a column and failure isolation

Dropping the only column, a primary-key column or an inline UNIQUE column fails.
Remaining index, CHECK and generated expressions must resolve against the resulting
schema. Unresolved unquoted view references also reject the edit. A CHECK belonging
only to the removed column disappears with it. Quoted column references in
index/generated expressions are revalidated without string fallback before
publishing the replacement, so they cannot silently become string literals.
DROP validates deferred view definitions; ADD does not validate unrelated views. Explicit view lists with a width mismatch
retain the reference's separate read-time error behavior.

Before removing a column, the engine resolves schema expressions against the
original schema and converts double-quoted string fallbacks to escaped
single-quoted literals. This includes unrelated table CHECK/generated expressions,
partial-index predicates and stored views, including unused CTE definitions.
Identifiers, comments and bare quoted defaults are preserved. The normalizer is
shared with column renames; its mode without a rename target never changes names.
For example, dropping `b` from `t(a,b,g AS("future"))` produces `g AS('future')`,
so a subsequently added column named `future` cannot capture that existing string.

After removal, table and index expressions bind again with double-quoted string
fallback disabled, including checks referring to generated columns. A missing
quoted column must not silently turn into text in a remaining CHECK. Names that
still resolve are accepted: removing a real `rowid` column can expose the hidden
rowid, and removing `true` or `false` can reveal unquoted SQL boolean constants.
Stored generated values remain unchanged; virtual values and future writes use
the resulting bindings.
Views follow SQLite's separate behavior: an original `SELECT "b" FROM t` can
become a string result after `b` is removed and bind to a column again if `b` is
later reintroduced. An originally unresolved double-quoted literal is normalized
before removal and remains a literal. Index key expressions with unresolved
quoted names reject the edit, even in an unrelated table.

Each operation restores the previous schema and rows on error, including any
unrelated objects normalized before a later failure. Savepoints and
transaction rollback restore both together. Journaled connections persist a
successful edit through the existing commit protocol, and failed edits do not
change the committed file. Images and indexes are rebuilt; this is not SQLite's
incremental schema-cookie, B-tree or page-update implementation.

## Verification and unfinished scope

```sh
cargo test -p sqlite-safe-core --test alter
cargo test -p sqlite-safe-core --test drop_quotes
python3 safe/scripts/alter_differential.py
python3 safe/scripts/drop_quote_differential.py
cargo test -p sqlite-safe-platform --test unix alter_columns
python3 platform/scripts/file_differential.py
```

The native suite compares 2,943 column/default/dependency/state/image scenarios.
It checks all eight page sizes, all three encodings and all three auto-vacuum
modes for both table kinds, including native-origin short records, schema text,
metadata, native integrity checks and subsequent native writes. Seven Rust tests
cover prepared reuse, cycles, rollback, malformed prefixes, limits and 72 image
configurations. One Unix Rust test and nine native file scenarios cover persistent
readers, commit/reopen and byte-unchanged files after failed or rolled-back edits.
The shared native statement harness reads column metadata after stepping because
SQLite can reprepare a statement after a schema change; pre-step metadata can be
stale even when the executed query is correct.

ALTER COLUMN SET/DROP NOT NULL, ADD CHECK and DROP CONSTRAINT are described in
[ALTER_CONSTRAINTS.md](ALTER_CONSTRAINTS.md). [Table renames](RENAME.md) and
[column renames](RENAME_COLUMN.md) are implemented. Remaining work includes
temporary/attached schemas, foreign keys, triggers,
virtual tables, legacy_alter_table/writable_schema behavior, schema cookies,
prepare-time validation and the C API. Missing functions, such as date/time
functions, remain unavailable in new defaults and generated expressions.

An additional 695 native comparisons cover DROP literal normalization, quoted
CHECK dependencies, view/CTE resolution, transactions, exact schema text and
encoded images. Six Rust tests cover later name capture, rowid/boolean rebinding,
prepared view reuse, counters, stored-value preservation, bounded work and rollback of normalized
unrelated objects, plus 72 image configurations containing both table kinds. A
Unix Rust test and nine native file scenarios cover persistent peers and
byte-unchanged files after failed or rolled-back normalization.

The full-image writer rebuilds index entries. SQLite 3.53.4 can retain stale
entries when DROP changes an index expression's binding: for example,
`CREATE TABLE t(a,true); CREATE INDEX ix ON t(true); INSERT INTO t VALUES(1,2);`
followed by `ALTER TABLE t DROP true` succeeds but native `integrity_check`
reports a missing index entry. The safe writer rebuilds this index. Physical
index scans and constraint timing involving such stale native entries are not
reproduced; incremental index storage remains unfinished. The name-rebinding
comparisons above cover logical values/metadata and remove indexes before later
writes, so they do not establish parity for that stale-index behavior.

Behavior was checked against the pinned public SQLite 3.53.4 `alter.c`, `parse.y`,
`build.c` and `vdbemem.c` source and the independently built native executable.
The safe engine never calls the native implementation as a fallback.
