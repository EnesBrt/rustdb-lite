# Column renames

The safe engine implements `ALTER TABLE [main.]table RENAME [COLUMN] old TO new`
for ordinary, STRICT and WITHOUT ROWID tables within the implemented main-schema
SQL surface. This complements [table renames](RENAME.md), [ADD/DROP COLUMN](ALTER_TABLE.md)
and [constraint edits](ALTER_CONSTRAINTS.md). The complete rewrite remains unfinished.

## Bound references and schema text

A rename updates the declared name, PRIMARY KEY and UNIQUE key lists, CHECK and
generated expressions, explicit index keys and partial predicates, and stored
view references that resolve to the original table column. Renaming an INTEGER
PRIMARY KEY also updates references spelled rowid, oid or _rowid_ when they resolve
to that alias. Hidden rowids themselves are not renameable declarations.

The parser retains separate source coordinates for qualifiers and column names.
Schema expressions and queries bind against the original schema before edits
are applied. Explicit aliases, CTE column lists, unrelated names, comments and
literals retain their meaning. Used and unused CTE definitions, correlated
references, compounds and aggregate modifiers participate in reference tracing.
Names keep unquoted spelling when both the old occurrence and requested new name
are unquoted; otherwise replacements use escaped double quotes. Same-name and
case-only column renames are allowed; collisions with another column are rejected.

SQLite's double-quoted-string fallback is supported in ordinary expressions:
`"missing"` becomes text only when it does not resolve to a column or alias.
Backtick and bracket identifiers do not have this fallback. Result labels retain
the source spelling of fallback literals. Parenthesized DEFAULT expressions still
reject unresolved identifiers; a bare quoted DEFAULT retains its original text.

Before column replacement, double-quoted strings in supported schema expressions
and views are rewritten as escaped single-quoted literals. This also covers
unrelated table declarations and partial-index predicates, so the new column
cannot capture an existing string. Index key expressions that rely on unresolved
double-quoted names reject the rename, matching the pinned reference. Historical
single-quoted index column names are rebound and replaced as quoted identifiers.

Native schema resolution has observable limitations that are retained: USING
lists are not renamed, and ORDER BY names introduced by wildcard expansion are
not rewritten. For example, renaming `a` in a view `SELECT * FROM t ORDER BY a`
fails and restores the original state. Explicit projection aliases remain stable;
unaliased projected names can change, invalidating dependent views or subqueries.
Quoted unresolved view expressions can subsequently use the native string
fallback. Success therefore does not promise that all view output labels or
semantics are invariant under the rename.

## Atomic state and persistence

Edited table/index definitions are reparsed in isolated builders. Existing row
identities, stored generated values, sequence state, indexes and change counters
are preserved. The completed views are validated before success. A failure at
any stage, including work/SQL-size limits, restores the preceding schema and rows.
Transactions and savepoints restore all edited definitions and data together.
Prepared queries bind again when executed; old column names can become invalid,
while explicitly aliased views can remain usable.

The journaled API persists the resulting schema through its full-image commit
protocol. This does not implement incremental B-tree/schema-cookie updates.

## Verification and remaining work

```sh
cargo test -p sqlite-safe-core --test rename_column
python3 safe/scripts/rename_column_differential.py
cargo test -p sqlite-safe-platform --test unix column_rename
python3 platform/scripts/file_differential.py
```

Seven Rust tests cover schema text, quote distinctions, name resolution, rowid
aliases, prepared reuse, counters, generated values, failures, savepoints and
resource bounds. Image tests cover 72 page-size/encoding/auto-vacuum combinations,
with both ordinary and WITHOUT ROWID tables in each image. The differential suite
compares 1,035 error/value/metadata/schema/transaction/image scenarios to SQLite
3.53.4, including subsequent native edits and native integrity checks. A Unix
Rust test and nine native file cases cover persistent readers, commit/reopen,
rollback and unchanged files after rejected edits.

Foreign keys, triggers, virtual tables, temporary/attached schemas,
legacy_alter_table/writable_schema behavior, configurable DQS flags, exact native
diagnostics, prepare-time schema validation and the C API remain unfinished.
DROP COLUMN still lacks the schema-wide string normalization described in
[ALTER_TABLE.md](ALTER_TABLE.md). General query/planner and platform limitations
remain recorded in [SQL.md](SQL.md) and [STATUS.md](STATUS.md).
