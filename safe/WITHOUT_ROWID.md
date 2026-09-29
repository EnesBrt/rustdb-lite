# WITHOUT ROWID tables

The safe engine supports `CREATE TABLE ... WITHOUT ROWID` with the implemented
column/composite PRIMARY KEY, UNIQUE, CHECK, NOT NULL, DEFAULT and collation
features. A primary key is required and AUTOINCREMENT is rejected. `STRICT` and
`WITHOUT ROWID` can be combined, separated by a comma, in either order.

```sql
CREATE TABLE items(
  namespace TEXT COLLATE NOCASE,
  id INT,
  content ANY,
  PRIMARY KEY(namespace, id DESC)
) WITHOUT ROWID, STRICT;
INSERT INTO items VALUES('docs', 1, '0001');
INSERT INTO items VALUES('DOCS', 1, 'updated')
  ON CONFLICT(namespace, id) DO UPDATE SET content=excluded.content
  RETURNING *;
```

## Keys and SQL behavior

Every primary-key column is implicitly NOT NULL unless it already has an
explicit NOT NULL policy. `INTEGER PRIMARY KEY` is an ordinary typed key here:
it does not allocate a value for NULL. Defaults on primary-key columns work.
There is no hidden `rowid`, `_rowid_` or `oid`; a real declared column can still
use one of those names. Inserts leave `last_insert_rowid()` unchanged while
`changes()` and `total_changes()` count successful rows normally.

INSERT, UPDATE, DELETE, the five conflict policies, UPSERT, RETURNING,
transactions and savepoints operate on the primary key. The in-memory row map
uses private handles that are never exposed as SQL rowids or written into these
tables' records. Scans sort by the primary key's collations and ASC/DESC flags.
UPDATE captures selected primary keys and looks each key up again before changing
it, including when an earlier REPLACE moved a row into a later selected key.
Selected-key storage and sorting respect SQL resource budgets.

The STRICT type checks and transaction-error behavior described in
[STRICT.md](STRICT.md) also apply. Defaults, views, CTEs, correlated expressions
and CREATE TABLE AS SELECT use the declared column order. A snapshot table made
with CREATE TABLE AS SELECT is an ordinary rowid table.

## Database and index layout

The table's root is an index B-tree. Its records begin with primary-key fields,
followed by the other stored columns in declaration order. VIRTUAL generated
columns are omitted from that payload. Interior index
records contain actual rows. The writer supports overflow records, all eight
page sizes, all three text encodings and auto-vacuum pointer maps.

The automatic primary index appears in index pragmas but has no separate
`sqlite_schema` row or B-tree root. Secondary indexes append the primary-key
columns needed to locate the row; a column is omitted from that suffix only
when its column and collation already match an index key. Repeated primary-key
terms with the same column/collation are coalesced, while distinct collations
retain distinct physical fields. Import restores values to declaration order.

Explicit secondary indexes copy primary-key sort directions. Automatic UNIQUE
indexes preserve the pinned reference's historical ascending suffix layout.
Metadata reports the distinction, including which fields are keys or auxiliary
fields. `PRAGMA index_info(table_name)` and `index_xinfo(table_name)` describe
the primary storage index; `table_list` sets `wr=1`.

The public low-level `ImageBuilder::add_table` still creates ordinary untyped
rowid tables. The SQL layer's internal writer builds WITHOUT ROWID records after
validating their schema and keys. Export and the journaled API rebuild complete
images; incremental B-tree/freelist mutation remains unfinished. Import rejects
a table/index B-tree kind mismatch, NULL primary keys and inconsistent repeated
primary fields, but is not a complete integrity checker for corrupt inputs.

## Verification

```sh
python3 legacy/scripts/build_oracle.py
python3 safe/scripts/without_rowid_differential.py
python3 platform/scripts/file_differential.py
```

The new differential script compares 1,654 SQL, conflict, counter, metadata and
image scenarios against SQLite 3.53.4. It includes fifteen primary-key schemas,
STRICT combinations, quoted custom types, key changes, FAIL prefixes, savepoints,
UPSERT, hidden-name errors, views and native-origin files. Twenty-four larger
image scenarios exercise all eight page sizes, all three encodings, all three
auto-vacuum settings across the matrix, long keys, BLOB overflow, interior records,
forced native index scans and native mutation followed by Rust reimport/export.

Seven Rust integration tests cover prepared bindings, defaults/counters, hidden
names, physical index metadata, key revisitation, UPSERT binding offsets,
malformed declarations, resource rollback and 72 encoded image combinations.
A Unix adapter test and nine native real-file scenarios verify commit/reopen,
retained FAIL prefixes, rollback, native readers and failed-statement isolation.

## Remaining scope

Foreign keys, triggers, expression/partial indexes, virtual
tables, native ABI and the rest of the unfinished SQL surface remain outside this
implementation. No optimizing index access path is added: native SQLite can
choose a different scan order for a filtered update or unordered query. That can
change RETURNING order or which rows precede a FAIL. Materialized-query timing
and resource limits also retain the differences documented in [QUERIES.md](QUERIES.md).

This is evidence for the implemented subset, not completion of the full SQLite,
public-extension, upstream-utility or platform rewrite.

Generated columns are now supported within the implemented expression subset;
see [GENERATED.md](GENERATED.md) for their evaluation, constraints and file layout.
