# Generated columns

The safe SQL engine implements `AS (expression)` columns, with optional
`GENERATED ALWAYS` and `VIRTUAL`/`STORED` keywords. An omitted storage kind is
VIRTUAL. This extends the implemented SQL subset; the full SQLite rewrite and
all of its scalar functions are still unfinished.

```sql
CREATE TABLE items(
  id INTEGER PRIMARY KEY,
  title TEXT,
  folded TEXT AS(upper(title)) VIRTUAL UNIQUE,
  width INT GENERATED ALWAYS AS(length(folded)) STORED
);
INSERT INTO items(title) VALUES('Rust') RETURNING *;
INSERT INTO items(title) VALUES('RUST')
  ON CONFLICT(folded) DO UPDATE SET title=excluded.folded||' updated'
  RETURNING id, folded, width;
```

## Evaluation and constraints

Expressions can reference other declared columns, including forward references,
other generated columns and a named INTEGER PRIMARY KEY. Generated values receive
the column's declared affinity and collation, independently of the expression's
own collation. STRICT uses the declared type, including ANY preservation.
Only the already implemented deterministic scalar functions are available.
Parameters, subqueries, aggregates, qualified references, hidden rowid references,
non-deterministic counter functions, DEFAULT and generated primary keys are
rejected. Every table needs an ordinary column. Self references and
cycles wholly among virtual columns are rejected at declaration. Native-style
cycles through stored columns (including a stored self reference) can be
declared and read from stored values, but
INSERT and UPDATE reject their unschedulable dependency graph.

INSERT without a column list accepts only ordinary columns. INSERT, UPDATE and
UPSERT assignments cannot directly write generated columns. Writes calculate
ordinary affinities first and generated dependencies in order. NOT NULL checks
on generated columns follow ordinary NOT NULL checks; substituting a default
under REPLACE recalculates generated values before the second constraint pass.
Dependent CHECK and index constraints are included when an input column changes.
The five conflict policies, UPSERT targets/excluded values, RETURNING, savepoints
and transaction counters are covered by the differential suite.

Virtual reads evaluate the expression on demand, including inside CASE branches,
correlated subqueries and outer joins. NULL-extended join rows remain NULL even
for constant generated expressions. A shared table-local expression graph avoids
expanding every reference into a duplicated expression tree. Dependency depth,
evaluation steps and value/result budgets are bounded. Resource limits can reject
queries accepted by native SQLite, and are not SQLite's configurable limits.

## Files and metadata

STORED values occupy physical record fields; VIRTUAL values do not. Import retains
stored generated values exactly as stored, and delays virtual evaluation until
needed. Export omits virtual slots from rowid records and WITHOUT ROWID primary
payloads. Index keys on virtual columns are evaluated when indexes are checked or
rebuilt, with the same physical rowid/primary-key suffixes as other column indexes.
Index access paths and incremental B-tree mutation remain unfinished.

`table_info` omits generated columns and renumbers its returned `cid` values.
`table_xinfo` reports all declared columns with `hidden=2` for VIRTUAL and `hidden=3`
for STORED. `table_list` counts all columns. Views and CREATE TABLE AS SELECT use
computed values and declared result types.

## Verification and remaining scope

```sh
cargo test -p sqlite-safe-core --test generated
python3 safe/scripts/generated_differential.py
python3 platform/scripts/file_differential.py
```

The differential suite compares 1,368 declaration, query, DML, conflict, counter,
metadata and image scenarios against pinned SQLite 3.53.4. Image comparisons cover
all eight page sizes, three text encodings, three auto-vacuum modes and both table
storage kinds. They check native integrity, forced native index scans and native
mutation followed by Rust import/export. Fixtures also establish lazy errors on
imported virtual columns and preservation of stored values after a generating
expression changes in the native schema.

Seven Rust tests cover bindings, dependencies, outer scopes, REPLACE/UPSERT,
metadata, cycles, malformed prefixes, resource limits, STRICT error prefixes and
72 combinations of encoding/page size/vacuum (each with both table kinds).
A Unix test and nine native file scenarios cover commit/reopen, index constraints,
retained FAIL prefixes and unchanged files after aborted or rolled-back writes.

Expression and partial indexes can use generated values; see
[EXPRESSION_INDEXES.md](EXPRESSION_INDEXES.md). ALTER TABLE (including ADD COLUMN),
foreign keys, triggers, date/time, JSON and other missing functions remain unsupported.
Imported schemas must otherwise fit the supported SQL subset. The query execution
and error-timing differences in [QUERIES.md](QUERIES.md), planner-dependent row
order and the STRICT physical-error difference in [STRICT.md](STRICT.md) still
apply. These tests do not establish all generated-expression combinations or
production compatibility. Native constant-folding exceptions for cyclic virtual
expressions, such as `g AS(0 AND g)` and `g AS(1 OR g)`, are not reproduced;
these declarations are currently rejected.

Reference: [SQLite generated columns](https://sqlite.org/gencol.html); implementation
behavior is checked against the pinned native `build.c`, `insert.c`, `update.c`,
`expr.c` and `vdbe.c` source and executable, not a linked engine fallback.
