# Validation report

Validated on Apple Silicon macOS on 2026-09-29 using the pinned Rust nightly
and the exact SQLite 3.53.4 C reference. These results apply to the generated
source checksum in `upstream/translation.json` and the configuration in
`scripts/config.py`.

| Check | Observed result |
| --- | --- |
| Debug build and Rust integration tests | 10 passed |
| Optimized build and Rust integration tests | 10 passed |
| AddressSanitizer Rust integration tests | 10 passed; no sanitizer report |
| C/Rust differential scenarios | 649 passed |
| Separate-process C/Rust locking and crash recovery | 8 passed |
| Public C-ABI client, run against each engine | Both passed |
| Upstream C symbol comparison | All 362 configured exports present |
| Rust dynamic library undefined symbols | No SQLite imports |
| Upstream fuzz corpora 1–8 | 46,391 cases; zero harness-reported errors |
| Source provenance and absence of translation stubs | Passed |
| Handwritten Rust formatting | `cargo fmt --check` passed |

## What the checks cover

`tests/engine.rs` checks typed values (including embedded NUL, arbitrary blobs,
empty blobs, i64 limits, and Unicode), copied bindings, statement reset and
errors, commit/rollback/savepoints, automatic rollback after deferred-constraint
failure, B-tree splits and overflow pages, indexes, VACUUM, reopen, WAL snapshots,
writer contention, concurrent connections, FTS4/5, R-tree, JSONB, math/percentiles,
introspection virtual tables, triggers, generated/strict tables, window functions,
CTEs, UPSERT, ATTACH, and WITHOUT ROWID.

`scripts/differential.py` runs independent subprocesses against the pinned C
reference and Rust executable. It contains 36 feature scripts, 7 expected-error
scripts, 600 seeded scalar-expression comparisons, and 6 bidirectional file
interoperability scenarios covering UTF-8, UTF-16LE, and UTF-16BE. The file cases
include updates from the other engine, indexes, overflow pages, and integrity
checks. Scalar expression results use SQLite's `quote()` and `typeof()` for exact
comparison. Other finite floating-point JSON results allow small formatting
roundoff (relative tolerance 2e-14, absolute tolerance 1e-14). Empty result sets
are normalized because the C shell omits them and the Rust shell emits `[]`.
The Rust shell disables statement scan counters as the upstream shell does;
the underlying engine retains its default setting.

`tests/c_api.c` is compiled as a normal C application against each engine. It
checks callbacks and destruction, both directions of variadic calls, custom
SQL functions, tracing, incremental blobs, online backup, serialization and
deserialization, sessions/changesets, UTF-16 preparation, normalized SQL,
progress cancellation, authorization, and allocator cleanup. No C SQLite core
is linked into the Rust client's executable.

`scripts/process_tests.py` uses separate processes loading one engine each.
It checks cross-engine writer exclusion, committed/uncommitted visibility,
WAL snapshot isolation, and integrity. A writer also spills an uncommitted
transaction to disk and exits without cleanup; the other engine must recover
the last committed database. Both directions and rollback/WAL modes pass.
This simulates a process crash; it does not simulate power loss or faulty storage.

`scripts/fuzz_regressions.py` builds SQLite's unmodified upstream `fuzzcheck`
harness, auxiliary test extensions, and recovery helper in C, linking the
**Rust engine** static library. None of the C SQLite core is included. It runs
the eight public corpora from the checksum-verified 3.53.4 source archive.
`--native-malloc` is used because the engine configuration uses the system
allocator rather than the optional MEMSYS5 allocator. Each case has the
harness's 10-second timeout; no cases are selected out or skipped. The harness
checks crashes, leaks, and its SQL invariants; this is not an exhaustive oracle
for every query's expected result.

Final cumulative upstream counts:

| Corpus | Cumulative cases | Reported errors |
| --- | ---: | ---: |
| fuzzdata1 | 9,917 | 0 |
| fuzzdata2 | 19,876 | 0 |
| fuzzdata3 | 22,192 | 0 |
| fuzzdata4 | 24,767 | 0 |
| fuzzdata5 | 33,601 | 0 |
| fuzzdata6 | 37,497 | 0 |
| fuzzdata7 | 45,642 | 0 |
| fuzzdata8 | 46,391 | 0 |

## Reproduce

```sh
cargo fmt --check
cargo test --all-targets
cargo test --release --all-targets
cargo build --release
python3 scripts/build_oracle.py
python3 scripts/differential.py
python3 scripts/check_c_api.py
python3 scripts/process_tests.py
python3 scripts/fuzz_regressions.py
python3 scripts/verify_source.py
RUSTFLAGS='-Zsanitizer=address' CARGO_TARGET_DIR=target/asan cargo test --target aarch64-apple-darwin --test engine
```

The initial run's logs are retained locally in ignored `build/`: `tests.log`,
`tests-release.log`, `asan.log`, `differential.log`, `c-api.log`,
`process-tests.log`, and `fuzz-regressions.log`.

## Limits of the evidence

The private TH3 suite, complete Tcl testfixture suite, exhaustive allocation/I/O
fault matrix, long-duration fuzzing, power-loss simulation, and other target
platforms have not been validated. The unsafe port has not had an independent
memory-safety or concurrency audit. No performance claim is made. The Rust shell
has a smaller command surface than the upstream shell. See [PORTING.md](PORTING.md)
before treating this as a production replacement.
