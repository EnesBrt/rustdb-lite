# Native storage adapters

This separate crate connects the safe SQLite core to operating-system storage.
Both crates forbid unsafe code. The core still has no third-party dependencies;
this adapter uses the pinned `rustix` 1.1.5 safe syscall interfaces and the standard
library. Their internals are part of the trusted OS boundary, not rewritten SQL.
See [BOUNDARY.md](BOUNDARY.md) for the reviewed call paths and limitations.

`unix::UnixStorage` is implemented for Linux, Android, macOS and iOS configurations.
**Runtime evidence currently covers macOS ARM64 only.** Six target configurations
compile; that does not establish runtime or filesystem guarantees on those targets.
Windows, web, embedded and other adapters remain unfinished.

## Using a real file

The existing parent directory must be stable and durable. The adapter opens or
creates the main file when the connection acquires its lock.

```no_run
#[cfg(any(target_os="linux", target_os="android", target_os="macos", target_os="ios"))]
fn update() -> sqlite_safe::Result<()> {
    use sqlite_safe::{journal::JournalLimits, sql::JournaledConnection};
    use sqlite_safe_platform::unix::UnixStorage;
    let storage = UnixStorage::new("items.db")?;
    let mut db = JournaledConnection::open(storage, 4096, JournalLimits::default())?;
    db.execute("CREATE TABLE IF NOT EXISTS items(id INTEGER PRIMARY KEY, name TEXT UNIQUE)", &[])?;
    db.execute("BEGIN", &[])?;
    db.execute("INSERT INTO items(name) VALUES('Rust')", &[])?;
    db.execute("COMMIT", &[])?;
    Ok(())
}
```

A small executable example accepts one complete SQL statement per argument:

```sh
cargo run -p sqlite-safe-platform --example file_sql -- items.db \
  'CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT UNIQUE)' \
  "INSERT INTO items(name) VALUES('Rust')" \
  'SELECT * FROM items'
```

It prints Rust result values. It is a developer example, not upstream shell
compatibility. The original `sqlite-safe-sql` command still imports/exports
snapshots and does not automatically opt into file access.

## File and locking behavior

The connection holds an exclusive, nonblocking POSIX `fcntl` write lock for its
entire lifetime. Its range overlaps all of native SQLite's lock bytes, including
bytes beyond the current file size. This prevents standard POSIX-locking SQLite
connections in **other processes** from reading or writing concurrently. Contention
returns `Error::Busy`; no busy timeout or shared-reader mode is implemented yet.
An inode registry prevents this crate's own connections or threads from bypassing
process-associated POSIX locks. A failed duplicate open does not open and close
another descriptor of the locked inode.

Do not concurrently use unrelated descriptors or a separately loaded native
SQLite library for the same database in this process. POSIX locks belong to the
process, and closing any descriptor of an inode can release them. Do not reuse
connections after fork. The registry only coordinates this crate's connections;
network filesystems and alternative native locking VFSes are not validated.

Files are opened relative to a retained directory descriptor, with `O_NOFOLLOW`
and close-on-exec. Main files and sidecars must be regular files with one hard
link. Metadata checks reject changed directory/file identities. Paths must remain
stable; the adapter does not claim protection against arbitrary external renames
or writers that ignore advisory locks.

Commit follows the core journal protocol. Main and journal contents are synced;
directory entries are synced after creation and deletion. On Apple targets,
`F_FULLFSYNC` follows `fsync`, including on the directory. Failures are propagated,
without silently weakening the requested synchronization. Journal headers use a
conservative 65,536-byte sector alignment. Existing WAL state is refused.

The SQL connection preserves page size, text encoding, application ID and user
version, together with auto-vacuum pointer-map layouts. It refuses reserved-byte
databases until image writing supports those extension formats. It rebuilds entire table/index images; it does
not implement incremental page mutation or WAL writing. SQL support is still the
subset documented in [the core SQL reference](../safe/SQL.md).
Incremental-vacuum free-page retention/scheduling is also pending; the rebuilt
images are compact even in that mode. See [auto-vacuum format support](../safe/AUTOVACUUM.md).

## Validation

Fifteen native Rust tests cover commit/rollback/reopen, recovery, corrupt-journal
retention, bounded reads, file identity/sidecar guards, connection lifetime, and
an eight-thread race for one inode, plus persisted CTE/subquery data changes and
unchanged files after failed inserts, plus persisted views and CREATE TABLE AS SELECT. They pass
locally in debug and release.

The native differential script checks 68 scenarios involving all three database
encodings, metadata retention, already-open native connections, native read and
write locks, failed duplicate Rust opens, updates of both auto-vacuum modes, and
a recursive CTE insert, a correlated update and view/schema changes that native
SQLite reopens and validates.
It also interrupts 210 real commit positions in ordinary and pointer-map files,
including explicit partial page writes, and checks both Rust and SQLite 3.53.4
recovery. This is process-interruption evidence, not a physical power-loss test.
The persistent native peer here is Python's SQLite 3.53.1.

```sh
cargo test --workspace --all-targets
cargo test --workspace --release --all-targets
cargo clippy --workspace --all-targets -- -D warnings
python3 legacy/scripts/build_oracle.py
python3 platform/scripts/file_differential.py
python3 platform/scripts/check_targets.py
```

[Compile results](coverage/compile-checks.json) cover macOS ARM64, Linux x86-64,
i686, s390x, Android ARM64 and iOS ARM64. OS runtime tests beyond this Mac,
physical power-loss tests, full VFS behavior, concurrent readers, native ABI,
and complete platform support remain unverified or unimplemented.

Constraint FAIL persists its successful prefix in autocommit mode before returning
the SQL error. Within a transaction the prefix remains private until commit;
constraint ROLLBACK discards it. Rust and native interchange tests cover these
paths, including reopening after errors; see [conflict policies](../safe/CONFLICTS.md).

UPSERT persists through the same commit protocol. Tests cover native readers
across updates in all encodings and auto-vacuum modes, and ensure a failed
DO UPDATE leaves the previously committed image intact. See [UPSERT](../safe/UPSERT.md).

RETURNING results are buffered and delivered after successful autocommit
persistence. Failed evaluation restores the prior file; a constraint FAIL keeps
and persists its write prefix without returning partial rows. Tests cover both
behaviors, explicit rollback, encodings and auto-vacuum modes; see
[RETURNING](../safe/RETURNING.md).

AUTOINCREMENT and sqlite_sequence persist in the same image commit. Tests cover
reopening after deleting rows, retained FAIL prefixes, native readers and FULL
rollback without altering the prior committed file. See
[AUTOINCREMENT](../safe/AUTOINCREMENT.md).

STRICT datatype errors may retain a transaction prefix without counting changes.
The adapter persists that prefix on COMMIT/outer RELEASE and discards it on
rollback. A Rust test and nine native file scenarios cover this behavior in the
three text encodings and three auto-vacuum modes; see [STRICT](../safe/STRICT.md).

WITHOUT ROWID tables persist through primary index B-trees and secondary-key
suffixes. A Rust test and nine native file scenarios cover primary-key updates,
FAIL prefixes, rollback, STRICT errors and native readers across writes; see
[WITHOUT ROWID](../safe/WITHOUT_ROWID.md).
