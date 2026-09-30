# Join execution and merged columns

The safe engine implements comma, INNER, CROSS, LEFT, RIGHT and FULL joins, with
ON, USING and NATURAL clauses. OUTER is optional. Joins associate left to right,
including mixed comma/explicit joins. Supported SQLite keyword combinations,
such as LEFT RIGHT JOIN and LEFT NATURAL JOIN, retain the native meaning;
contradictory combinations such as LEFT INNER JOIN are rejected.

```sql
CREATE TABLE previous(id INT, old_value TEXT);
CREATE TABLE current(id INT, new_value TEXT);
INSERT INTO previous VALUES(1,'before'),(2,'unchanged');
INSERT INTO current VALUES(2,'unchanged'),(3,'after');
SELECT id, old_value, new_value
FROM previous FULL OUTER JOIN current USING(id)
ORDER BY id;
```

## Matching and outer rows

ON is evaluated for each candidate pair before outer-row extension. LEFT emits
an unmatched left row with NULL right columns; RIGHT emits unmatched right rows
with NULL preceding columns; FULL performs both. NULL join keys do not compare
equal. Duplicate keys produce all matching pairs. WHERE filters the resulting
rows after extension. Empty sides, chained joins, generated columns, aggregates,
CTEs, views, scalar/EXISTS/IN subqueries and INSERT SELECT use these same rows.

Generated columns of an absent side are NULL, including expressions that would
otherwise generate a non-NULL constant. Correlated subqueries retain the proper
physical row slots and nearest outer scope. An ON predicate can contain supported
correlated subqueries. Each source is still materialized or scanned with nested
loops; no join optimizer, hash join or index access path is implemented.

## USING, NATURAL and metadata

USING requires real columns present on both sides; hidden rowids do not qualify.
NATURAL synthesizes the common column names and cannot also have ON or USING.
The left operand determines equality affinity/collation according to SQLite's
expression rules. NATURAL with no common columns behaves as an unconstrained
join, retaining the chosen outer-row behavior.

The unqualified output contains one copy of each USING column. INNER/LEFT uses
the left value, RIGHT the right value, and FULL coalesces the applicable values.
Join-generated coalescence inherits its first argument's affinity/collation;
user-written coalesce retains its existing function semantics. This distinction
affects comparisons, grouping, derived metadata and CREATE TABLE AS SELECT.
Explicit qualified column expressions continue to read their physical side.

SQLite's wildcard expansion has an additional rule: a qualified `table.*` can
use a merged value when the table precedes a RIGHT/FULL join and a later USING
clause names that column. The engine reproduces the tested cases, including
chains of three tables. Tests distinguish this from explicitly selecting
`table.column`. Name-resolution errors and SQLite's historical ambiguity rules
for INNER/LEFT versus RIGHT/FULL USING are also compared with the native engine.

## Limits and remaining work

Parenthesized join groups remain unsupported; parenthesized FROM sources must
be subqueries. ON currently resolves against preceding/current sources. Native
inner-join ON expressions referencing a source later in FROM still require work.
This is not full join-language or optimizer compatibility. Query materialization,
evaluation/error timing, unordered row selection and planner-dependent behavior
retain the limits in [QUERIES.md](QUERIES.md).

There are at most 64 joined sources. Result rows, field metadata, captured merged
column mappings and values are subject to SQL memory/row budgets. Name matching,
candidate evaluation and unmatched-right scans consume execution fuel. Generated
merged expressions obey the expression-depth cap. These bounds can reject work
that native SQLite accepts; they are not SQLite's configurable limits or an RSS
guarantee.

## Verification

```sh
cargo test -p sqlite-safe-core --test joins
python3 safe/scripts/join_differential.py
python3 platform/scripts/file_differential.py
```

The join differential suite checks 579 scenarios against SQLite 3.53.4: join
kinds, NULLs, duplicate matches, empty sides, types/collations, one/multiple USING
keys, wildcard expansion, correlated scopes, three-table chains, grouping, views,
invalid syntax, counters and image interchange. Native checks include all three
text encodings and auto-vacuum modes, native changes and Rust reimport/export.

Six Rust tests cover bindings, physical/merged values, NULL extension of virtual
columns, ON versus WHERE, persistent views and snapshots, rollback, row budgets,
malformed prefixes and 72 encoding/page-size/auto-vacuum combinations. A Unix
test and nine native real-file cases cover reopen, native readers, joined-view
INSERT SELECT, failed-statement isolation and explicit rollback.

These are additional checks of the safe implementation, not evidence that the
full SQLite rewrite, public extensions, upstream utilities or platforms are done.
