# LIKE, ESCAPE and GLOB

The safe SQL engine implements LIKE/NOT LIKE with an optional ESCAPE expression,
GLOB/NOT GLOB, and their scalar forms `like(pattern,text[,escape])` and
`glob(pattern,text)`. All matching executes in Rust, with no native engine or
third-party dependency. This closes another part of the experimental SQL surface;
the full SQLite rewrite remains incomplete.

```sql
CREATE TABLE names(id INT PRIMARY KEY, name TEXT);
INSERT INTO names VALUES(1,'A_one'),(2,'a_two'),(3,'É_été');
SELECT name FROM names WHERE name LIKE '%!_%' ESCAPE '!';
SELECT name FROM names WHERE name GLOB '[A-ZÉ]*';
```

## Semantics

LIKE uses `%` for any sequence and `_` for one character. It folds ASCII case;
non-ASCII case remains distinct. ESCAPE must convert to one character before the
first NUL. An escaped wildcard or ordinary character is matched literally, and
a trailing escape cannot match. The escape itself may be `%`, `_`, or a non-ASCII
character. An empty or multi-character escape raises an error even when the
pattern or input is NULL. A NULL escape returns NULL.

GLOB is case sensitive and uses `*`, `?`, bracket sets, ranges and `^` negation
inside sets. An initial `]` inside a set matches a literal closing bracket.
Malformed or unclosed sets do not match; no backslash escape is implied.
The bracket scanner also preserves the pinned native behavior of reversed and
chained ranges. Explicit SQL collations do not change pattern matching.

Pattern operators lower to scalar calls with the pattern argument first;
negation wraps the result and preserves NULL. ESCAPE precedence is handled by
the parser. Function validation rejects an ESCAPE argument on GLOB. Lazy
CASE/coalesce branches retain their existing evaluation rules. Prepared parameters,
correlated expressions and aggregate filters use the same functions.

Patterns and input stop matching at the first NUL. BLOB arguments convert to
text using the database encoding. The matcher handles native UTF-8 character
boundaries for malformed bytes without requiring valid Rust strings. UTF-16
conversion drops a trailing byte and preserves the pinned engine's surrogate
conversion behavior. These are matching-specific conversion rules; they do not
change the separate text codec's validation contract.

The functions are available to supported CHECK constraints, generated columns,
expression/partial indexes, UPSERT targets and stored views. Operator syntax and
the equivalent scalar function share the same schema expression structure.
Writes and failed statements use the existing transaction and journal paths.

## Resource bounds and unfinished scope

Matching is iterative. It retains pattern/input positions and a wildcard retry
position, with no recursive backtracking or matrix proportional to both input
lengths. Scanning and retrying consume the statement's execution budget, so
adversarial repeated-prefix patterns return a bounded error. UTF-16 conversion
is charged to the logical byte limit; these budgets do not establish an RSS cap.

The native default pattern limit of 50,000 bytes is enforced, including bytes
after an embedded NUL. Text patterns are measured in UTF-8; BLOB patterns use
their original byte count before conversion, following the pinned reference.
Pattern-limit and ESCAPE checks occur before returning a NULL result.
There is no API to change that limit yet. Existing SQL/value limits also apply.

The case_sensitive_like pragma, compile-time case/BLOB switches, ICU case folding,
function replacement callbacks and index-driven LIKE/GLOB search optimization
remain unimplemented. The public regexp/FTS/ICU extensions are not provided by
this matcher. The remaining SQL and platform limits in [SQL.md](SQL.md),
[QUERIES.md](QUERIES.md) and [STATUS.md](STATUS.md) still apply.

## Evidence

```sh
cargo test -p sqlite-safe-core --test patterns
python3 safe/scripts/pattern_differential.py
python3 platform/scripts/file_differential.py
```

The independent SQLite 3.53.4 oracle passes 9,607 value, error, schema and image
scenarios. They cover literal/class/wildcard combinations, Unicode and ASCII
case, NULLs, escape validation, precedence, malformed UTF bytes, deterministic
random patterns, the 50,000-byte boundary, generated columns and UPSERT targets.
Image interchange covers all 72 combinations of eight page sizes, three text
encodings and three auto-vacuum modes, including native mutation and Rust reopen.

Six Rust tests cover prepared parameters, NUL/BLOB boundaries, 10,000 successive
wildcard/literal pairs without recursive matching, deeply nested parser input,
fuel and length limits, rollback and all 72 image configurations. One Unix test
and nine native file scenarios cover persisted views, generated values and
unchanged files after rollback or an invalid ESCAPE expression.

The implementation was checked against the pinned native `func.c`, `utf.c`, and
`parse.y`, together with SQLite's [expression syntax](https://www.sqlite.org/lang_expr.html)
and [scalar function reference](https://www.sqlite.org/lang_corefunc.html).
