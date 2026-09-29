# Experimental journaled storage

The safe core now implements rollback-journal encoding/recovery and a full-image
commit protocol. `sql::JournaledConnection<S>` connects that protocol to the SQL
subset. The separate [platform crate](../platform/README.md) now provides a Unix
file adapter with local macOS ARM64 runtime tests. Other OS runtime guarantees and
physical power-loss behavior remain unverified. The snapshot CLI uses offline images.

## API and commit behavior

`pager::Storage` is the adapter contract. It supplies exclusive locking, bounded
reads, sector size, a fresh checksum seed, complete writes, synchronization,
exclusive journal creation and deletion, and directory synchronization.
`pager::Pager<S>` acquires and retains that exclusive lock until it is dropped.
It recovers an existing journal before exposing the snapshot, and refuses WAL
sidecars or WAL-mode main files.

A journaled SQL connection opens an existing main file (which may be empty):

```rust
use sqlite_safe::{journal::JournalLimits, pager::Storage,
                  sql::JournaledConnection, Result};

fn update<S: Storage>(storage: S) -> Result<()> {
    let mut db = JournaledConnection::open(storage, 4096, JournalLimits::default())?;
    db.execute("CREATE TABLE IF NOT EXISTS items(id INTEGER PRIMARY KEY, name TEXT UNIQUE)", &[])?;
    db.execute("BEGIN", &[])?;
    db.execute("INSERT INTO items(name) VALUES('example')", &[])?;
    db.execute("COMMIT", &[])?;
    Ok(())
}
```

Successful autocommit writes, explicit COMMIT, and outermost RELEASE persist the
whole new image. An autocommit constraint FAIL persists its successful prefix
before returning the error. Constraint ROLLBACK discards the active transaction;
ABORT restores just the statement. See [conflict policies](CONFLICTS.md).
BEGIN and savepoints modify private memory until commit;
ROLLBACK and dropping an uncommitted connection discard those changes. Read-only
statements do not write a journal. The connection retains exclusive access even
while idle; shared-reader/reserved/pending-lock transitions are not implemented.

The commit protocol writes a complete undo image under an invalid header, syncs
its contents and directory entry, activates the journal header, and syncs again.
Only then does it write/truncate the database and sync it. Journal removal and
directory sync finish the commit. The file change counter and schema cookie
advance on each committed image. Recovery syncs its source journal before
restoring pages, syncs the restored database, then removes the journal.

An error during persistence poisons the connection. Closing and reopening runs
recovery. An error after journal deletion may mean that the commit took effect;
the caller must inspect the reopened state before retrying the operation. Failed
validation before any pager write does not poison a standalone `Pager`; the SQL
wrapper also becomes unavailable on export failures to avoid exposing an
unpersisted autocommit state.

## Journal codec

`journal::encode` writes standard SQLite page records, sparse page checksums,
and a padded sector header. It records every original page, including pages
removed when an image shrinks. Empty original files are supported.
`journal::recover` works on immutable byte snapshots, including multiple aligned
journal segments and the unbounded-record-count representation. Duplicate page
numbers retain their first undo image. It verifies record checksums and budgets,
and rejects missing pages when a truncated main file cannot be reconstructed.

This codec does not decide whether an arbitrary live journal is hot. Locking and
super-journal state are needed for that decision. It rejects super-journals,
legacy zero page-size headers, damaged declared records, and nonzero invalid
segment headers; it does not claim SQLite's complete permissive recovery behavior.
Zero headers and unfinished zero-header tails are treated as inactive. Torn
activation headers are retained as errors; the protocol has not started main-file
writes at that point. Bounds default to 256 MiB/image, 272 MiB/journal and 100,000
records. Page-size changes, multi-file transactions and the lock-byte page are
outside the writer's current scope.

The format and ordering are checked against SQLite's public
[file format](https://www.sqlite.org/fileformat.html),
[atomic commit](https://www.sqlite.org/atomiccommit.html), and
[locking](https://www.sqlite.org/lockingv3.html) documentation and the pinned
3.53.4 pager source.

## Evidence and remaining work

Tests cover 600 injected commit failure/crash variants with growth, truncation,
empty origins, writes reaching storage before sync, partial writes, failures
before/after operations, and lost unsynced state. Separate tests interrupt
recovery repeatedly, verify exclusive-lock lifetime, and exercise SQL autocommit,
transactions, savepoints, statement errors, and reopen behavior. These are a
simulation of the adapter contract, not real power-loss or OS-locking tests.
The SQL FAIL prefix-commit path also has injected before/after-operation failures,
including torn writes and recovery into the original or committed prefix state.

The native differential suite validates eight Rust-written journal page sizes
using SQLite 3.53.4 recovery. Three native spill-journal snapshots, produced by
Python SQLite 3.53.1, recover byte-for-byte with both Rust and the pinned reference.
Normal Rust tests also exercise all page sizes, different sector sizes, every
truncation of a small encoded journal, and 3,000 deterministic journal mutations.

The Unix adapter additionally passes native process-lock tests and 210 real
process-interruption/partial-write recovery points on macOS. It uses POSIX `fcntl`
locks, not `flock`. Its documented process-local descriptor restrictions apply.
Other platform adapters, shared readers, incremental page caching/mutation, WAL
writing/checkpointing, attached database commits, hardware power-loss testing and
complete platform runtime coverage remain unfinished.

The pager refuses changes to encoding, reserved-byte layout and auto-vacuum mode
of an existing file. The journaled SQL wrapper preserves auto-vacuum pointer-map
formats, but rebuilds compact images without incremental-vacuum free-page retention;
see [AUTOVACUUM.md](AUTOVACUUM.md). It rejects reserved-byte databases because its
image builder cannot preserve extension data in those layouts.
