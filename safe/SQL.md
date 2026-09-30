# Experimental safe SQL engine

The rewrite now executes a defined SQL subset without native SQLite, FFI, unsafe
code, or third-party dependencies. This is an **incomplete in-memory engine**.
The complete SQLite replacement requested by the user is not finished.

## Rust API

```rust
use sqlite_safe::{sql::Connection, Text, Value};

let mut db = Connection::new();
db.execute("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT NOT NULL)", &[])?;
let insert = db.prepare("INSERT INTO items(name) VALUES(:name)")?;
assert_eq!(insert.parameter_index(":name"), Some(1));
db.execute_prepared(&insert, &[Value::Text(Text::utf8("Rust"))])?;
let result = db.execute("SELECT id, upper(name) FROM items WHERE id=?1", &[Value::Integer(1)])?;
assert_eq!(result.rows.len(), 1);
let image = db.to_image(4096)?;
let mut restored = Connection::from_image(&image)?;
assert_eq!(restored.execute("SELECT count(*) FROM items", &[])?.rows,
           vec![vec![Value::Integer(1)]]);
# Ok::<(), sqlite_safe::Error>(())
```

Preparation tokenizes and parses one statement, records parameter numbers, and
owns the result. Name resolution occurs on execution against the current schema;
this is not yet SQLite's prepare-time validation/API contract. Parameters use
one-based indices in `parameter_index`; the execution slice starts at parameter 1.
Omitted bindings are NULL. Named parameters share indices when repeated.

`execute_batch` parses the complete script before executing statements in order.
Consequently, a syntax error anywhere prevents the script from executing. This
is different from `sqlite3_exec`'s incremental preparation. Execution stops on runtime errors. ABORT undoes that statement, FAIL retains its
successful prefix, and ROLLBACK undoes the active transaction; see [conflict
policies](CONFLICTS.md).
The returned results, intermediate rows, expression values, and statement work
have explicit limits. Transaction/savepoint snapshots consume additional memory;
`max_database_bytes` is a logical-data/intermediate-result budget, not an RSS cap.

## Implemented surface

| Area | Current implementation |
| --- | --- |
| Lexing | UTF-8 SQL, comments, quoted identifiers, strings, BLOBs, decimal/hex numbers, numeric underscores, ordinary parameters |
| Expressions | Arithmetic/bit operations, comparisons, NULL logic, IS/IS NOT, BETWEEN, IN lists, CASE, CAST, COLLATE, LIKE, scalar functions |
| Tables | Rowid and WITHOUT ROWID tables; declared-type affinity; column/table PRIMARY KEY, UNIQUE and CHECK constraints (including composite keys), NOT NULL, DEFAULT, collations, constraint-name syntax |
| Generated columns | VIRTUAL/STORED dependencies, lazy reads, typed writes, constraints and physical layouts; [scope](GENERATED.md) |
| Primary-key storage | WITHOUT ROWID index B-trees, composite keys and index suffixes; see [WITHOUT_ROWID.md](WITHOUT_ROWID.md) |
| Views and table snapshots | CREATE/DROP VIEW, optional view column lists, CREATE TABLE AS SELECT/VALUES/WITH; main schema, metadata and image interchange; see [VIEWS.md](VIEWS.md) |
| Indexes | CREATE [UNIQUE] INDEX, DROP INDEX, composite column/expression keys, partial WHERE predicates, ASC/DESC, built-in collations and automatic constraint indexes; [scope](EXPRESSION_INDEXES.md) |
| Data changes | INSERT VALUES/DEFAULT VALUES/SELECT, UPDATE, DELETE; five conflict policies, schema ON CONFLICT and REPLACE; rowid allocation with AUTOINCREMENT |
| Sequences | Editable sqlite_sequence, persisted high-water values, savepoints and exhausted-rowid rollback; see [AUTOINCREMENT.md](AUTOINCREMENT.md) |
| STRICT tables | Six declared types, ANY preservation, primary-key nullability, type errors and transaction behavior; see [STRICT.md](STRICT.md) |
| UPSERT | ON CONFLICT targets, multiple clauses, DO NOTHING/DO UPDATE, excluded values and conditional updates; see [UPSERT.md](UPSERT.md) |
| RETURNING | Buffered row projections on INSERT/UPSERT, UPDATE and DELETE, expression subqueries, aliases and counters; see [RETURNING.md](RETURNING.md) |
| Queries | Projection, stars, aliases, filtering, comma/inner/cross/left/right/full joins with ON, USING and NATURAL ([scope](JOINS.md)), DISTINCT, GROUP BY, HAVING, ORDER BY, NULLS FIRST/LAST, LIMIT/OFFSET |
| Query composition | Derived FROM tables, ordinary/recursive WITH, VALUES queries, UNION [ALL]/INTERSECT/EXCEPT; see [QUERIES.md](QUERIES.md) |
| Expression subqueries | Scalar, EXISTS, single-column IN/NOT IN, correlated columns in queries and data changes; statement-local caching for uncorrelated results |
| Aggregates | count, sum, total, avg, min, max, group_concat, string_agg; single-argument DISTINCT |
| Scalar functions | typeof, length, octet_length, hex, unhex, lower, upper, abs, unicode, char, ifnull, nullif, coalesce, iif/if, instr, replace, trim/ltrim/rtrim, substr/substring, min/max, like, changes, total_changes, last_insert_rowid |
| Collations | BINARY, ASCII NOCASE, RTRIM |
| Transactions | BEGIN/COMMIT/ROLLBACK and nested savepoints over private owned memory |
| Metadata | table_list/table_info/table_xinfo, index_list/index_info/index_xinfo pragmas; user_version, application_id, and read-only auto_vacuum |
| Images | Offline import/export with table and index B-trees, stored views, preserved CREATE SQL, INTEGER PRIMARY KEY slots, text encoding, auto-vacuum mode, application_id and user_version |
| Journaled API | `JournaledConnection<S>` persists autocommit/COMMIT/outer RELEASE through a caller-supplied storage adapter; see [PAGER.md](PAGER.md) |

Queries use scans and bounded nested-loop joins. Grouping, sorting and aggregate
execution are independent Rust implementations; an optimizing query planner and
index access paths are not present. Passing examples establish only the tested
behaviors, not complete compatibility of every expression/feature combination.

## Limits and unsupported behavior

- `Connection` transactions modify owned memory only. The separate journaled API
  can use the [Unix file adapter](../platform/README.md), locally tested on macOS
  with native locking and process-interruption recovery. It retains exclusive
  access for the connection lifetime. Shared readers, WAL checkpoints and complete
  platform coverage remain unfinished. The CLI still uses snapshots.
- No temporary schemas, triggers, window functions,
  virtual tables, foreign keys, ALTER TABLE, ATTACH,
  extension loading, or C ABI.
- Row-value subqueries, the `value IN table_name` shorthand, and aggregates owned
  by an outer query (such as `SELECT (SELECT sum(t.x)) FROM t`) remain unsupported.
  FROM subqueries cannot refer to sibling FROM sources. Scalar subqueries return
  the first row or NULL; EXISTS does not evaluate plain result expressions but
  still computes aggregate inputs. Subqueries are prohibited in DEFAULT/CHECK.
- Derived FROM tables, views and CTEs use bounded materialization. Recursive CTEs use
  working-row queues with UNION cycle suppression, queue ordering, LIMIT/OFFSET,
  and simple outer LIMIT propagation. General streaming/coroutines and query
  flattening are pending. An infinite recursive query with an outer filter or
  join can exhaust budgets even when native SQLite stops after an outer LIMIT.
  AS MATERIALIZED/AS NOT MATERIALIZED both currently materialize; their distinct
  RETURNING cache-sharing behavior is implemented for the tested cases.
- No date/time, JSON, math, formatting, or other functions outside the list above.
  LIKE's infix ESCAPE syntax is pending; `like(pattern,text,escape)` is available.
  Tcl parameter suffixes and general double-quoted-string fallback are pending;
  the fallback is supported in generated-column and index declarations.
- Schema-inspection pragmas support the table/view/index subset above, both argument
  syntaxes, primary-key positions, default SQL, index origin, collation names,
  directions, and auxiliary rowid entries. table_list adds strict flags and an empty
  temp-schema catalog; invalid-view metadata timing limits are in [STRICT.md](STRICT.md).
  Read-only results respect query limits.
  Table-valued pragma functions, SQL access to `sqlite_schema`, and the remaining
  pragmas are pending. Unknown pragmas currently return an unsupported error.
- Composite ordinary-table keys permit distinct NULL entries, matching SQLite's
  legacy rowid-table behavior. Single-column table `PRIMARY KEY(x DESC)` aliases
  rowid when x has type INTEGER; column `x INTEGER PRIMARY KEY DESC` does not.
  Automatic constraints with the same columns/collations share an index regardless
  of direction. Constraint names survive in CREATE SQL, but violation diagnostics
  currently report generic constraint types, not exact native messages/names.
- Unique constraints and explicit unique indexes are enforced by scans in memory.
  Ordinary-table random rowid allocation after i64::MAX is pending, including
  manually setting that rowid on sqlite_sequence before it needs a new entry.
  Exports rebuild table/index trees; there is no incremental index mutation or
  index-based query access. Expression and partial index support is documented
  in [EXPRESSION_INDEXES.md](EXPRESSION_INDEXES.md).
  Import rejects triggers, unsupported index definitions, and other schemas
  that cannot be parsed. Index metadata is preserved and entries are rebuilt from
  table values; import is not a complete physical index integrity check.
- SQL values use UTF-8 internally, while imported database encodings are preserved
  during export. BINARY comparisons/index ordering, byte casts, hex(text), and
  octet_length account for that encoding. Some SQL text operations reject invalid
  UTF bytes; byte-preserving physical inspection remains a separate API.
- Auto-vacuum pointer-map layouts are preserved by snapshot export and journaled
  writes. Images are rebuilt compactly: incremental-vacuum free-page retention,
  setting the mode through SQL, and SQL vacuum operations remain unimplemented.
  See [AUTOVACUUM.md](AUTOVACUUM.md). Journaled connections still reject reserved-byte
  extension layouts; snapshot import can inspect understood tables in such files.
- Exhausting the largest rowid currently reports unsupported random allocation.
- Default budgets: SQL 1 MiB, 32,768 tokens, expression depth 64, 100,000 rows,
  16 MiB/value, 64 MiB/database or result, 2,000,000 execution steps, 32,766
  parameters, and 16 savepoints. Additional implementation caps are 1,024-byte
  names, 1,000 function arguments, 2,000 table/result columns, 64 joined tables,
  1,024 tables/views combined, 2,000 indexes per table, 2,000 CTEs per WITH, 500 compound SELECT
  terms, and query nesting at most 32 (also bounded by `max_expr_depth`).
  Exported table/index data also shares
  the database byte budget. These differ from SQLite's configurable limits.

Unsupported syntax is rejected. It is never delegated to the retained unsafe
engine. The public-extension and upstream-utility inventories remain unfinished.

## Command line

From the repository root:

```sh
cargo run --bin sqlite-safe-sql -- --help
cargo run --bin sqlite-safe-sql < safe/examples/sql_demo.sql
cargo run --bin sqlite-safe-sql -- --save new.db < statements.sql
cargo run --bin sqlite-safe-sql -- --load offline.db < queries.sql
```

SQL comes from standard input. Output is one JSON object per statement, with
column names, change counts, and rows. Values carry storage-type tags; REAL values
are exact IEEE-754 bit strings, text/BLOB values are hex. This is an experimental
command, not upstream shell compatibility. `--save` refuses existing paths,
requires the transaction to have ended, and preserves the loaded page size
(4,096 bytes for a new database). `--load` requires a consistent offline
copy and rejects nonempty journal/WAL sidecars; this check does not acquire locks.

## Validation

`cargo test --test sql` covers expression/parameter semantics, affinities,
constraints, statement rollback, savepoints, joins/aggregates, snapshot import and
export, bounded execution, and 5,000 deterministic malformed-SQL mutations.
`python3 safe/scripts/sql_differential.py` runs 956 expression/query/error/snapshot
cases using the separately built SQLite 3.53.4 reference. Comparisons preserve
runtime types, exact floating-point bits, embedded NUL text, and BLOB bytes.
The suite includes large-integer aggregate precision and floating-point text
conversion cases. These are additional tests for the new safe engine; no legacy
engine test counts are reused.

Index tests additionally cover composite uniqueness, NULL keys, collations,
namespace errors, transactional DDL, 240 successive tree sizes with overflow,
and automatic-index round trips. `scripts/index_differential.py` checks 14 native
interchange scenarios, including forced native index scans and native mutation
of Rust-built trees across all eight page sizes.

`scripts/constraint_differential.py` adds 87 table-constraint/query/error/index
interchange cases, including native and Rust origins, rowid aliasing, nullable
composite keys, duplicate constraint indexes and auto-vacuum/UTF-16 schemas.
`scripts/pragma_differential.py` adds 375 schema-inspection comparisons across
UTF-8/UTF-16 databases, defaults, type spelling, quoted names, key order, collations,
index origins and missing objects. Both use the pinned SQLite 3.53.4 oracle.

`scripts/encoding_differential.py` adds 54 SQL/metadata/index scenarios across
UTF-8, UTF-16LE and UTF-16BE, including encoding-sensitive order, casts, scalar
functions and preservation of application IDs during updates.

`cargo test --test queries` covers query scopes, compounds, recursive work queues,
prepared bindings, transaction rollback, malformed prefixes and resource limits.
`scripts/query_differential.py` adds 275 native comparisons, including all three
database encodings, recursive ordering/limits, cycle suppression and CTE inserts.
These tests do not establish full subquery/planner compatibility; see [QUERIES.md](QUERIES.md).

`cargo test --test subqueries` and `scripts/subquery_differential.py` cover scalar,
EXISTS, IN and correlated queries, caching, rollback and resource limits. The
differential suite passes 258 scenarios against SQLite 3.53.4. Persisted correlated
updates and failed subquery inserts also have real-file platform tests.

`cargo test --test views` and `scripts/view_differential.py` cover stored view
dependencies, isolated scopes, metadata, CREATE TABLE AS SELECT, transactional
DDL, resource limits and native image interchange. The differential suite passes
246 scenarios; file tests also verify visibility to a persistent native peer.

`cargo test --test query_types` and `scripts/type_differential.py` cover compound
column type inference, nested metadata, numeric preservation in new tables,
collations in VALUES compounds and scalar/IN affinity rules. The differential
suite passes 508 scenarios, including a 21-by-21 expression matrix and exact
storage classes and value bytes/bits before and after native file interchange.

Constraint policies and their persistence/error behavior are documented in
[CONFLICTS.md](CONFLICTS.md). The statement probe compares 316 policy, counter,
transaction and image-interchange scenarios against SQLite 3.53.4. A remaining
metadata difference is that an unaliased hidden rowid projection is named `rowid`
even when native SQLite names it after an INTEGER PRIMARY KEY alias; explicit
result aliases avoid that difference.
