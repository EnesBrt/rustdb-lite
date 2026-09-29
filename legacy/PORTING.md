# Migration record

## Pinned source and target

- SQLite 3.53.4, source ID `bf7c7f30031888f4e796e429ab3978879485813aaca6f641c7b33e4e09459bcc`.
- Official amalgamation: <https://www.sqlite.org/2026/sqlite-amalgamation-3530400.zip>.
- SHA3-256: `628a44cfe82c66aed1ccbbe85a562d2e33ebe64b3288981ed76285612227934e`.
- C2Rust 0.22.1, LLVM 18.1.8, macOS SDK from the local Xcode command-line tools.
- Rust `nightly-2026-02-19`, rustc `1.95.0-nightly (c04308580 2026-02-18)`.
- Generated target: `aarch64-apple-darwin`. SDK-defined layouts and constants are
  embedded in the generated VFS. Regeneration with another SDK can change them.

`upstream/source.json` contains per-file hashes. `upstream/translation.json`
records the selected definitions, generated source checksum, and patch counts.

## Translation method

The entire configured amalgamation was translated with C2Rust's
`--fail-on-error`; no functions were replaced with panic or unimplemented
stubs. This preserves the existing SQL implementation, planner, bytecode engine,
storage algorithms, and selected extensions in Rust source instead of attempting
an incomplete independent SQL implementation. Translation is not a proof of
semantic equivalence or freedom from undefined behavior.

`scripts/postprocess.py` applies only these repairs:

1. Replace removed `VaListImpl` with the pinned nightly's `VaList` and replace
   `as_va_list()` with cloning of the Darwin variadic cursor. The public C ABI
   tests exercise variadic calls in both directions, formatting, configuration,
   and extension callbacks.
2. Replace removed `atomic_*_relaxed` and `atomic_fence_seqcst` intrinsics with
   typed standard-library atomics in `src/compat.rs`. Memory ordering remains
   relaxed for loads/stores and sequentially consistent for the fence. Types
   cover i32, u8/u16/u32, data pointers, and the log function pointer.
3. Specify two function-pointer transmute destination types for the test-control
   API. No callback behavior is changed.
4. Initialize three character-comparison table pointers with Rust const address
   expressions. C2Rust originally emitted a Mach-O startup constructor for them;
   AddressSanitizer's global redzones made that constructor section invalid for
   Apple's linker. Direct constant initialization removes the constructor and
   its loader dependency while retaining the same addresses.

The generated module keeps C-style names, casts, raw pointers, mutable statics,
bitfields, and unsafe functions. Its lint noise is scoped to that module.
Handwritten code is formatted separately; `cargo fmt` skips the generated file.
Development/test integer overflow checks are disabled to match release arithmetic
for this C-style port; release uses Rust's default disabled overflow checks.

## Remaining work before a stronger claim

- Independently audit unsafe aliasing, pointer alignment/provenance, initialization,
  and cross-thread state. C2Rust output does not inherit all of C's semantics
  automatically. Passing regression tests does not prove memory safety.
- Run broader upstream Tcl/TH3-compatible testing, long-running randomized
  differential testing, allocation/I/O failure injection, and filesystem/power-loss
  tests. The included subprocess tests simulate abrupt process exit, not power loss.
- Benchmark latency, throughput, binary size, and memory use against C SQLite;
  no performance improvement is claimed.
- Generate and validate other OS/architecture VFS implementations. The current
  compile-time target guard prevents accidentally using Darwin ABI layouts elsewhere.
- Refactor subsystems into reviewed idiomatic Rust incrementally, keeping the
  full-engine reference tests. Doing so is a separate substantial migration stage.
- Extend the Rust shell and high-level convenience API if full upstream shell
  command compatibility or broader ergonomic APIs are needed.

This repository therefore provides a complete translation of the selected engine
configuration and a working API/shell, not an independently certified replacement
for all SQLite releases, platforms, utilities, extensions, and reliability guarantees.
