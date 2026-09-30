# Memory-safe SQLite rewrite

**Status: incomplete.** The default Cargo workspace now builds an independently
written safe Rust storage core and an experimental in-memory SQL engine. It does
not yet replace SQLite.
The earlier C2Rust translation is preserved in [`legacy/`](legacy/README.md),
outside the default workspace, and is not a dependency of the safe rewrite.

The requested goal is a memory-safe implementation covering the public SQLite
extensions, upstream utilities, and platform adapters. Commercial extensions are
excluded, as requested. Coverage is tracked against the public SQLite **3.53.4**
source archive. No extension, utility, or platform is counted as supported merely
because its source was inventoried or a Rust target compiled.

## What is implemented

The new crate in [`safe/`](safe/README.md) uses `#![forbid(unsafe_code)]`, has
**zero third-party dependencies**, and supports `no_std` with an allocator.

- Safe SQL parsing and execution: table operations, expressions, joins, aggregates,
  column/composite/table constraints, prepared parameters, schema-inspection
  pragmas, and in-memory transactions/savepoints.
- INNER/CROSS/LEFT/RIGHT/FULL joins with ON, USING and NATURAL, merged columns,
  parenthesized groups and correlated scopes; see [join support](safe/JOINS.md).
- Aggregate FILTER and input ORDER BY, including DISTINCT and filtered min/max
  row selection; see [aggregate support](safe/AGGREGATES.md).
- Derived tables, ordinary/recursive CTEs, VALUES queries, and UNION/INTERSECT/EXCEPT;
  see [query execution and remaining differences](safe/QUERIES.md).
- Scalar and row-valued subqueries, EXISTS and IN, including correlated reads in
  queries and data changes; statement-local caches and explicit resource bounds.
- Row comparisons, table-name IN shorthand and multi-column UPDATE/UPSERT
  assignments; see [row-value support](safe/ROW_VALUES.md).
- Stored views and CREATE TABLE AS SELECT, with schema metadata, transactional
  creation/deletion and native database interchange; see [view support](safe/VIEWS.md).
- All five constraint conflict policies, statement overrides, schema ON CONFLICT,
  and persistence of retained FAIL prefixes; see [conflict support](safe/CONFLICTS.md).
- UPSERT with ordered targets, DO NOTHING/DO UPDATE, excluded values, correlated
  expressions and persistence; see [UPSERT support](safe/UPSERT.md).
- Buffered RETURNING rows for INSERT/UPSERT, UPDATE and DELETE, including
  expression subqueries and conflict handling; see [RETURNING](safe/RETURNING.md).
- AUTOINCREMENT with editable/persistent sqlite_sequence state, savepoints and
  rowid-exhaustion rollback; see [sequence behavior](safe/AUTOINCREMENT.md).
- STRICT tables with typed writes, ANY preservation and table_list catalog
  flags; see [type and transaction behavior](safe/STRICT.md).
- WITHOUT ROWID tables with composite primary storage, secondary-key suffixes,
  SQL mutations and native image interchange; see [primary-key storage](safe/WITHOUT_ROWID.md).
- VIRTUAL/STORED generated columns with lazy reads, dependency scheduling,
  constraints and native file layouts; see [generated columns](safe/GENERATED.md).
- Column/expression/partial indexes and automatic constraint indexes, including
  uniqueness, matching UPSERT targets, encoded image writing and schema interchange;
  see [expression and partial indexes](safe/EXPRESSION_INDEXES.md).
- Offline SQL snapshot import/export and an experimental `sqlite-safe-sql` command.
  See the exact supported surface and limitations in [`safe/SQL.md`](safe/SQL.md).
- Checked database headers, all SQLite page sizes, varints, and record codecs.
- Physical rowid-table and index B-tree traversal, including WITHOUT ROWID
  storage, interior index records, overflow chains, and UTF-8/UTF-16 records.
- A bounded in-memory builder for new SQLite-compatible rowid-table database
  images, including auto-vacuum pointer-map layouts. This does not perform SQL
  validation or transactional in-place writes.
- Recovery of immutable database/WAL snapshot pairs to their last valid commit,
  with salts/checksums checked and incomplete or invalid tails ignored.
- Rollback-journal encoding/recovery and an experimental journaled SQL API over a
  storage-adapter trait, with injected failure tests.
- A separate [Unix file adapter](platform/README.md), with native SQLite lock
  contention, real-file recovery and process-interruption tests on macOS ARM64.
- An offline inspection utility with `info`, `schema`, and physical `rows` output.

SQLite's own `integrity_check` accepts generated files at all eight page sizes.
142 core Rust integration tests include 14,000 malformed-input mutations
and 600 simulated commit failure/crash variants. Twenty-one additional adapter tests
exercise real files and concurrent threads.
The separate C oracle validates 956 SQL expression/query/error/snapshot cases and
32 storage scenarios covering file construction, existing database reads, and WAL
recovery, plus 14 index, 11 rollback-journal, 54 encoding/metadata and 30 auto-vacuum
image scenarios. Another 87 constraint/index-interchange and 375 schema-inspection
scenarios pass, together with 275 scoped/recursive/compound-query comparisons and
258 scalar/EXISTS/IN/correlated-subquery comparisons and 246 view/CREATE TABLE AS
SELECT/schema-interchange scenarios.
Another 508 comparisons cover compound column types, nested metadata, stored
value classes and scalar/IN affinities, including integer precision boundaries.
Another 316 scenarios compare conflict policies, errors, counters, transaction state
and encoded images, plus 592 UPSERT, 311 RETURNING, 268 AUTOINCREMENT and 2,608
STRICT/type/catalog scenarios, plus 1,654 WITHOUT ROWID, 1,368 generated-column
and 2,078 expression/partial-index scenarios. Another 579 scenarios compare joins,
merged-column types, correlated outer rows and persisted views, plus 754 scenarios
for parenthesized namespaces, rowids, ON dependencies and encoded images.
Another 1,110 scenarios compare aggregate filters, input ordering, DISTINCT,
result types, bare-column selection, correlated scopes and stored images.
Another 3,446 scenarios compare row predicates, multi-column subqueries and
assignments, NULL/type rules, correlated membership and encoded images.
Incremental-vacuum scheduling and free-page retention remain pending.
The Unix adapter passes 122 native file interchange/locking/SQL scenarios and
210 real interrupted commit/partial-write recovery points. These are not hardware
power-loss tests. The core
compiles for nine targets, including 32-bit, big-endian,
Windows, Linux, Android, iOS, WebAssembly, and embedded targets. **Only macOS ARM64
has been runtime-tested locally.** The published CI baseline also passes core
tests on Linux and Windows, and Unix adapter tests on Linux; see the exact
[CI evidence](safe/STATUS.md#published-ci-evidence). Native WAL fixtures also cover both checksum byte
orders, every byte truncation, and uncommitted/stale/corrupted tails. See
[`safe/STATUS.md`](safe/STATUS.md).

## Build and try the safe implementation

```sh
cargo build --release
cargo test --all-targets
cargo run --bin sqlite-safe-sql < safe/examples/sql_demo.sql
cargo run --example create_fixture -- example.db 4096
cargo run --bin sqlite-safe-inspect -- example.db info
cargo run --bin sqlite-safe-inspect -- example.db schema
cargo run --bin sqlite-safe-inspect -- example.db rows 2
```

The fixture command creates a new file and refuses to overwrite an existing one.
The inspector is for immutable offline copies, not live databases. It rejects
nonempty journal/WAL sidecars unless an explicit immutable WAL snapshot is supplied.
That check does not provide locking; the input must already be a consistent copy.

## Coverage and remaining work

The generated [inventory](safe/coverage/INVENTORY.md) includes **71 public extension
entries**, **95 utility/build-tool files**, **139 compile switches**, and **307
compiler target triples**. Every unfinished item remains marked unfinished.
The JSON inventory also hashes 840 core, extension, and tool source files.

Complete SQL/semantic compatibility, the optimizing query planner, incremental
B-tree mutation, complete VFS/locking behavior, other platform adapters, all public SQL
extensions, C API compatibility, and upstream utility parity are still outstanding. The new
inspector is not a replacement for `sqlite3`, `sqldiff`, `sqlite3_analyzer`, or
`sqlite3_rsync`.

The [architecture and completion criteria](safe/ARCHITECTURE.md) describe the
memory-safety boundary and how implementation, compile checks, runtime tests,
and behavioral compatibility are distinguished. This repository does **not**
yet fulfill the complete requested rewrite.

The legacy port's earlier test counts and broad SQL coverage apply only to that
unsafe port. They are not evidence for the new safe implementation.
