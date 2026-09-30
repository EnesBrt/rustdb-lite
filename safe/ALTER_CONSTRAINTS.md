# ALTER TABLE constraints

The safe engine now implements the pinned SQLite 3.53.4 constraint-edit commands
for ordinary rowid and WITHOUT ROWID tables:

```sql
ALTER TABLE scores ALTER [COLUMN] value SET NOT NULL [ON CONFLICT policy];
ALTER TABLE scores ALTER [COLUMN] value DROP NOT NULL;
ALTER TABLE scores ADD [CONSTRAINT name] CHECK(expression) [ON CONFLICT policy];
ALTER TABLE scores DROP CONSTRAINT name;
```

Brackets above describe optional syntax. Main-schema qualification and quoted
names are supported. This extends [column edits](ALTER_TABLE.md); the complete
SQLite rewrite remains unfinished.

## Validation and subsequent writes

SET NOT NULL reads the target column, including virtual generated values, and
fails if any existing value is NULL. Its conflict policy controls future writes;
IGNORE and REPLACE do not bypass validation of existing NULLs. The first explicit
NOT NULL clause is removed before the replacement is appended, preserving other
column clauses. Its former constraint name disappears. DROP NOT NULL removes
only the first explicit clause, succeeds if none exists, and preserves implicit
primary-key nullability in STRICT and WITHOUT ROWID tables.

ADD CHECK validates the expression against the existing schema, rejects schema
parameters/subqueries/aggregates, and rejects a duplicate named constraint.
Names compare without ASCII case distinctions, including names attached to other
constraint types or otherwise retained labels. Validation locates violations with
`WHERE (expression) IS NOT TRUE`: both false and NULL fail this initial scan. Once
installed, ordinary INSERT/UPDATE CHECK enforcement accepts true or NULL. The
reference's validation expression can resolve TRUE to a real column named TRUE;
this name-binding behavior is covered by the native comparisons.

CHECK uses boolean branch evaluation: branches irrelevant to the requested truth
or NULL outcome are not evaluated. This matters for expressions that can raise
errors. A table-independent validation condition can raise before an empty scan,
while adding CHECK(0) to an empty table succeeds and rejects later inserts.
The ON CONFLICT suffix on a table CHECK is accepted but does not alter its normal
enforcement, following the existing table-constraint grammar.

DROP CONSTRAINT removes the first matching named NOT NULL or CHECK constraint.
Named PRIMARY KEY and UNIQUE constraints cannot be removed. A dangling label or
a label attached to DEFAULT/COLLATE/generated syntax can be removed without
removing that following clause, matching the reference tokenizer. Missing named
constraints are errors.

## Schema, counters and persistence

Constraint spans are found using bounded lexical units. Parenthesized expressions
and type sizes remain indivisible, so their commas, keywords and string contents
cannot become column or constraint boundaries. Edits preserve surrounding schema
text and normalize the removed span's spacing as the reference does. Added clauses
retain trailing block comments and discard trailing line comments before insertion.
The edited CREATE TABLE and indexes are rebuilt through the safe schema validator.

Rows, rowids, stored generated values, sqlite_sequence and change counters are
preserved by DDL. Successful validation commands expose the reference's internal
validation-expression column name but return no result rows. Constraint edits do
not validate unrelated deferred views. Prepared SQL binds against the new schema
on its next execution.

Errors restore the statement's previous schema and data. Savepoints and transaction
rollback restore both together. The journaled API persists successful edits through
its existing commit protocol; failed or rolled-back edits leave the committed
file unchanged. This rebuilds images and indexes, rather than implementing
incremental schema-cookie/page updates.

## Verification and remaining scope

```sh
cargo test -p sqlite-safe-core --test alter_constraints
python3 safe/scripts/alter_constraint_differential.py
cargo test -p sqlite-safe-platform --test unix constraint_edits
python3 platform/scripts/file_differential.py
```

1,653 native comparisons cover nullability, all five conflict policies, CHECK
branching and NULL behavior, named/duplicate constraints, malformed definitions,
counters, transactions, generated columns, strict/primary-key metadata and files.
Images cover all eight page sizes, three encodings and three auto-vacuum modes
with both table kinds; native checks compare schema text, rows, metadata and
integrity before and after subsequent native writes. Seven Rust tests include
prepared reuse, failure isolation, malformed prefixes, budgets and 72 image
configurations containing both table kinds. A Unix test and nine native file
scenarios cover persistent native readers, commit/reopen and unchanged files on
failed or rolled-back edits.

Foreign keys, triggers, virtual/temporary/attached schemas, table/column RENAME,
general double-quoted-string fallback, schema cookies and the C API remain
unfinished. The date/time and other missing scalar functions are unavailable in
new CHECK expressions. Full planner, prepare-time validation and native diagnostic
compatibility are not established. Existing query materialization and STRICT
journal-planning limits still apply. Passing these cases does not establish every
combination with SQLite's complete SQL surface.

The independent reference is the pinned public SQLite 3.53.4 `alter.c`, `parse.y`
and `expr.c` source and native executable. It is used only for testing.
