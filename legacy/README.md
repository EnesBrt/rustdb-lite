# SQLite in Rust

A source-level Rust port of the **SQLite 3.53.4 engine**, including its SQL
compiler, query planner, virtual machine, B-trees, pager, transactions, WAL,
Unix VFS, public C API, and the extensions listed below.

The engine is in `src/engine.rs`: approximately 253,000 lines of Rust generated
from the complete configured SQLite amalgamation using C2Rust, with explicit
compatibility repairs. **Cargo builds the engine from Rust. It does not compile,
link, or delegate queries to C SQLite.** Upstream C files are retained as the
auditable migration source and independent test reference.

This is a working **unsafe, mechanically translated port**, not a completed
idiomatic or memory-safe redesign. It currently targets **Apple Silicon macOS
(`aarch64-apple-darwin`)**. Other targets intentionally fail compilation: their
platform ABI and VFS must be regenerated and tested. Passing the included tests
does not establish SQLite's full production reliability. See [PORTING.md](PORTING.md)
and [TESTING.md](TESTING.md) for the exact scope and evidence.

## Build and run

Rustup installs the pinned `nightly-2026-02-19` toolchain when necessary. Nightly
is required for definitions of C variadic functions and opaque extern types.
Normal builds require Rust and the macOS linker, but neither C2Rust nor LLVM's
development libraries.

```sh
cargo build --release
cargo run --release -- :memory: "SELECT sqlite_version(), 6 * 7;"
cargo run --release -- --json example.db "CREATE TABLE IF NOT EXISTS t(id INTEGER PRIMARY KEY, name TEXT); INSERT INTO t(name) VALUES('Rust'); SELECT * FROM t;"
echo 'SELECT 6 * 7;' | cargo run --release -- :memory:
```

The shell accepts `DATABASE [SQL]`, SQL on stdin, `--json`, `--csv`, and
`--headers`. With an interactive terminal it supports multiline SQL and
`.tables`, `.schema`, `.databases`, `.read`, `.mode`, `.headers`, and `.quit`.
It is a new Rust shell; it does **not** reproduce every upstream shell command.
JSON blobs are emitted as `{"$blob":"00FF"}`. Empty result sets are `[]`.
The high-level API rejects invalid UTF-8 TEXT; retrieve `CAST(value AS BLOB)`
for lossless arbitrary bytes. The underlying C API retains SQLite's byte APIs.

## Rust API

```rust
use sqlite_rust::{Connection, Value};

fn main() -> sqlite_rust::Result<()> {
    let mut db = Connection::open_in_memory()?;
    db.execute_batch("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT)")?;
    let tx = db.transaction()?;
    tx.execute("INSERT INTO items(name) VALUES(?)", &[Value::Text("Rust".into())])?;
    tx.commit()?;
    let result = db.query("SELECT id, name FROM items", &[])?;
    println!("{:?}", result.rows);
    Ok(())
}
```

Connections, prepared statements, and transactions have RAII cleanup. A
transaction rolls back on drop. Prepared statements support bound parameters,
stepping, reset, and typed owned results. Connections remain on their creating
thread; independent connections can be used by different threads. The complete
raw API is available under `sqlite_rust::engine` and remains unsafe.

## C API

The release build produces `libsqlite_rust.a` and `libsqlite_rust.dylib`. Use the
retained `upstream/sqlite3.h` or `upstream/sqlite3ext.h` with them. All **362**
exports present in the matching C build were found in the Rust dynamic library.

```sh
clang -Iupstream your_program.c target/release/libsqlite_rust.a -liconv -lm -lpthread -o your_program
```

The linked binary still uses operating-system/libc services, as ordinary native
Rust programs do. It has no dependency on `libsqlite3`.

## Included configuration

Standard SQLite SQL, JSON/JSONB, foreign keys, triggers, views, CTEs, window
functions, generated columns, strict tables, WITHOUT ROWID, backup,
serialization/deserialization, loadable extension support, and the default
Unix VFS are retained. Foreign-key enforcement keeps SQLite's default and can
be enabled with `PRAGMA foreign_keys=ON`.

Enabled optional components include FTS3/4/5, R-tree, Geopoly, session changesets,
preupdate hooks, snapshots, unlock notifications, math functions, percentiles,
STAT4, normalization, column metadata, carray, dbstat, dbpage, bytecode, and
statement virtual tables. The exact 27 preprocessor settings are in
`scripts/config.py` and `upstream/translation.json`.

The target is this configured upstream engine, not every mutually exclusive
SQLite build configuration, platform VFS, or separately distributed extension.
ICU, proprietary extensions, and upstream auxiliary executables are not included.

## Validation and regeneration

```sh
cargo test --all-targets
cargo test --release --all-targets
python3 scripts/build_oracle.py
python3 scripts/differential.py
python3 scripts/check_c_api.py
python3 scripts/process_tests.py
python3 scripts/fuzz_regressions.py
python3 scripts/verify_source.py
```

Only the comparison and upstream test harness scripts compile C. Their binaries
live in ignored `build/`; they are not dependencies of the Rust engine.

To regenerate, install LLVM 18 development tools and CMake, then run
`python3 scripts/regenerate.py`. The script installs C2Rust 0.22.1 into the
project-local `.tools/`, verifies the official archive checksum, translates
with `--fail-on-error`, and applies the documented transformations.

SQLite's source is public domain. The handwritten project code is MIT licensed;
dependency licenses remain with their respective authors.
