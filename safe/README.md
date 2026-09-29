# Safe SQLite core

This crate contains independently written storage components and an experimental
in-memory SQL engine for the requested memory-safe SQLite rewrite. The complete
rewrite is **unfinished**. It uses only
`core`, `alloc`, and optional `std`; `unsafe` code is forbidden in the library,
binaries, tests, and examples. Default Cargo commands select this crate. The
separate [platform crate](../platform/README.md) is another workspace member.

The public scope is pinned to SQLite 3.53.4 for reproducible comparison. See the
[coverage inventory](coverage/INVENTORY.md), [status](STATUS.md), and
[architecture](ARCHITECTURE.md). Commercial products are outside the requested scope.

The SQL API, implemented surface, CLI, and limitations are documented in
[SQL.md](SQL.md). Derived tables, CTEs and compound queries have additional
execution details in [QUERIES.md](QUERIES.md). Stored views and CREATE TABLE AS
SELECT are described in [VIEWS.md](VIEWS.md). Data-change projections and their
cache/error behavior are described in [RETURNING.md](RETURNING.md). The lower-level physical storage API
is shown below. Persistent automatic rowids and sqlite_sequence are documented in
[AUTOINCREMENT.md](AUTOINCREMENT.md). STRICT types and catalog flags are described
in [STRICT.md](STRICT.md). Primary-key SQL/storage support is documented in
[WITHOUT_ROWID.md](WITHOUT_ROWID.md).

## Storage API

```rust
use sqlite_safe::{Database, ImageBuilder, Table, Text, Value};

let mut table = Table::new("items", &["name", "quantity"]);
table.rows.push((1, vec![Value::Text(Text::utf8("Rust")), Value::Integer(42)]));
let mut builder = ImageBuilder::new(4096)?;
builder.add_table(table)?;
let bytes = builder.finish()?;
let database = Database::parse(&bytes)?;
let schema = database.schema()?;
let rows = database.rows(schema[0].root_page)?;
# Ok::<(), sqlite_safe::Error>(())
```

`Database::visit_rows` avoids retaining every decoded row. Traversal uses an
explicit stack, tracks duplicate/cyclic page references, and checks payload,
column, row, page, and depth budgets. It validates cell/freeblock bounds and
overlap. This is not a full database integrity checker: freelists, pointer maps,
cross-root ownership, index collations, and all schema invariants still require
additional validation.

Results represent **physical stored values**. INTEGER PRIMARY KEY aliases remain
NULL in record slots; the actual rowid is separate. WITHOUT ROWID columns are in
their stored primary-key-first order. REAL affinity conversion and defaults for
missing columns are SQL-layer work and are not applied. Invalid text bytes are
preserved, and `Text::to_string` reports invalid UTF encoding instead of losing data.

`ImageBuilder` creates complete new images with increasing signed rowids and
untyped, unconstrained columns. It writes B-tree interior/leaf pages, overflow
chains, and the schema tree. It supports UTF-8, UTF-16LE and UTF-16BE records
selected through `ImageBuilder::encoding`. `ImageBuilder::auto_vacuum` adds
FULL/INCREMENTAL pointer-map layouts; see [AUTOVACUUM.md](AUTOVACUUM.md) for the
distinction between format support and incremental vacuum behavior. It does not implement SQL,
constraint evaluation, updates to existing trees, incremental I/O, durability,
or locking. The SQL layer also uses its internal index writer to emit sorted
index leaf/interior pages, automatic/explicit indexes and view schema entries. The caller
owns the returned byte vector.

`wal::recover` checks an immutable WAL snapshot's header, salts, checksums, and
commit markers. It returns a separate image plus valid/committed-frame counts
and ignored-tail size. It does not acquire locks or read a changing WAL/index.

`journal` adds rollback-journal encoding and snapshot recovery. `pager` and
`sql::JournaledConnection` implement commit ordering over a storage-adapter trait.
The separate platform crate supplies an exclusive Unix file adapter, runtime-tested
on macOS ARM64. See [PAGER.md](PAGER.md) and [platform documentation](../platform/README.md)
for contracts, failure behavior, native/fault evidence, and remaining platform work.

The format implementation follows SQLite's [file format specification](https://www.sqlite.org/fileformat.html).

## Run

From the repository root:

```sh
cargo test --all-targets
cargo test --release --all-targets
cargo clippy --all-targets -- -D warnings
cargo check --lib --no-default-features
cargo run --bin sqlite-safe-inspect -- OFFLINE_COPY.db schema
cargo run --bin sqlite-safe-inspect -- OFFLINE_COPY.db rows 2
cargo run --bin sqlite-safe-inspect -- SNAPSHOT.db schema --wal SNAPSHOT.wal
```

The inspector emits JSON lines. It retains embedded NULs in text. Raw invalid
text and BLOBs are hex encoded. REAL values use IEEE-754 bit strings so inspection
does not discard NaNs or floating-point precision. Output is a storage debugging
format, not the upstream shell's JSON format. The inspector has a 256 MiB input
limit and never writes the source database. Empty zero-byte SQLite files are
not currently accepted by the snapshot parser; use `ImageBuilder` for an empty
initialized image.

For differential validation, the separately built C reference is a test tool:

```sh
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
python3 safe/scripts/check_targets.py
```

The target-check script requires the listed Rust target libraries to be installed
with `rustup target add --toolchain stable`. It does not install them or run target
binaries. The reference C engine is never in this crate's dependency graph.

Rust tests embed small [native WAL fixtures](tests/fixtures/README.md), validated
by the pinned SQLite reference. They exercise both checksum byte orders, every
byte truncation, uncommitted frames, stale salts, and corruption. Regenerating
these fixtures is an explicit maintenance step; normal Rust tests do not run C.
