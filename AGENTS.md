# Project instructions

The ongoing task is a memory-safe rewrite of SQLite covering its public optional
extensions, upstream utilities, and platform behavior. Commercial extensions are
excluded. The safe implementation is incomplete; keep claims and coverage records
consistent with actual implementation and verification.

The default workspace contains `safe/` and `platform/`. Both forbid unsafe code.
The core has no third-party dependencies and supports `no_std + alloc`. The older
unsafe translation stays in `legacy/`, outside the safe workspace and dependency
graph. Native SQLite is an independent test reference, never an engine fallback.

## Publication

The user requested that all project work and subsequent advances be pushed to
`https://github.com/EnesBrt/rustdb-lite`. After meaningful validated progress,
commit the task changes and push the current project branch to that remote.
Do not wait for another publication confirmation. Never force-push or overwrite
unrelated changes. Keep build outputs, downloaded test archives, caches, and
credentials out of commits; preserve required source and test fixtures.

Run checks appropriate to the change, fix failures, and update scope/evidence
documentation before publishing. Test commands and the current compatibility
limits are recorded in `safe/STATUS.md` and the GitHub Actions workflow. Avoid
switching Cargo toolchains while differential tests execute binaries from the
same target directory.
