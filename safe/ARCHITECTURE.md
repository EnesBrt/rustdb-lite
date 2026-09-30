# Rewrite contract and architecture

## Scope

The user requested a memory-safe rewrite covering every public optional extension,
all upstream utilities, and every platform. Commercial SEE, ZIPVFS,
and CEROD products are excluded by their explicit clarification.

Compatibility is measured against the pinned public SQLite 3.53.4 tree. The
inventory includes all files under `src`, `ext`, and `tool`, including auxiliary
build/developer tools. Each miscellaneous C extension has a separate entry;
larger extensions are grouped by upstream directory, with all member files
recorded. Tests and documentation inside extension directories remain in the
source manifest. A later SQLite version or a third-party extension requires an
explicit inventory update. The current inventory is a coverage denominator,
not a statement that these components have been implemented.

## Memory-safety boundary

- The engine and rewritten public extensions must use owned values, borrowing,
  checked indexing, and typed handles. They must forbid unsafe code.
- No FFI, C engine, C2Rust output, or fallback to the legacy engine is permitted
  inside the safe core. It currently has zero third-party dependencies.
- `core`, `alloc`, Rust's standard library, the compiler, allocator, and host
  operating system are trusted runtime components. Their internals are outside
  this crate's `forbid(unsafe_code)` policy; the policy is not a proof about them.
- The separate platform crate also forbids unsafe code, using pinned rustix safe
  APIs for OS operations. Rustix and its OS-binding dependencies form an explicit
  trusted boundary; see [the boundary review](../platform/BOUNDARY.md). The core
  dependency graph remains empty.
- Malformed inputs must not produce unchecked memory access. They should also
  return bounded errors rather than panic, recurse indefinitely, or allocate
  according to untrusted sizes without checking resource budgets.
- SQL/data correctness, crash durability, lock correctness, and resource denial
  of service are independent obligations. Safe Rust alone does not prove them.
- Native C ABI entry points and arbitrary native plugins cannot have the same
  unconditional in-process guarantee as a safe Rust API. A future compatibility
  boundary must be explicitly audited and kept out of the safe core. Native
  plugin isolation would require a separate process/runtime and its own semantic
  contract; such isolation is not implemented or claimed today. Public upstream
  extensions themselves are to be rewritten in safe Rust, not wrapped C.

## Layering

1. **Format and values:** byte-exact headers, varints, records, text encodings,
   page layouts, checksums, resource limits, and error types. Implemented in part.
2. **Storage:** page identifiers/owned buffers, B-tree cursors and mutation,
   freelists, pointer maps, overflow pages, and integrity checks. Snapshot reads
   and fresh-image table/index builders are implemented; incremental mutation is not.
3. **Pager and VFS:** explicit I/O and locking interfaces, rollback journals,
   WAL/index synchronization, checkpoints, recovery, atomic commit, injected
   failures, and durable filesystem ordering. Offline WAL/journal reconstruction,
   a rollback-journal writer, a storage trait, and a full-image commit protocol
   with simulated fault tests are implemented. The SQL journaled API uses this
   protocol. A separate Unix adapter now has real-file, locking and process-crash
   tests on macOS ARM64; six target configurations compile. Other runtime and
   power-loss guarantees remain unverified; see [PAGER.md](PAGER.md).
4. **SQL:** tokenizer, parser, AST, name/type resolution, query planning,
   execution, collations/functions, schema changes, constraints, triggers, views,
   transactions, pragmas, and public connection/statement APIs. An experimental
   scan-based in-memory subset now exists, including inner/outer joins with
   ON/USING/NATURAL, aggregates, constraints,
   derived tables, ordinary/recursive CTEs, compound queries, scalar/EXISTS/IN and
   correlated subqueries, stored views, CREATE TABLE AS SELECT, prepared parameters,
   statement rollback, and savepoints. The optimizer and
   complete SQL/schema/API semantics remain unfinished; see [SQL.md](SQL.md).
5. **Extensions:** safe function, collation, tokenizer, and virtual-table
   interfaces; independent implementations of every public inventory entry.
   No extension is complete until integrated and differentially tested through
   SQL. Not implemented.
6. **Utilities and bindings:** upstream shell behavior, dump/backup/import,
   `sqldiff`, analyzer, rsync, expert/RBU/recovery tools, generators/build tools,
   and public bindings. The offline inspector and experimental SQL command do
   not count as completing any upstream utility. Upstream parity is not implemented.

This decomposition preserves the complete requested goal. A milestone is not a
substitute for completing the engine, extension, utility, and platform matrices.

## Platform contract

"Every platform" cannot be established by a single host test. The core avoids OS,
pointer-width, alignment, and native-endian dependencies and builds with
`no_std + alloc`. That is a portability foundation, not functioning database
support on each target. The compiler's 307 target triples are inventoried with
no implied validation. Some targets may require allocator support, Rust/LLVM
porting, custom VFS implementations, or unavailable test hardware.

Each supported platform needs separate evidence for:

1. Compilation of the relevant core, APIs, utilities, and adapters.
2. Runtime SQL/format/extension/utility conformance on that platform.
3. Multi-process locking and shared-memory behavior where available.
4. Transaction recovery, injected I/O failures, power-loss/durability tests,
   and deployment-specific storage behavior.

The CI workflow runs native core tests on Linux, Windows and macOS, native Unix
adapter tests on Linux/macOS, and cross compilation for additional targets.
Published passing runs and their exact scope are recorded in [STATUS.md](STATUS.md).
Those results do not establish every platform or power-loss contract. Embedded and web
targets will require explicit storage capability contracts; filesystem-dependent
utilities cannot be silently treated as supported when that capability is absent.

## Completion gate

The rewrite is complete only when the safe engine executes the target SQL/API
surface, every public extension/utility inventory entry has a reviewed implemented
or explicitly agreed disposition, and the promised platform matrix has runtime
evidence. It also requires differential SQL tests, ABI/binding tests, query planner
and utility fixtures, fuzzing of malformed SQL/files, resource exhaustion tests,
concurrency and crash/fault testing, and independent review of any boundary code.
The unsafe legacy port's passing tests do not satisfy this gate for the safe core.
