# Query composition and recursive execution

The safe engine implements derived FROM tables, ordinary and recursive common
table expressions (CTEs), VALUES queries, UNION/UNION ALL/INTERSECT/EXCEPT, and
scalar/EXISTS/IN subqueries with correlated columns. INNER/CROSS/LEFT/RIGHT/FULL
joins support ON, USING and NATURAL, including parenthesized groups, merged-column
values and types, result aliases in ON and permitted forward ON references;
see [JOINS.md](JOINS.md). Stored views use the same
query machinery; [VIEWS.md](VIEWS.md) describes their schema and scope rules.
This expands the [experimental SQL subset](SQL.md); it does not complete SQLite's
query language, planner, or API compatibility.

```sql
WITH RECURSIVE numbers(n) AS (
  VALUES(1)
  UNION ALL SELECT n+1 FROM numbers WHERE n<5
)
SELECT n, n*n FROM numbers ORDER BY n;
```

## Implemented behavior

- Query sources carry column names, affinities and collations. Duplicate derived
  column names receive numeric suffixes, and derived sources have no hidden rowid.
- Compound source columns infer affinity from all their expressions and their
  possible storage classes, including branches that return no rows. Incompatible
  text/numeric branches suppress coercion. Numeric CAST compounds preserve existing
  integer/real values when CREATE TABLE AS SELECT writes them. Scalar compound
  subqueries take expression affinity from the final SELECT; their first returned
  value can come from an earlier SELECT. These are separate rules.
- CTE names are scoped to their WITH statement, support forward references and
  nested shadowing, and can have explicit result-column lists. `main.name`
  resolves a stored table or view even when a CTE shadows that name. Unused CTEs are
  not evaluated. Results and bindings are cached only within one execution.
- WITH prefixes SELECT, INSERT/REPLACE, UPDATE and DELETE. The selected conflict policy
  and journaled persistence paths also apply to those data changes, including
  scalar and predicate subqueries in INSERT/UPDATE/DELETE.
- Compound operators associate left to right, require matching column counts,
  and apply global ORDER BY/LIMIT/OFFSET. ORDER BY accepts result ordinals, aliases
  and matching result expressions from the component SELECTs. Set comparisons
  preserve storage types, compare NULLs as equal for duplicate removal, and use
  the selected collation without applying column affinity conversions.
- Recursive CTEs accept one or more initial and recursive terms. Each recursive
  term reads the CTE once directly; all recursive connectors must use the same
  UNION or UNION ALL operator. Recursive aggregate terms, indirect self-reference
  and mutual recursion are rejected. The RECURSIVE keyword is optional.
- A recursive working table contains one dequeued row. FIFO queues implement
  unordered traversal; recursive ORDER BY prioritizes the next row. UNION retains
  a seen set for cycle suppression; UNION ALL retains duplicates. A recursive
  OFFSET still expands skipped rows, while LIMIT caps emitted rows. A simple
  outer LIMIT/OFFSET can stop a single recursive source early.
- Scalar subqueries return the first row's single value, or NULL for no rows.
  They retain result affinity but do not inherit the result expression's collation.
  EXISTS returns 0/1 and skips plain projection expressions; aggregate inputs
  still execute. IN/NOT IN subqueries use one result column, with the native
  empty-set, NULL, affinity and comparison-collation rules covered by tests.
- Correlated expressions resolve columns in the nearest available outer row.
  Nested subqueries, CTEs, joins, grouping and data changes can use these values.
  Uncorrelated expression results are cached within the executing statement;
  correlated results are reevaluated. CTE caches distinguish declaration scopes
  and do not reuse results that depend on an outer row.
- Unselected CASE/coalesce/iif branches do not execute subqueries, but their names
  and result-column counts are validated. Subqueries cannot appear in DEFAULT or
  CHECK expressions. LIMIT/OFFSET expressions cannot reference outer columns.

The implementation is in `src/sql/connection/query.rs` and its `recursive` and
`expressions` modules, together with the expression binder/evaluator.
It uses owned Rust values with `forbid(unsafe_code)` and does not invoke native
SQLite or the legacy translation.

## Remaining differences and bounds

Row-value subqueries, `value IN table_name` shorthand, windows, the
optimizing planner and index access paths remain unimplemented. Aggregates owned
by an outer query, such as `SELECT (SELECT sum(t.x)) FROM t`, are rejected with an
unsupported error; relocating these aggregates into the owning query is pending.
FROM sources cannot reference siblings as if they were lateral subqueries.
Parenthesized join groups preserve the tested native alias, wildcard and rowid
namespaces; [JOINS.md](JOINS.md) describes their materialization limits and ON
dependency rules.

Derived tables, views and CTEs are materialized. AS MATERIALIZED and AS NOT MATERIALIZED
both currently use materialization. Their distinct cache-sharing behavior across
DML and RETURNING is described in [RETURNING.md](RETURNING.md). Query flattening,
coroutines and general streaming remain unfinished. Evaluation and error timing
can therefore differ from native SQLite, particularly when an outer predicate or
LIMIT could avoid evaluating a source expression. An infinite recursive CTE with
an outer filter, join, sort, DISTINCT or aggregate can reach the execution budget
even when native SQLite would stop after producing enough outer rows. Put a
termination condition or a LIMIT inside the recursive CTE for bounded execution
on this engine.

Materialization also affects expression subqueries. For example, an EXISTS query
over a derived table can evaluate expressions in that table which native SQLite
would skip. Scalar/EXISTS consumers stop direct VALUES and unordered UNION ALL
sources early, but this does not provide general streaming through every nested
source or filter. Exact error timing and evaluation counts are not yet compatible
for all query shapes.

INSERT SELECT materializes its source before insertion. Functions observing
connection changes during that source query can differ from native coroutine
execution. INSERT VALUES evaluates each row before insertion unless a subquery
reads the destination table; such input is materialized before any row is inserted.
Exact representative selection among collation-equivalent compound rows has
tested cases, but full equivalence across native planner choices is not established.

Parser and runtime query depth are capped at 32 and by `max_expr_depth`.
Execution depth also covers dependencies between separate CTE definitions,
including recursive terms and stored view chains. Each WITH has at most 2,000 CTEs and each compound
query at most 500 terms. Rows, values, materialized results, queues and seen sets
are bounded by SQL limits. Captured column metadata is shared, accounted to the
query byte budget, and deduplicated with execution fuel. Nested outer-row captures
also share a logical byte limit. Materialization charging is conservative and cumulative
within an execution; these logical budgets do not measure total allocator/RSS use.
An execution fuel budget bounds recursive expansion, duplicate comparisons and
queue selection. Ordered queue selection currently scans the queue.

## Evidence

Five Rust integration tests in `tests/queries.rs` cover scopes, source metadata,
compound comparison, recursive ordering/limits/cycles, statement-local prepared
bindings, rollback, malformed query prefixes and resource limits. The depth test
includes a chain of 80 recursive CTE dependencies without syntactically nested
definitions. The platform tests also commit, reopen and roll back CTE data changes.

Five more integration tests in `tests/subqueries.rs` cover scalar/EXISTS/IN
semantics, nearest outer-row resolution, statement-local caches, prepared
bindings, rollback, malformed prefixes and resource exhaustion. The platform
tests persist correlated updates and verify unchanged files after a failed
subquery insert.

Four integration tests in `tests/query_types.rs` check compound metadata, nested
type spelling, VALUES/recursive collations, numeric value preservation and scalar
versus IN coercion. `scripts/type_differential.py` passes 508 comparisons against
SQLite 3.53.4, including a 21-by-21 expression matrix and large integers that
distinguish numeric comparison from REAL-affinity set keys.

`python3 safe/scripts/query_differential.py` passes 275 cases against the pinned
SQLite 3.53.4 reference, including UTF-8/UTF-16LE/UTF-16BE source images and exact
typed values. `python3 safe/scripts/subquery_differential.py` adds 258 comparisons
for expression subqueries and correlated data changes, including all three text
encodings. These comparisons supplement the existing SQL, constraint, pragma
and index suites. The public extension and upstream utility inventories are
unchanged and remain unfinished.
