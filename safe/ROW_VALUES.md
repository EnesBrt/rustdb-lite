# Row values and multi-column assignments

The safe SQL engine supports parenthesized row operands and multi-column
subqueries in comparisons, BETWEEN, CASE and IN/NOT IN. UPDATE and UPSERT can
assign several columns from a row expression or one subquery. This extends the
experimental SQL subset; it does not complete the SQLite rewrite.

```sql
CREATE TABLE coordinates(id INTEGER PRIMARY KEY, x INT, y INT);
INSERT INTO coordinates VALUES(1,10,20),(2,30,40);
SELECT id FROM coordinates WHERE (x,y) > (?1,?2) ORDER BY x,y;
UPDATE coordinates SET (x,y)=(y,x) WHERE (x,y) IN (VALUES(10,20));
```

## Comparisons and membership

Rows must have equal widths and scalar components. Equality, inequality, ordering,
IS and IS NOT use each component's affinity and collation. Ordinary comparisons
preserve unknown NULL results unless another component determines the outcome.
IS/IS NOT compare NULLs without an unknown result. Direct comparisons evaluate
components from left to right and can skip later expressions. BETWEEN and simple
CASE evaluate their entire starting row once before comparing alternatives.

A row subquery supplies its first result row, or a row of NULLs when it is empty.
Its components retain the metadata needed for comparisons. In particular, an
absent collation does not override the other operand's collation, and an explicit
COLLATE can take precedence. Scalar subqueries keep their existing separate
collation rules. Names and widths are validated even in unselected CASE branches.

IN accepts a query, a list of explicit row expressions, or table-name shorthand,
such as `(x,y) IN coordinates`. Shorthand reads the table's visible columns;
ordinary CTE and main-schema name resolution apply. It does not add table-valued
function arguments, attached databases or temporary schemas. A single nested
subquery on an IN list's right side supplies its complete result set. Lists of
rows follow the pinned native parser's conversion to a VALUES query.

Membership compares complete candidate rows. An unknown comparison with one
candidate does not hide a definite match with another. A definite unequal
component excludes that candidate even if another component is NULL. IN over an
empty set is false and NOT IN is true. The engine also reproduces tested empty
literal-list simplification, including discarded names and preserved aggregate
query classification. IN keys apply comparison affinity before encoded key
comparisons, which differs from direct scalar comparison in some type cases.

## Assignments and schema expressions

A tuple assignment checks its source width against the target column list.
Explicit components read the old row. A subquery's first row is shared by the
columns of that assignment during one updated row; correlated results are not
reused for a different updated row. An empty subquery supplies NULLs. Prepared
parameters, excluded UPSERT values, generated-column recomputation, conflict
policies, RETURNING and journaled persistence use the existing write paths.

When a column is assigned more than once, all names are bound but only its last
assignment executes. Rowid aliases and hidden rowid names identify the same
write target. A discarded expression therefore cannot raise a runtime error.
Row comparisons can also appear in supported CHECK/generated/index expressions;
row operands themselves cannot be stored as column values or projected directly.
Subqueries retain the existing restrictions in schema expressions.

## Bounds and remaining work

Row operands have at most 2,000 components and retain the SQL expression, token
and nesting bounds. Cached component values, assignment query results and query
column metadata consume logical byte budgets. Comparisons and queries share
execution fuel. The budgets are conservative and do not measure allocator/RSS
usage. Width mismatches and misuse in scalar contexts return errors.

The engine still materializes query sources, uses scans for membership and
constraints, and rebuilds database images. Index-driven row seeks, correlated
query planning, general streaming and complete native evaluation/error timing
remain unfinished. Native planner choices may also affect which row an unordered
subquery returns. The limits in [QUERIES.md](QUERIES.md) and [SQL.md](SQL.md) still
apply; the public extensions, upstream utilities and other platform adapters are
not completed by this change.

## Verification

```sh
cargo test -p sqlite-safe-core --test row_values
python3 safe/scripts/row_value_differential.py
python3 platform/scripts/file_differential.py
```

The independent SQLite 3.53.4 oracle passes 3,446 comparisons covering two- and
three-column NULL/type matrices, collations, direct and subquery operands,
BETWEEN/CASE, lists and query/table membership, correlations, tuple updates,
UPSERT, generated values, conflict policies, invalid shapes and schema indexes.
Image checks cover all eight page sizes, three encodings and three auto-vacuum
modes, including native mutation followed by Rust reimport/export.

Six Rust tests cover prepared bindings, short-circuit errors, shared assignments,
repeated targets, rollback, malformed prefixes, deeply nested expressions,
width/memory bounds and the same 72 image configurations. One Unix test and nine native file cases check persistent
membership views, tuple updates and unchanged files after rollback or failures.

SQLite's [row-value syntax reference](https://www.sqlite.org/rowvalue.html)
provides the general syntax. The implementation and tests also inspect the pinned
3.53.4 source for row-list parsing, per-column comparison and assignment behavior.
