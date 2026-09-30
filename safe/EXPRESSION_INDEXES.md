# Expression and partial indexes

The safe SQL engine supports `CREATE [UNIQUE] INDEX` with column/expression keys
and an optional `WHERE` predicate, within its implemented expression subset.
This adds schema, constraint and file-format behavior; the query optimizer and
complete SQLite compatibility remain unfinished.

```sql
CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT, active INT);
CREATE UNIQUE INDEX active_name ON items(lower(name)) WHERE active=1;
INSERT INTO items(name,active) VALUES('Rust',1),('RUST',0);
INSERT INTO items(name,active) VALUES('RUST',1)
  ON CONFLICT(lower(name)) WHERE active=1
  DO UPDATE SET name=excluded.name
  RETURNING *;
```

## Keys, membership and constraints

An index can mix ordinary columns, generated columns and expressions, with
ASC/DESC and the implemented BINARY, NOCASE and RTRIM collations. Only implemented
deterministic scalar functions are available. Parameters, subqueries, aggregates,
non-deterministic counter functions and hidden-rowid key references are rejected.
Named INTEGER PRIMARY KEY columns can be keys. Predicates can reference hidden
rowids on rowid tables and can qualify columns with the table name; key
expressions cannot use qualified column references. Unknown functions are errors.

Only rows whose predicate is true belong to a partial index; false and NULL
exclude them. The predicate is evaluated before key expressions. Consequently,
an excluded row does not fail merely because its key expression would fail.
Non-unique indexes also evaluate their included keys and propagate errors.
CREATE INDEX checks existing rows atomically before installing its schema.

Uniqueness compares included keys using their collations, retaining SQLite's
distinct-NULL behavior. INSERT, UPDATE and UPSERT account for key dependencies,
predicate dependencies, generated dependencies and rowid/primary-key changes.
Entering a partial index rechecks uniqueness even when the key value is unchanged.
The five conflict policies and transaction/savepoint rollback apply to this work.
Functions in affected indexes can require statement rollback on a STRICT
datatype error, even when the index is non-unique; see [STRICT.md](STRICT.md).

UPSERT expression targets are compared structurally, including argument order
and nested collations. Partial targets require the same predicate structure.
For example, `x+1` does not match `1+x`, and `y>0` does not match `0<y`.
The matcher retains tested native lexical distinctions for CAST type spelling,
large numeric literals, REAL literals and BLOB literals. Small integer constants
have special native COLLATE-matching behavior covered by tests. This is not a
claim that every lexical variant or bound-parameter comparison is implemented.
A WHERE clause on a target matching a full unique index is resolved but does
not filter that index. A valid subquery there does not become an early bind error.

## Files and metadata

`index_list` reports the partial flag. `index_info` and `index_xinfo` report
expression keys with `cid=-2` and a NULL name; ordinary keys retain their declared
column numbers. Root COLLATE names, sort directions and key/auxiliary flags are
preserved. Rowid indexes append rowid; WITHOUT ROWID secondary indexes append
the required primary-key fields, with the existing collation/direction rules.

Import reconstructs indexes from declarations and table values. Export and
journaled writes rebuild sorted index trees with the appropriate membership,
including overflow records, interior records, all eight page sizes, all three
text encodings and auto-vacuum pointer maps. The Unix adapter persists these
images through the existing rollback-journal protocol.

This rebuilding does not preserve stale expression-index entries or reproduce
every error side effect of externally modified/corrupt schemas. Collision checks
can reevaluate existing rows instead of reading their stored index entries.
Import is not a complete physical index integrity check. Incremental index
mutation, index-based query access, INDEXED BY, optimizer-dependent scan/error
order and REINDEX remain unfinished. Query materialization limits documented in
[QUERIES.md](QUERIES.md) continue to apply.

## Verification

```sh
cargo test -p sqlite-safe-core --test expression_index
python3 safe/scripts/expression_index_differential.py
python3 platform/scripts/file_differential.py
```

The new differential suite checks 2,078 declaration, membership, conflict,
transaction, counter, UPSERT, STRICT-error, metadata and image scenarios against
pinned SQLite 3.53.4. Its 144 small-image combinations cover both table storage
kinds, eight page sizes, three encodings and three auto-vacuum modes. Another
24 larger images exercise overflow and interior records. Native integrity
checks, forced native index scans, native mutation and Rust reimport/export
verify the physical layouts. Forced index scans run in the independent native
test reference, not in the safe engine.

Seven Rust tests cover matching and rejection, atomic errors, resource limits,
malformed prefixes, partial membership changes, metadata, STRICT error prefixes
and 72 image combinations. A Unix test and nine native file scenarios cover
commit/reopen, matching UPSERTs, failed writes, rollback and retained FAIL prefixes.
The existing 592-case UPSERT suite also covers target-WHERE subquery resolution.

The implementation remains dependency-free safe Rust with `no_std + alloc`;
native SQLite is used only as a separately built test reference. Public
extensions, upstream utilities and full platform support remain unfinished.
