# Auto-vacuum image format

`ImageBuilder::auto_vacuum(AutoVacuum::Full)` and `AutoVacuum::Incremental`
produce new images with SQLite pointer maps. `AutoVacuum::None` is the default.
`Header` exposes the mode and largest root page. SQL snapshot import/export and
journaled SQL updates preserve the imported mode. `PRAGMA auto_vacuum` reads it;
setting the pragma and executing SQL `VACUUM` or `incremental_vacuum` are pending.

All table and index roots are allocated before any other tree or overflow page.
The allocator inserts pointer-map pages at the required intervals, counts them
against the image/page limits, and skips the lock-byte page. Root entries have no
parent. Interior cells and rightmost-child pointers establish child-to-parent
entries. First overflow pages point to the page holding their cell; subsequent
overflow pages point to the preceding overflow page. Index keys promoted into
interior pages retain the correct overflow owner. Schema pages use the same
rules, including schema-cell overflow and children of page 1.

The header records the largest allocated root, and the incremental-mode flag.
An empty auto-vacuum image has only page 1 and largest-root value 1; a pointer-map
page is allocated only when another actual page is needed. The layout follows
SQLite's [pointer-map specification](https://www.sqlite.org/fileformat2.html#pointer_map_or_ptrmap_pages).

This is format support with fresh, compact image construction. Each commit still
rebuilds the database. It does not preserve freelist pages or reproduce native
incremental-vacuum free-page retention/scheduling. It does not implement an
incremental allocator, pointer-map updates to an existing tree, or a complete
integrity checker. Reserved-byte extension layouts remain unsupported for writes.

## Evidence

Three Rust tests cover allocation limits including map pages, hundreds of roots
spanning several maps, overflow reads, mode retention across schema changes and
rollback at all eight page sizes, empty images, and invalid header combinations.

The native differential test covers 30 scenarios: both modes at eight page sizes,
additional UTF-16 index/overflow cases, many-root/schema-overflow cases, empty
files, and native-origin files in all three encodings. It runs SQLite 3.53.4
integrity checks and forced index scans, then native deletion, root relocation,
incremental vacuum and full vacuum. Logical rows are compared across Rust
reimport/export. Native SQL is only an external test oracle.

```sh
cargo test --test autovacuum
python3 legacy/scripts/build_oracle.py
python3 safe/scripts/autovacuum_differential.py
python3 platform/scripts/file_differential.py
```

Real-file checks also exercise updates while a native connection remains open,
and interrupted commits/recovery of files containing pointer maps. These tests
do not establish hardware power-loss behavior or complete auto-vacuum semantics.
