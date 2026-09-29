# Views and CREATE TABLE AS SELECT

The safe SQL engine supports stored views and table creation from query results
in the `main` schema. Definitions, execution and image serialization use Rust
code under `forbid(unsafe_code)`. This is part of the experimental SQL subset;
the full SQLite rewrite remains unfinished.

```sql
CREATE TABLE readings(id INTEGER PRIMARY KEY, value REAL);
INSERT INTO readings VALUES(1,2.5),(2,3.5);
CREATE VIEW recent(sample, value) AS SELECT id, value FROM readings WHERE id>1;
CREATE TABLE snapshot AS SELECT * FROM recent;
UPDATE readings SET value=4.5 WHERE id=2;
SELECT value FROM recent;   -- 4.5: the definition reads current data
SELECT value FROM snapshot; -- 3.5: an ordinary table with copied results
```

## Stored views

`CREATE VIEW [IF NOT EXISTS] name [(columns)] AS query` stores a query definition.
`DROP VIEW [IF EXISTS] name` removes it. Views share the table/index namespace;
the wrong DROP object type is an error even with IF EXISTS. View definitions
can use the currently supported SELECT, VALUES, CTE and compound query syntax.
Parameters are prohibited in stored definitions.

Dependencies resolve when a view is read or its column metadata is inspected.
A view may be created before its source table. Dropping/recreating that table
changes the columns and values exposed by a subsequent view execution. Missing
dependencies and circular view chains fail on use. Invalid result widths are
also deferred: an explicit column list with the wrong length fails when read.
As in native SQLite, table_info/table_xinfo can still report those explicit names
with empty types if the underlying query otherwise resolves successfully.

Stored definitions cannot see caller CTE names or caller correlated row values.
Their CTE and expression caches are isolated for the same reason. Within a
definition, ordinary query scoping and correlated subqueries work as documented
in [QUERIES.md](QUERIES.md). `main.name` can bypass a caller CTE that shadows a
view. Duplicate exposed names receive numeric suffixes. Views have no hidden
rowid and are read-only; INSERT/UPDATE/DELETE and indexes on views are rejected.

Column metadata preserves source declared types where SQLite does, while CAST
and other affinity-bearing expressions use canonical types. Views have no
primary key, defaults or NOT NULL constraints. Query column affinity and
collation are retained for comparisons against view columns.

Compound definitions merge column affinities using the possible result types of
their expressions. A numeric column combined with a text literal, for example,
has BLOB affinity, so creating a table from it preserves the text value. Numeric
CAST compounds use SQLite's NUM type while preserving existing integer and real
values. Nested views and derived queries follow the final compound projection
for declared type spelling, which can differ from the view's own metadata.

## Table creation from a query

`CREATE TABLE [IF NOT EXISTS] name AS query` evaluates the query and creates an
ordinary rowid table. Rows receive consecutive rowids starting at 1 in result
order. The column names come from the results, with duplicate names made unique.
Types follow expression affinity: TEXT, INT, NUM, REAL or an empty type. The new
columns use BINARY collation and have no constraints or defaults. Source indexes
and primary-key behavior are not copied.

Stored CREATE SQL uses the canonical column definitions, identifier quoting and
line wrapping checked against the pinned native reference. CREATE TABLE AS
SELECT does not change the connection's changes, total_changes or
last_insert_rowid counters. Failures restore the statement's schema and data;
both table creation and view DDL participate in transactions and savepoints.

## Persistence, limits and remaining work

Views are sqlite_schema records with rootpage zero. They allocate no B-tree root
and are included in UTF-8/UTF-16 images and all three auto-vacuum layouts. Import
checks the stored object name, table name, rootpage and CREATE statement identity.
Tables made from queries reopen as ordinary tables. File-backed connections use
the existing rollback-journal commit protocol for these schema changes.

The engine supports only the main schema. TEMP/TEMPORARY views, attached
databases, INSTEAD OF triggers and broader trigger support remain unimplemented.
Definitions must use the implemented SQL subset. Views use bounded materialized
execution; flattening, streaming and complete native evaluation/error timing
remain pending. View dependency depth shares the query depth limit. Tables and
views together are capped at 1,024, and definitions count toward the logical
database byte budget. Other SQL, parser and result limits still apply.

## Validation

Six Rust integration tests cover deferred dependencies, scope/cache isolation,
types and names, atomic errors, rollback, malformed prefixes, corrupt schema
identity and depth/row limits. Round trips cover 72 combinations of text encoding,
page size and auto-vacuum mode. One additional platform test covers persisted
views and snapshots and unchanged files after failed statements.

`python3 safe/scripts/view_differential.py` passes 246 scenarios against SQLite
3.53.4, comparing query values, metadata, exact stored schema text, error outcomes
and native integrity checks. The platform differential suite also verifies view
and table changes through a persistent native connection. These tests establish
the tested subset; they do not establish full view/planner compatibility or any
public-extension or upstream-utility completion.

`scripts/type_differential.py` adds 508 compound-type comparisons, including
nested views and scalar metadata, VALUES and recursive sources, exact stored
value classes and schema text. Four Rust tests cover these type rules and
inference from branches that produce no rows.
