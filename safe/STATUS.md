# Current implementation status

The full requested rewrite is **not complete**. The default build is now the safe
storage core and experimental in-memory SQL engine. The unsafe translated engine
is a separate `legacy/` project.

| Component | Actual state |
| --- | --- |
| Memory-safety policy | Unsafe code forbidden in both crates; core has no dependencies; platform uses trusted rustix OS APIs |
| Portability foundation | `no_std + alloc`; nine target compile checks |
| Varints and record codecs | Implemented and tested |
| Header/page parsing | Implemented with validation and explicit limits |
| Table/index B-tree reads | Implemented, including overflow and index interior records |
| New database images | Rowid/WITHOUT ROWID tables and column/expression/partial indexes, including interior/overflow records and auto-vacuum pointer maps |
| WAL | Offline checksum/salt/commit recovery only |
| SQL tokenizer/parser, expressions, table/query execution | Partial implementation; see [SQL.md](SQL.md) |
| In-memory transactions/savepoints and conflict policies | ABORT/FAIL/ROLLBACK/IGNORE/REPLACE, statement overrides and schema policies; [scope](CONFLICTS.md) |
| UPSERT | Ordered rowid/unique targets, DO NOTHING/DO UPDATE, excluded values, conditional/correlated updates; [scope](UPSERT.md) |
| RETURNING | Buffered INSERT/UPSERT/UPDATE/DELETE projections, subqueries, counters and conflict behavior; [scope](RETURNING.md) |
| AUTOINCREMENT | Persisted/editable sqlite_sequence, conflict timing, savepoints and FULL rollback; [scope](AUTOINCREMENT.md) |
| STRICT tables | Six declared types, ANY preservation, datatype errors, catalog flags and persisted transaction prefixes; [scope](STRICT.md) |
| Generated columns | VIRTUAL/STORED dependencies, lazy reads, typed writes, constraints and physical layouts; [scope](GENERATED.md) |
| WITHOUT ROWID tables | Primary index storage, secondary suffixes, key-based SQL mutations, metadata and encoded interchange; [scope](WITHOUT_ROWID.md) |
| Query optimizer, complete schema/SQL semantics | Not implemented |
| Explicit/automatic index schemas and uniqueness | Column/expression keys and partial predicates; fresh-image rebuilds; [scope](EXPRESSION_INDEXES.md) |
| ALTER TABLE | Column ADD/DROP ([scope](ALTER_TABLE.md)), SET/DROP NOT NULL and named CHECK edits ([scope](ALTER_CONSTRAINTS.md)), table RENAME ([scope](RENAME.md)) and column RENAME ([scope](RENAME_COLUMN.md)); schema validation and transactional persistence |
| Table constraints and schema inspection | Composite PRIMARY KEY/UNIQUE, table CHECK, named-constraint syntax, six inspection pragmas |
| Joins | INNER/CROSS/LEFT/RIGHT/FULL, ON/USING/NATURAL, parenthesized namespaces, merged values and correlated/forward ON scopes; [scope](JOINS.md) |
| Aggregate modifiers | FILTER, input ORDER BY, DISTINCT and min/max bare-column selection; [scope](AGGREGATES.md) |
| Derived tables, CTEs and compound queries | Implemented with bounded materialization and recursive work queues; [limits and differences](QUERIES.md) |
| Expression subqueries | Scalar and row-valued, EXISTS, IN, correlated reads and DML; outer-owned aggregates pending |
| Row values | Comparisons, BETWEEN/CASE/IN, table-name membership and multi-column UPDATE/UPSERT assignments; [scope](ROW_VALUES.md) |
| Pattern matching | Iterative LIKE/ESCAPE/GLOB, bracket sets, encoded BLOB conversion and persisted schema expressions; [scope](PATTERNS.md) |
| Views and CREATE TABLE AS SELECT | Main-schema views, lazy dependency resolution, metadata and native image interchange; [scope and limits](VIEWS.md) |
| Rollback journal codec and commit protocol | Implemented over a storage trait; simulated failure tests; see [PAGER.md](PAGER.md) |
| Journaled SQL API | Autocommit/transaction persistence through caller-supplied adapter |
| Unix file adapter | Exclusive POSIX locking, bounded I/O, fsync/fullfsync, journal lifecycle; macOS ARM64 runtime tested |
| Other adapters, shared-reader VFS behavior, WAL index/checkpoint | Not implemented |
| Physical power-loss durability on deployment hardware | Not established |
| Incremental B-tree/freelist mutation, incremental-vacuum scheduling | Not implemented; pointer maps emitted for fresh images |
| Public optional SQL extensions | 71 inventoried entries; not implemented |
| Upstream utilities/build tools | 95 inventoried files; not implemented |
| C ABI and other bindings | Not implemented |
| Production compatibility and reliability | Not established |

## Verified locally, 2026-09-30

- 181 core Rust integration tests pass in debug and release builds, including
  3,000 deterministic database mutations, 3,000 mutations of valid native WAL
  seeds, 5,000 malformed-SQL mutations, and 3,000 journal mutations, with bounded
  budgets and panic detection.
- 956 SQL expression/query/error/snapshot cases validated using SQLite 3.53.4.
  SQL tests include exact value types/bits/bytes, joins, aggregates, constraints,
  in-memory transactions, and schema-preserving snapshot interchange. This is
  coverage of the implemented subset, not full SQL compatibility.
- Database-image construction at all eight page sizes (512 through 65,536),
  checked by SQLite 3.53.4 `PRAGMA integrity_check`, schema reads, and row comparison.
- Thirty-two differential scenarios: eight file-construction cases; eighteen
  reader cases spanning three page sizes, all three text encodings, auto-vacuum,
  table/index/WITHOUT ROWID trees, freeblocks and overflow; six WAL/sidecar cases.
- The WAL producer in those tests is Python's native SQLite 3.53.1. It exercises
  full commits, a truncated final frame, a missing final frame, a checksum-damaged
  final frame, garbage tails, and refusal of an implicit live WAL main file.
- Embedded native WAL fixtures additionally exercise both checksum byte orders,
  all 4,354 byte truncations across two fixture variants, valid uncommitted tails,
  stale salts, damaged checksums, header errors, and frame/image limits. The
  committed, big-endian, and uncommitted fixture variants were independently
  accepted by SQLite 3.53.4 and returned the expected integrity and query results.
  Fixture provenance and hashes are recorded in `tests/fixtures/`.
- Fourteen index interchange scenarios: all eight writer page sizes with forced
  native index scans, native mutation/rebalancing, Rust reimport/export, three
  native-origin encoding cases, and three schema-name regressions. SQLite
  integrity checks pass.
- Eleven rollback-journal scenarios: native recovery of Rust journals at eight
  page sizes, and byte-exact recovery of native spill journals at three sizes.
- 600 simulated commit failure/crash variants cover growth, truncation, empty
  originals, partial writes, early persistence, and failures before/after barriers.
  Interrupted recovery, exclusive-lock lifetime, and journaled SQL transaction
  reopen tests also pass. These are adapter-contract simulations, not
  OS/power-loss tests.
- Twenty-seven platform Rust tests pass in debug/release, covering real files, recovery,
  path/sidecar guards, lock lifetime, CTE/subquery data changes, views, CREATE TABLE
  AS SELECT and an eight-thread contention race.
- 176 native file interchange/lock/SQL scenarios pass on macOS ARM64. They include
  persistent native connections across updates of all three text encodings and
  updates of both auto-vacuum modes. 210 real process-interruption/partial-write
  recovery points cover ordinary and pointer-map files and match native SQLite.
- Fifty-four encoding-sensitive SQL/metadata/index scenarios pass against the
  pinned reference, including byte casts and preserving application IDs.
- Thirty auto-vacuum image scenarios pass native integrity checks, index scans,
  root relocation, and native vacuum operations. Mode-preserving Rust writes
  rebuild compact images; incremental-vacuum scheduling remains pending. See
  [AUTOVACUUM.md](AUTOVACUUM.md).
- Eighty-seven table-constraint/query/error/index-interchange scenarios pass, including
  composite nullable keys, rowid aliasing, CHECK constraints and index deduplication.
- 375 schema-inspection pragma scenarios pass, including UTF-16 origins, defaults,
  declared types, index/key metadata, collations and quoted object names.
- 275 derived/ordinary/recursive CTE/compound-query scenarios pass, including scoped
  names, NULL/numeric/collation equality, queue ordering, limits, data changes and
  UTF-16 source images. Five Rust query tests additionally cover prepared binding
  reuse, rollback, malformed prefixes, and execution/nesting/materialization limits.
  General streaming execution remains pending.
- 258 scalar/EXISTS/IN/correlated-subquery scenarios pass, including NULL/empty
  results, affinity/collation rules, lazy expression branches, CTE scopes, data
  changes and UTF-16 images. Five additional Rust tests cover rollback, bindings,
  caches, malformed prefixes and resource limits. Aggregate ownership across
  query scopes remains unsupported. Materialization can
  still differ from native evaluation/error timing; see [QUERIES.md](QUERIES.md).
- 246 view/CREATE TABLE AS SELECT scenarios pass, including deferred dependencies,
  column metadata, canonical schema text, namespace errors, all three encodings,
  all eight page sizes and ordinary/FULL/INCREMENTAL image layouts. Six Rust view
  tests cover scope isolation, rollback, malformed definitions, resource limits
  and 72 combinations of encoding, page size and auto-vacuum mode. A platform
  test covers persisted view schema and unchanged files after failed statements.
  Temporary schemas, triggers and view flattening remain pending; see [VIEWS.md](VIEWS.md).
- 508 compound-type/schema/value/scalar scenarios pass, including a 21-by-21
  expression matrix, nested views, recursive/VALUES sources, precise INTEGER/REAL
  persistence and large-integer IN versus scalar comparisons. Four Rust tests
  cover these rules and inference from branches that return no rows.
- 316 conflict-policy/error/state/counter/image scenarios pass against SQLite
  3.53.4. Six Rust policy tests cover defaults, precedence, transactions, counters,
  malformed prefixes and limits. Pager and Unix tests additionally cover retained
  FAIL prefixes, transaction rollback and errors during persistence. This does not
  implement triggers or foreign keys; see [CONFLICTS.md](CONFLICTS.md).
- 592 UPSERT target/action/error/counter/image comparisons pass against SQLite
  3.53.4. Five Rust tests cover bindings, correlated scopes, precedence, rollback,
  resource errors and encoded roundtrips; a platform test and nine native
  real-file cases verify persisted updates and failed-statement isolation. See
  [UPSERT.md](UPSERT.md) for the remaining SQL limitations.
- 311 RETURNING row/metadata/cache/conflict/error/counter comparisons pass against
  SQLite 3.53.4. Six Rust tests cover bindings, empty metadata, subqueries,
  materialized CTEs, rollback, limits and encoded roundtrips. A platform test and
  nine native file cases cover persistence and error isolation; see [RETURNING.md](RETURNING.md).
- 268 AUTOINCREMENT/sequence/conflict/counter/transaction/image scenarios pass
  against SQLite 3.53.4. Six Rust tests cover bindings, allocation, sequence edits,
  rollback, limits and snapshots. A platform test and nine native file cases cover
  persisted high-water values, FAIL prefixes and exhausted-rowid rollback. Native
  image checks include Rust-created and empty system tables and four corrupted
  schema variants; see [AUTOINCREMENT.md](AUTOINCREMENT.md).
- 2,608 STRICT declaration/value/conflict/transaction/catalog/image scenarios pass
  against SQLite 3.53.4. Eight Rust tests cover bindings, nullability, datatype
  prefix retention, budgets, catalog flags and 72 image combinations. A platform
  test and nine native file cases verify commit/reopen and unchanged files after
  aborted statements. Mixed invalid-view catalog metadata remains a known
  difference; see [STRICT.md](STRICT.md).
- 1,654 WITHOUT ROWID SQL/key/conflict/counter/metadata/image scenarios pass
  against SQLite 3.53.4, including long-key/overflow/interior records and native
  rebalancing. Seven Rust tests include 72 encoding/page-size/vacuum combinations;
  a Unix test and nine native file cases cover persisted key changes, FAIL prefixes,
  rollback and failed writes. See [WITHOUT_ROWID.md](WITHOUT_ROWID.md).
- 1,368 generated-column declaration/query/conflict/counter/metadata/image
  scenarios pass against SQLite 3.53.4. Seven Rust tests cover dependency scheduling,
  bindings, constraints, lazy/correlated reads, metadata, STRICT prefixes, budgets
  and 72 image combinations with both table kinds. A Unix test and nine native
  file cases cover persisted generated values and indexes, FAIL prefixes, rollback
  and failed writes; see [GENERATED.md](GENERATED.md).
- 2,078 expression/partial-index declaration, membership, constraint, UPSERT,
  STRICT-error, metadata and image scenarios pass against SQLite 3.53.4. Seven Rust
  tests cover atomic errors, bindings, lexical target matching, physical metadata,
  resource limits and 72 image combinations. Native checks additionally cover
  144 small and 24 larger image combinations with overflow/interior records,
  forced native index scans and native mutations. A Unix test and nine native
  file cases verify commit/reopen, membership changes, rollback and FAIL prefixes;
  see [EXPRESSION_INDEXES.md](EXPRESSION_INDEXES.md).
- 579 join/value/type/collation/correlation/error/image scenarios pass against
  SQLite 3.53.4. Six Rust tests cover qualified and merged values, empty sides,
  generated NULL extension, prepared bindings, chained joins, views, rollback,
  malformed prefixes, row limits and 72 image combinations. A Unix test and nine
  native file cases verify persisted views and INSERT SELECT, failed writes and
  rollback; see [JOINS.md](JOINS.md).
- 754 join-scope/name/rowid/ON-dependency/image scenarios pass against SQLite
  3.53.4. Six additional Rust tests cover parenthesized association, group and
  original aliases, merged/physical columns, result metadata, bindings, recursive
  singleton groups, correlated forward references, malformed prefixes, resource
  limits and 72 image combinations. A Unix test and nine native file cases verify
  grouped-view persistence, INSERT SELECT, rollback and failed writes. Group
  materialization and planner-dependent evaluation remain bounded differences;
  see [JOINS.md](JOINS.md).
- 1,110 aggregate FILTER/ORDER BY/DISTINCT/type/scope/error/image scenarios pass
  against SQLite 3.53.4. Six Rust tests cover prepared bindings, lazy filters,
  distinct collation/type representatives, correlated scopes, bare min/max rows,
  malformed prefixes, memory limits and 72 image combinations. A Unix test and
  nine native file cases verify persisted aggregate views, INSERT SELECT,
  rollback and failed writes. Materialized execution and planner-dependent
  evaluation limits remain; see [AGGREGATES.md](AGGREGATES.md).
- 3,446 row-comparison/membership/assignment/type/scope/error/image scenarios pass
  against SQLite 3.53.4, including two- and three-column NULL matrices. Six Rust
  tests cover prepared bindings, lazy comparisons, shared subquery assignments,
  repeated targets, rollback, malformed/deep expressions, width/memory limits and
  72 image configurations. A Unix test and nine native file cases verify persisted
  tuple updates, generated values, membership views and failed-write isolation.
  Parser construction is split to preserve bounded errors on deeply nested
  expressions without increasing thread stack sizes; see [ROW_VALUES.md](ROW_VALUES.md).
- 9,607 LIKE/ESCAPE/GLOB value/error/schema/image scenarios pass against SQLite
  3.53.4, including random patterns, malformed bytes, Unicode, precedence and
  the pattern-length boundary. Six Rust tests cover prepared bindings, lazy
  branches, iterative wildcard matching, deep syntax, bounded errors, rollback
  and 72 image configurations. A Unix test and nine native file cases verify
  persisted views, generated values, partial indexes and failed-write isolation.
  See [PATTERNS.md](PATTERNS.md) for remaining configuration and planner limits.
- 2,943 ALTER ADD/DROP column/default/dependency/state/image scenarios pass
  against SQLite 3.53.4. Seven Rust tests cover prepared reuse, generated cycles,
  stored-value preservation, rollback, malformed prefixes, limits and 72 image
  configurations. A Unix test and nine native file cases verify persistent readers,
  commit/reopen and unchanged files after rejected edits. The native statement
  harness observes metadata after possible step-time reprepare; see
  [ALTER_TABLE.md](ALTER_TABLE.md).
- 695 additional DROP literal/dependency/transaction/schema/image comparisons
  pass against SQLite 3.53.4. Six Rust tests cover schema-wide quote normalization,
  CHECK dependencies, rowid/boolean rebinding, subsequent name capture, prepared
  reuse, stored values, counters, rollback of unrelated objects, bounded work and
  72 image configurations.
  A Unix test and nine native file cases verify persistent peers and unchanged
  files after errors or rollback; see [ALTER_TABLE.md](ALTER_TABLE.md).
- 1,653 ALTER constraint/nullability/policy/schema/image scenarios pass against
  SQLite 3.53.4. Seven Rust tests cover CHECK branch/NULL behavior, names and
  preserved comments, prepared reuse, counters, rollback, resource limits and
  72 image configurations with both table kinds. A Unix test and nine native
  file cases cover persistent peers, commit/reopen and failed-edit isolation.
  General schema/SQL and platform limits remain; see
  [ALTER_CONSTRAINTS.md](ALTER_CONSTRAINTS.md).
- 514 table rename/scope/metadata/transaction/image scenarios pass against SQLite
  3.53.4. Six Rust tests cover preserved values/indexes/sequences, bound references,
  comments, prepared reuse, rollback, resource limits and 72 image configurations.
  A Unix test and nine native file cases cover persistent peers and failed-edit
  isolation; see [RENAME.md](RENAME.md).
- 1,035 column rename/quote/scope/metadata/transaction/image comparisons pass
  against SQLite 3.53.4. Seven Rust tests cover bound references, exact schema text,
  declared/rowid result labels, literal normalization, prepared reuse, counters,
  rollback, bounded work and 72 image configurations containing both table kinds.
  A Unix test and nine native file cases cover persistent peers, commit/reopen and
  failed-edit isolation; see [RENAME_COLUMN.md](RENAME_COLUMN.md).
- The separate Unix adapter compiles for macOS ARM64, Linux x86-64/i686/s390x,
  Android ARM64 and iOS ARM64. Only the macOS adapter has runtime evidence here.
- Floating-point comparisons use exact IEEE-754 bit patterns. Text comparisons
  use the oracle's hex bytes and the database encoding, preserving embedded NULs
  that the reference shell's normal JSON text output truncates.
- Stable Rust 1.93.1 compiles the library with and without `std`; Clippy passes
  with warnings denied. Formatting is checked with rustfmt.
- Rust 1.85.1 also passes native all-target and `no_std` library compile checks.
  Both SQL API examples pass as Rust documentation tests; the platform example
  also compiles as a documentation test.
- `cargo tree -p sqlite-safe-core` contains only the core. The platform crate
  adds pinned rustix and OS-binding dependencies, reviewed in
  [platform/BOUNDARY.md](../platform/BOUNDARY.md).

| Target | Core compile check | Runtime test here |
| --- | --- | --- |
| aarch64-apple-darwin | Passed | Passed |
| x86_64-unknown-linux-gnu | Passed | Not run |
| i686-unknown-linux-gnu | Passed | Not run |
| s390x-unknown-linux-gnu | Passed | Not run |
| x86_64-pc-windows-gnu | Passed | Not run |
| wasm32-unknown-unknown | Passed | Not run |
| aarch64-linux-android | Passed | Not run |
| aarch64-apple-ios | Passed | Not run |
| thumbv7em-none-eabi | Passed | Not run |

Machine-readable results are in [coverage/compile-checks.json](coverage/compile-checks.json).
The published baseline has also passed GitHub Actions; see the evidence below.
These compile checks do not establish SQL, filesystem, or utility support.

## Reproduce from the repository root

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo test --workspace --release --all-targets
cargo check --lib --no-default-features
python3 legacy/scripts/build_oracle.py
python3 safe/scripts/differential.py
python3 safe/scripts/sql_differential.py
python3 safe/scripts/index_differential.py
python3 safe/scripts/journal_differential.py
python3 safe/scripts/encoding_differential.py
python3 safe/scripts/autovacuum_differential.py
python3 safe/scripts/constraint_differential.py
python3 safe/scripts/pragma_differential.py
python3 safe/scripts/query_differential.py
python3 safe/scripts/subquery_differential.py
python3 safe/scripts/view_differential.py
python3 safe/scripts/type_differential.py
python3 safe/scripts/conflict_differential.py
python3 safe/scripts/upsert_differential.py
python3 safe/scripts/returning_differential.py
python3 safe/scripts/sequence_differential.py
python3 safe/scripts/strict_differential.py
python3 safe/scripts/without_rowid_differential.py
python3 safe/scripts/generated_differential.py
python3 safe/scripts/expression_index_differential.py
python3 safe/scripts/join_differential.py
python3 safe/scripts/join_scope_differential.py
python3 safe/scripts/aggregate_differential.py
python3 safe/scripts/row_value_differential.py
python3 safe/scripts/pattern_differential.py
python3 safe/scripts/alter_differential.py
python3 safe/scripts/drop_quote_differential.py
python3 safe/scripts/alter_constraint_differential.py
python3 safe/scripts/rename_differential.py
python3 safe/scripts/rename_column_differential.py
python3 safe/scripts/check_targets.py
python3 platform/scripts/file_differential.py
python3 platform/scripts/check_targets.py
```

The reference binaries and downloaded test sources are under ignored
`legacy/build/`. They are test inputs only. Current safe validation logs are in
ignored `build/safe-*.log`. The source inventory can be regenerated with
`python3 safe/scripts/inventory.py` after fetching the pinned source archive with
`python3 legacy/scripts/fetch_test_sources.py`.

## Published CI evidence

[GitHub Actions run 36710140469](https://github.com/EnesBrt/rustdb-lite/actions/runs/36710140469)
passed all 17 jobs for commit `d80fe0a` on 2026-09-30. That baseline includes
175 core and 26 Unix adapter Rust integration tests, including ALTER ADD/DROP,
constraint edits, table/column renames and quote handling. Core debug/release/doc
tests passed on Ubuntu 24.04, macOS 14 ARM64 and Windows 2022; Unix adapter
debug/release/doc and native file comparisons passed on Ubuntu and macOS. Native
differential, minimum-Rust and cross-compilation jobs also passed.

These results establish execution of the tested subset on those runners, not
complete platform or durability support. Other target runtime tests remain
pending. New DROP quote-normalization evidence is recorded above; the linked
baseline does not establish this newer change's cross-platform behavior.
