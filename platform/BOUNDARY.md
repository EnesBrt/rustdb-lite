# OS boundary review

The safe core and this adapter both use `#![forbid(unsafe_code)]`. Core SQL,
format parsing, record/tree writing, journal recovery and commit ordering remain
independent Rust implementations. Native SQLite is used only by subprocess tests.

The platform crate adds `rustix = "=1.1.5"` with its `fs` feature. Cargo.lock pins
its transitive packages. `rustix`, its OS-binding dependencies, Rust's standard
library, the compiler, allocator and kernel are trusted components. Their use of
unsafe code and OS FFI is outside this crate's policy. This is not a formal proof
of their memory safety or of storage hardware behavior.

## Reviewed call paths

- `rustix::fs::fcntl_lock` borrows a valid file descriptor and constructs the
  target's native `flock` structure. The selected operation uses `F_SETLK` with
  `F_WRLCK`, start 0 and length 0. It locks through end-of-file and beyond, covering
  SQLite's byte-range locks. It is not `flock(2)`. Lock failures are propagated;
  EAGAIN/EACCES are returned as `Error::Busy`.
- `openat`, `statat` and `unlinkat` operate relative to an owned directory handle.
  Safe path arguments provide NUL validation and temporary-buffer lifetimes.
  Owned descriptors become `std::fs::File` through its safe conversion.
- `FileExt::read_exact_at` and `write_all_at` use checked Rust slices. Reads check
  file length against the caller's budget before reserving memory. All writes
  require the full buffer to be accepted, otherwise the error reaches the pager.
- `rustix::fs::fsync` and Apple's `fcntl_fullfsync` take borrowed descriptors.
  The adapter calls both on Apple, propagating failures. Other implemented Unix
  targets use `fsync`. There is no relaxed-sync fallback.
- `/dev/urandom` supplies the journal checksum seed through standard safe reads.
- The process-local registry serializes inode acquisition and descriptor closure.
  An inode already held by this crate is rejected before another descriptor is
  opened. If a path race is detected only after open, that descriptor is retained
  until the existing connection unlocks; further opens are refused while such a
  deferred close is pending. Normal drop closes descriptors under the registry
  mutex before another connection can acquire that inode.

The reviewed dependency sources are `src/fs/fcntl.rs`, `src/fs/at.rs`,
`src/fs/sync.rs`, `src/fs/fcntl_apple.rs`, and the corresponding libc/Linux-raw
backend implementations in rustix 1.1.5. These conclusions concern the API paths
used here, not every function exported by rustix.

## Limits of this boundary

POSIX locks are process-associated. The registry cannot coordinate independent
native SQLite copies or arbitrary file descriptors in the same process, nor
connections copied by fork. Such use is unsupported. Stable paths and cooperating
advisory-lock users are required. Network filesystem semantics, alternate native
SQLite VFS lock mechanisms, unsupported operating systems and device behavior
that violates sync guarantees are not covered by local evidence.

Local tests verify actual lock conflicts with separate native SQLite processes,
this crate's concurrent threads, real-file recovery, and process interruption.
They cannot establish power-loss safety of every filesystem/device or runtime
correctness on targets that have only been compiled.

References: [rustix fcntl API](https://docs.rs/rustix/1.1.5/rustix/fs/fn.fcntl_lock.html),
[POSIX fcntl](https://pubs.opengroup.org/onlinepubs/9799919799/functions/fcntl.html),
[Apple fcntl](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html),
[SQLite locking](https://www.sqlite.org/lockingv3.html),
[SQLite atomic commit](https://www.sqlite.org/atomiccommit.html).
