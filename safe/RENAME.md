# Table renames

The safe engine implements `ALTER TABLE [main.]old RENAME TO new` for ordinary,
STRICT and WITHOUT ROWID tables. This complements [column additions/removals](ALTER_TABLE.md)
and [constraint edits](ALTER_CONSTRAINTS.md). Column RENAME and the complete
SQLite rewrite remain unfinished.

## Schema and name resolution

A successful rename updates the table declaration, automatic index names,
explicit index target names and qualified partial-index predicates, qualified
CHECK references, stored views and matching sqlite_sequence entries. New table
references use double-quoted identifiers with escaped embedded quotes. Unrelated
names, explicit aliases, literals, comments and surrounding SQL text are retained.

The parser retains source coordinates separately from semantic expression
equality. The normal query binder records table sources and qualified references
that actually resolve to the renamed table. This handles alias and CTE shadowing,
qualified wildcards, correlated references, compound ORDER BY terms and table-name IN expressions.
Expanding another stored view does not mix its source coordinates into the
calling view's edit. Unused CTE declarations are visited separately; their
semantic errors do not invalidate a view, while resolved references still change.

The schema resolver follows the reference's projected-name behavior for nested
join groups. Consequently some queries that execute normally cannot participate
in a rename, as in native SQLite. Invalid unrelated views and name collisions
also fail the rename. A nearer alias can prevent an outer qualifier from being
rewritten; resulting unresolved references reject the edit, matching the native
behavior even if the original correlated query executes successfully. A new name cannot collide with a table, view or index,
including the old name with different ASCII casing; sqlite_ names are reserved.

## Atomic state and storage

The edited table and indexes are reparsed and rebound before replacement. Values,
rowids, stored generated values, uniqueness rules and change counters are retained.
Sequence entries match the old table name exactly, including user-edited duplicate
rows. Prepared statements resolve the schema again on execution: a statement
using the old table name fails, while one using an updated view can still succeed.

Any error, including a resource limit or an invalid resulting view, restores the
statement's previous schema and rows. Transactions and savepoints restore names,
view definitions and sequence contents together. The journaled API persists the
same operation through the existing full-image commit protocol. This does not
implement incremental schema-cookie or B-tree page edits.

## Verification and remaining scope

```sh
cargo test -p sqlite-safe-core --test rename
python3 safe/scripts/rename_differential.py
cargo test -p sqlite-safe-platform --test unix table_rename
python3 platform/scripts/file_differential.py
```

Six Rust tests exercise binding, exact schema text, counters, sequences, prepared
reuse, savepoints, rejected edits, bounded work and 72 image configurations, each
containing ordinary and WITHOUT ROWID tables. 514 native comparisons cover errors,
rows, names, metadata, transactions and schema text. Image interchange spans
all eight page sizes, three encodings, three auto-vacuum modes and both table
kinds, including native renames followed by Rust renames and updates. A Unix test
and nine native file cases cover persistent peers, reopen and unchanged files
after failed or rolled-back renames.

Table renames apply to the currently implemented schema surface. Foreign keys,
triggers, virtual tables, temporary/attached schemas, legacy_alter_table and
writable_schema behavior, general double-quoted-string fallback, exact native
diagnostics, the C API and full query planning remain unfinished. Column renames
are not implemented. The parser's source coordinates and binder tracing provide
infrastructure for continuing that work, not evidence that it is complete.
