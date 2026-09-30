# Aggregate input filters and ordering

The safe SQL engine supports `count`, `sum`, `total`, `avg`, aggregate `min` and
`max`, `group_concat` and `string_agg`, with `FILTER (WHERE ...)`, input `ORDER BY`
and single-argument `DISTINCT`. The parser also accepts the explicit `ALL`
argument modifier. These features use the Rust evaluator and do not call native
SQLite. Window functions and aggregate extension functions remain unfinished.

```sql
CREATE TABLE events(category TEXT, label TEXT, position INT, visible INT);
INSERT INTO events VALUES('build','test',2,1),('build','compile',1,1),
                         ('build','debug',3,0);
SELECT category,
       group_concat(label, ' -> ' ORDER BY position) FILTER (WHERE visible),
       count(*) FILTER (WHERE visible)
FROM events GROUP BY category;
```

## Evaluation and name resolution

Each aggregate has its own filter. A false or NULL filter skips both arguments
and ordering expressions for that row. Included rows evaluate their ordering
keys and arguments before duplicate elimination. The order is stable for equal
keys in the engine's scan order; ASC/DESC, NULLS FIRST/LAST and expression
collations use the same comparisons as query sorting. An integer ordering term
inside an aggregate is a constant expression, not a result-column ordinal.
Separators in `group_concat`/`string_agg` remain associated with their input row.

FILTER and input ordering accept supported expression subqueries and correlated
columns. They participate in name resolution, dependency tracking, expression
depth limits and the checks prohibiting nested aggregates. Scalar functions
reject FILTER and nonempty input ordering. Aggregate contexts remain prohibited
in WHERE, GROUP BY, generated columns and index expressions. Aggregates owned by
an outer query are still unsupported; see [QUERIES.md](QUERIES.md).

SQLite ignores an ORDER BY attached to a call with no arguments before resolving
its ordering names. It also validates but does not evaluate the ordering terms
on aggregate min/max. The implementation reproduces these behaviors. The star
argument does not accept ORDER BY, DISTINCT or ALL.

## DISTINCT, collations and bare columns

DISTINCT compares argument values using their storage classes and argument
collation. With an independent ordering key, the first equivalent argument is
retained, then the surviving inputs are sorted. When the sole ordering expression
matches the sole DISTINCT argument, SQLite uses a unique sorting key and keeps
the last equivalent payload. This can affect text spelling under NOCASE and
INTEGER versus REAL storage classes. The tested cases reproduce that distinction.

An explicit collation in an argument can affect the aggregate result's
comparison. Collations used only in FILTER or input ordering do not propagate to
that result. Filters, ordering keys and result comparisons remain separate.

A min/max step also determines the row supplying bare, non-grouped columns.
Filtered steps, NULLs, duplicates and multiple min/max calls affect that row.
The scan-based implementation reproduces the tested native accumulator-selection
rules, including all-NULL filtered groups and repeated aggregate expressions.
Native planner choices can select different representatives or input orders;
these checks do not establish equivalence for every plan or undefined tie choice.

## Bounds and remaining differences

Groups and ordered aggregate inputs are materialized. Values, keys and distinct
sets consume logical byte budgets, and scans, sorting and comparisons consume
execution fuel. Charging is conservative, including replaced DISTINCT payloads;
it is not an allocator/RSS guarantee. There are at most 2,000 ordering terms,
subject also to the SQL token and expression limits. Failed writes retain the
existing statement/transaction isolation behavior.

This does not add a query optimizer, index access paths, streaming aggregates,
window execution, user-defined functions or public aggregate extensions such as
percentile/decimal/JSON aggregates. Aggregates can be evaluated again for result,
HAVING, ordering and bare-column selection; native evaluation counts and exact
error timing are not fully reproduced. The general query materialization limits
in [QUERIES.md](QUERIES.md) continue to apply.

## Verification

```sh
cargo test -p sqlite-safe-core --test aggregate_modifiers
python3 safe/scripts/aggregate_differential.py
python3 platform/scripts/file_differential.py
```

The independent SQLite 3.53.4 oracle checks 1,110 scenarios covering aggregate filters, ordered and distinct
inputs, collations, result types, exact REAL bits, empty groups, aliases,
correlations, malformed syntax, forbidden aggregate contexts, bare-column
selection, data changes and stored views. Image comparisons span all eight page
sizes, three text encodings and three auto-vacuum modes, including native changes
and Rust reimport/export.

Six Rust tests additionally cover prepared bindings, skipped errors, invalid-name
validation, memory limits, malformed prefixes, rollback and image snapshots.
One Unix test and nine native file cases cover persisted aggregate views,
INSERT SELECT, rollback and failed-statement isolation.
These checks establish the recorded aggregate subset, not completion of the
SQLite rewrite.

The syntax and general behavior are documented in SQLite's
[aggregate function reference](https://www.sqlite.org/lang_aggfunc.html).
Version-specific DISTINCT and bare-column rules are checked against the pinned
3.53.4 source and executable, independently from the safe implementation.
