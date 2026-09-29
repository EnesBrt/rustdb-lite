# UPSERT

INSERT and REPLACE now support `ON CONFLICT ... DO NOTHING` and
`ON CONFLICT ... DO UPDATE SET ... [WHERE ...]` on the supported rowid and [WITHOUT ROWID](WITHOUT_ROWID.md) tables.
The implementation remains safe Rust; native SQLite is used only as a test
reference.

```sql
CREATE TABLE stock(id INTEGER PRIMARY KEY, item TEXT UNIQUE, quantity INTEGER);
INSERT INTO stock(item,quantity) VALUES('bolts',10),('nuts',20);
INSERT INTO stock(item,quantity) VALUES('bolts',5),('washers',30)
ON CONFLICT(item) DO UPDATE SET quantity=quantity+excluded.quantity;
```

For the conflicting item, `quantity` reads the existing row and
`excluded.quantity` reads the proposed insertion after affinity and applicable
NOT NULL default substitution. This example leaves 15 bolts, 20 nuts and 30
washers. The update does not change `last_insert_rowid()`; the new insertion does.

## Targets and actions

- Targets resolve against INTEGER PRIMARY KEY/rowid or an explicit/automatic
  UNIQUE index. Composite targets can reorder columns and include ASC/DESC.
  Explicit COLLATE must match the index; an omitted COLLATE can match an index's
  collation. A target that does not identify a unique constraint is an error.
  The pinned reference rejects composite targets containing an INTEGER PRIMARY
  KEY alias even when a redundant composite unique index exists; this behavior
  is preserved.
- Multiple clauses run in their written order, with at most one action per
  proposed row. An optional final clause without a target catches other unique
  conflicts. DO NOTHING and a false/NULL DO UPDATE WHERE skip the row and do not
  fall through to the next clause. Redundant targets use the first clause.
- Targeted unique checks run before other unique checks. NOT NULL and CHECK
  checks still occur before UPSERT and are not caught by it. Normal conflict
  policies apply to violations outside the selected UPSERT action.
- DO UPDATE always uses ABORT for constraint failures, including when the INSERT
  uses OR IGNORE, OR FAIL, OR ROLLBACK or OR REPLACE. An error restores the whole
  statement while preserving an enclosing transaction and its savepoints.
- Scalar assignments, parameters, correlated scalar/EXISTS/IN expressions and
  outer WITH scopes use the existing expression engine. Unqualified columns
  refer to the existing row; proposed columns require the `excluded` qualifier.
  `INSERT INTO table AS alias` is supported. A real target named or aliased
  `excluded` shadows the pseudo-table, as in SQLite.
- VALUES and INSERT SELECT sources are supported. Use WHERE in a SELECT source
  when needed to disambiguate its joins from the following ON CONFLICT clause.
  DEFAULT VALUES does not accept UPSERT, matching SQLite's grammar.
- A target WHERE expression is resolved, but does not filter a matching full
  unique index. Partial-index creation and matching remain unsupported.

Each inserted or updated row contributes to changes and total_changes. Skipped
rows do not. Target/name resolution errors leave the previous changes count;
runtime errors apply statement rollback behavior. DO UPDATE can modify rowid,
but does not update last_insert_rowid. Changes persist through the same
[journaled API](PAGER.md) as other data modifications.

## Verification and remaining work

Five Rust tests cover prepared binding reuse, correlated excluded values,
conflict order, statement rollback, transaction/counter behavior, malformed
prefixes, execution budgets and encoded snapshot roundtrips. A Unix adapter test
covers commit/reopen and unchanged files after a failed update.
Target resolution and constraint dispatch consume the statement's execution
budget, including wide composite targets when an INSERT SELECT returns no rows.
The parser limits a statement to 1,000 UPSERT clauses.

```sh
python3 legacy/scripts/build_oracle.py
python3 safe/scripts/upsert_differential.py
python3 platform/scripts/file_differential.py
```

The UPSERT comparison contains 592 scenarios against SQLite 3.53.4, checking
outcomes, rows, value types, transaction state and counters after each statement,
and native integrity of UTF-8/UTF-16 images in all three auto-vacuum modes. Nine
additional real-file scenarios check native readers across persisted UPSERTs,
failed updates and explicit commits.

The RETURNING projection reports successful inserts and updates, with separate
expression caches for each update clause; see [RETURNING.md](RETURNING.md).

Expression/partial indexes, row-value assignment syntax, generated columns,
Triggers and foreign keys remain
unfinished. INSERT SELECT materializes input; query planning, streaming and the
evaluation/error-timing differences documented in [QUERIES.md](QUERIES.md) still
apply. This implementation is not evidence of complete upstream SQL parity.
