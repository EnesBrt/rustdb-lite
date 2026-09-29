# RETURNING

The safe SQL engine supports `RETURNING` on its rowid-table INSERT, REPLACE,
UPSERT, UPDATE and DELETE statements. This is part of the experimental SQL
subset, not a claim of complete SQLite compatibility.

```sql
CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT UNIQUE, count INTEGER);
INSERT INTO items(name,count) VALUES('Rust',1) RETURNING id,name;
INSERT INTO items(name,count) VALUES('Rust',2)
  ON CONFLICT(name) DO UPDATE SET count=count+excluded.count
  RETURNING *;
DELETE FROM items WHERE count>2 RETURNING id,name,count;
```

## Results and errors

- INSERT and UPDATE return the new row values; DELETE returns the removed values.
  UPSERT returns successfully inserted and updated rows. IGNORE, DO NOTHING and
  a false DO UPDATE WHERE condition produce no result for the skipped row.
- The projection accepts expressions, parameters, aliases and bare `*`. Result
  labels preserve declared column names; hidden rowid aliases use the INTEGER
  PRIMARY KEY column name when present. Statements affecting no rows still return
  their column metadata.
- `Connection::execute` and `execute_prepared` buffer the entire result and return
  it in `QueryResult`. `changes` counts changed rows. There is no streaming step
  API; successful autocommit persistence completes before the journaled API
  returns these rows.
- Binding errors occur before data changes. Evaluation and resource errors abort
  the statement and discard its buffered result. A constraint FAIL discards the
  result but retains its successful write prefix, which the journaled API persists
  in autocommit mode. Constraint ROLLBACK follows the transaction rules in
  [CONFLICTS.md](CONFLICTS.md).
- RETURNING expressions observe the previous statement's `changes()` and
  `total_changes()`. `last_insert_rowid()` observes successful inserts as they
  happen, and remains changed even if a later error undoes an insert.

## Scope and query evaluation

The target's declared name qualifies RETURNING columns. An explicit DML `AS`
alias instead qualifies UPDATE assignments and UPDATE/DELETE filters. That alias,
`excluded`, and `old`/`new` pseudo-tables are not visible as RETURNING row sources
in the pinned SQLite 3.53.4 behavior. `table.*` and top-level aggregates are
rejected. Aggregates inside expression subqueries are supported.

Correlated subqueries see the returned row. A subquery directly reading the
modified table is reevaluated after each successful change. Uncorrelated queries
through a view, derived source or CTE can retain their first result. INSERT and
each UPSERT update clause have separate expression caches. Explicit MATERIALIZED
CTE results are shared with the data-changing portion of the statement; NOT
MATERIALIZED CTEs keep separate results for the RETURNING programs. Default CTE
results are shared between those RETURNING programs but not with the ordinary
DML runtime.

These rules reproduce the pinned cases, including counter timing and CTE
shadowing. SQLite's planner can change evaluation and result order, especially
for self-referencing subqueries. The engine still uses scans and bounded
materialization; full planner/evaluation equivalence is not established. General
query limitations remain listed in [QUERIES.md](QUERIES.md).

## Bounds and evidence

Projection width is capped at 2,000 columns. Projection names and buffered values
are checked against the SQL byte budget, rows against the result-row limit, and
expression/query work against the shared execution fuel. This is a logical-data
budget, not a process RSS limit.

Six Rust tests cover reusable bindings, labels, empty results, cache behavior,
conflict policies, rollback, malformed prefixes, resource bounds and all three
database encodings/auto-vacuum modes. A Unix test checks persisted output, failed
evaluation and retained FAIL writes. Native comparisons are reproducible with:

```sh
python3 legacy/scripts/build_oracle.py
python3 safe/scripts/returning_differential.py
python3 platform/scripts/file_differential.py
```

The RETURNING script compares 311 scenarios with SQLite 3.53.4, including exact
column names, typed values, errors, autocommit state and connection counters.
The file suite includes nine RETURNING cases across encodings and auto-vacuum
modes. Native SQLite is only a test reference, never an execution fallback.

RETURNING is not a table-valued source and cannot be composed inside a CTE.
Triggers, virtual tables, UPDATE FROM, DML ORDER BY/LIMIT, foreign keys and the
C API remain unimplemented.
