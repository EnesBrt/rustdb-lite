#!/usr/bin/env python3
"""ALTER TABLE RENAME, scoped references and persisted schemas vs SQLite 3.53.4."""
import itertools
import subprocess
import tempfile
from pathlib import Path
from conflict_differential import reference_library, compare
from index_differential import native
from sql_differential import ROOT, ORACLE, run_engine


def quote(name):
    return '"' + name.replace('"', '""') + '"'


def main():
    subprocess.run(['cargo', 'build', '--example', 'statement_probe', '--bin', 'sqlite-safe-sql'], cwd=ROOT, check=True)
    lib = reference_library()
    cases = 0

    def check(statements):
        nonlocal cases
        compare(lib, statements)
        cases += 1

    queries = [
        'SELECT * FROM t', 'SELECT t.* FROM t', 'SELECT t.a,t.b FROM t',
        'SELECT t.a FROM t AS t', 'SELECT q.a FROM t q',
        'SELECT t.a FROM (t)', 'SELECT t.a FROM (t) AS t',
        'SELECT t.a FROM ((t))', 'SELECT t.a FROM main.t',
        'SELECT (SELECT t.a) FROM t', 'SELECT t.a,(SELECT t.a FROM u AS t) FROM t',
        'SELECT t.a FROM t WHERE t.a IN (SELECT a FROM t)',
        'SELECT t.a FROM t WHERE t.a IN one',
        'SELECT t.a FROM t UNION ALL SELECT t.a FROM t ORDER BY t.a',
        'SELECT p.a FROM (t JOIN u USING(a)) AS p',
        'SELECT t.a FROM (t JOIN u USING(a))',
        'SELECT t.a FROM t JOIN (u JOIN u AS x USING(a)) ON u.a=t.a',
        'SELECT * FROM (SELECT t.a FROM t) AS t WHERE t.a>0',
        'SELECT t.* FROM (SELECT t.a FROM t) t',
        'SELECT t.a FROM t JOIN u ON t.a=u.a',
        'SELECT t.a FROM u LEFT JOIN t ON t.a=u.a',
        'SELECT t.a FROM u FULL JOIN t USING(a)',
        'SELECT t.a FROM t NATURAL JOIN u',
        'SELECT t.a,count(*) FROM t GROUP BY t.a HAVING t.a>0 ORDER BY t.a',
        'SELECT sum(t.a) FILTER(WHERE t.b>0) FROM t',
        'SELECT group_concat(t.b ORDER BY t.a) FROM t',
        'SELECT CASE WHEN t.a>0 THEN t.b ELSE 0 END FROM t',
        'SELECT t.a FROM t WHERE EXISTS(SELECT 1 FROM u WHERE u.a=t.a)',
        'WITH t AS (SELECT 7 AS a) SELECT t.a FROM t',
        'WITH t AS (SELECT 7 AS a) SELECT t.a FROM main.t',
        'WITH x AS (SELECT 7 AS a) SELECT t.a FROM t',
        'WITH q AS (SELECT t.a FROM t) SELECT q.a FROM q',
        'WITH unused AS (SELECT t.a FROM t) SELECT 1',
        'WITH unused AS (SELECT t.missing FROM t) SELECT 1',
        'WITH unused AS (SELECT missing FROM absent) SELECT 1',
        'WITH q(a) AS (SELECT a FROM t UNION ALL SELECT a+1 FROM q WHERE a<3) SELECT * FROM q',
        'WITH t(a) AS (VALUES(1) UNION ALL SELECT a+1 FROM t WHERE a<3) SELECT * FROM t',
        'SELECT t.a,(WITH t(a) AS (VALUES(7)) SELECT t.a FROM t) FROM t',
        'SELECT * FROM absent', 'SELECT missing FROM t',
        'SELECT t.a FROM t,u AS t', 'SELECT a FROM t,u',
    ]
    for schema, query, populated in itertools.product(['', ' STRICT', ' WITHOUT ROWID'], queries, [False, True]):
        initial = [f'CREATE TABLE t(a INT PRIMARY KEY,b INT){schema}', 'CREATE TABLE u(a,b)', 'CREATE TABLE one(a)']
        if populated:
            initial += ['INSERT INTO t VALUES(1,2),(2,3)', 'INSERT INTO u VALUES(1,3)', 'INSERT INTO one VALUES(1)']
        check(initial + [f'CREATE VIEW v AS {query}', 'ALTER TABLE main.t RENAME TO x',
            'SELECT * FROM v', 'SELECT * FROM t ORDER BY a', 'SELECT * FROM x ORDER BY a',
            'PRAGMA table_xinfo(x)', 'PRAGMA index_list(x)'])
    for old, new, schema in itertools.product(['t', 'é🦀', 'a"b', 'with space'], ['x', 'select', 'a"new', 'é 🦀', ''], ['', ' STRICT', ' WITHOUT ROWID']):
        a, b = quote(old), quote(new)
        check([f'CREATE TABLE {a}(id INT PRIMARY KEY,a INT,b TEXT,g INT AS(a+1)) {schema}',
            f'INSERT INTO {a}(id,a,b) VALUES(1,2,\'text\')',
            f'CREATE INDEX ix ON {a}((a+g)) WHERE {a}.a>0',
            f'CREATE VIEW v AS SELECT {a}.* FROM {a}',
            f'ALTER TABLE {a} RENAME TO {b}', 'SELECT * FROM v', f'SELECT * FROM {b}',
            f'PRAGMA index_list({b})', 'PRAGMA index_xinfo(ix)', f'INSERT INTO {b}(id,a,b) VALUES(2,3,\'next\')',
            f'SELECT * FROM {b} ORDER BY id'])
    for query in [
        'SELECT (SELECT t.a FROM u AS t) FROM t',
        'SELECT (SELECT t.rowid FROM u AS t) FROM t',
        'SELECT (WITH q AS (SELECT t.a FROM t) SELECT * FROM q) FROM t',
        'SELECT t.a+1 FROM t UNION ALL SELECT t.a+1 FROM t ORDER BY t.a+1',
        'WITH unused AS (SELECT t.a, t.missing FROM t) SELECT 1',
        'WITH unused AS (SELECT t.missing, t.a FROM t) SELECT 1',
        'WITH unused AS (SELECT * FROM t WHERE t.missing) SELECT 1',
        'SELECT t.a IN () FROM t',
    ]:
        check(['CREATE TABLE t(a)', 'CREATE TABLE u(b)', 'INSERT INTO t VALUES(7)',
            'INSERT INTO u VALUES(9)', f'CREATE VIEW v AS {query}',
            'ALTER TABLE t RENAME TO renamed', 'SELECT * FROM v',
            'ALTER TABLE renamed RENAME TO again', 'SELECT * FROM v'])
    for name in ['t', 'T', 'u', 'v', 'ix', 'sqlite_reserved', 'sqlite_sequence']:
        check(['CREATE TABLE t(a)', 'CREATE TABLE u(a)', 'CREATE VIEW v AS SELECT * FROM t',
            'CREATE INDEX ix ON t(a)', f'ALTER TABLE t RENAME TO {name}', 'INSERT INTO t VALUES(1)', 'SELECT * FROM v'])
    check(['CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,a UNIQUE)', 'INSERT INTO t(a) VALUES(4)',
        "INSERT INTO sqlite_sequence VALUES('t',99),('T',123),('other',888)", 'BEGIN',
        'ALTER TABLE t RENAME TO x', 'SELECT * FROM sqlite_sequence ORDER BY rowid',
        'INSERT INTO x(a) VALUES(5)', 'SAVEPOINT s', 'ALTER TABLE x RENAME TO y',
        'INSERT INTO y(a) VALUES(6)', 'ROLLBACK TO s', 'SELECT * FROM x', 'ROLLBACK',
        'SELECT * FROM t', 'SELECT * FROM sqlite_sequence ORDER BY rowid'])
    check(['CREATE TABLE seed(id INTEGER PRIMARY KEY AUTOINCREMENT)', 'CREATE TABLE t(a)',
        "INSERT INTO sqlite_sequence VALUES('t',7)", 'ALTER TABLE t RENAME TO x', 'SELECT * FROM sqlite_sequence'])
    with tempfile.TemporaryDirectory(prefix='safe-rename-') as folder:
        folder = Path(folder)
        # Exact SQL matters as well as executable results: preserve aliases,
        # literals, comments, unused CTEs and unrelated schema declarations.
        valid_queries = queries[:-4] + [
            'WITH unused AS (SELECT t.a,t.missing FROM t) SELECT 1',
            'WITH unused AS (SELECT t.missing,t.a FROM t) SELECT 1',
            'WITH unused AS (SELECT * FROM t WHERE t.missing) SELECT 1',
        ]
        for n, query in enumerate(valid_queries):
            source, saved = folder/f'query-{n}.db', folder/f'query-{n}-saved.db'
            native(source, f'CREATE TABLE t(a INT PRIMARY KEY,b);CREATE TABLE u(a,b);CREATE TABLE one(a);CREATE VIEW v AS {query};')
            expected = folder/f'query-{n}-expected.db'
            expected.write_bytes(source.read_bytes())
            result = subprocess.run([str(ORACLE), str(expected)], input='ALTER TABLE t RENAME TO renamed', text=True, capture_output=True, timeout=30)
            if result.returncode:
                run_engine('ALTER TABLE t RENAME TO renamed', '--load', source, '--save', saved, fail=True)
                assert expected.read_bytes() == source.read_bytes(), query
                cases += 1
                continue
            run_engine('ALTER TABLE t RENAME TO renamed', '--load', source, '--save', saved)
            source = expected
            for q in ['SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name', 'SELECT * FROM v', 'PRAGMA integrity_check']:
                assert native(source, q) == native(saved, q), (query, q, native(source, q), native(saved, q))
            cases += 1
        for n, (size, encoding, mode, wr) in enumerate(itertools.product(
                [512,1024,2048,4096,8192,16384,32768,65536], ['UTF-8','UTF-16le','UTF-16be'], [0,1,2], [False,True])):
            source, saved, final = [folder/f'{n}-{part}.db' for part in ['source','saved','final']]
            native(source, f"PRAGMA page_size={size};PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(id INT PRIMARY KEY,a INT CHECK(t.a>0),s INT AS(a*2) STORED,g INT AS(s+1),UNIQUE(a)) STRICT{' , WITHOUT ROWID' if wr else ''};INSERT INTO t(id,a) VALUES(1,2),(2,3);CREATE INDEX ix ON t((s+g)) WHERE t.a>0;CREATE VIEW v AS SELECT t.* FROM t;CREATE VIEW w AS SELECT id,s,g FROM v;")
            sql = 'ALTER TABLE t RENAME TO "é new🦀";'
            run_engine(sql, '--load', source, '--save', saved)
            native(source, sql)
            queries_for_image = ['SELECT * FROM v ORDER BY id', 'SELECT * FROM w ORDER BY id', 'PRAGMA index_xinfo(ix)', 'SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name', 'PRAGMA integrity_check']
            for q in queries_for_image:
                assert native(saved,q) == native(source,q), (n,q)
            native(saved, 'ALTER TABLE "é new🦀" RENAME TO again;INSERT INTO again(id,a) VALUES(3,4);')
            sql = 'ALTER TABLE again RENAME TO final;UPDATE final SET a=a+10;'
            run_engine(sql, '--load', saved, '--save', final)
            native(saved, sql)
            for q in queries_for_image:
                assert native(final,q) == native(saved,q), (n,q)
            cases += 1
    print(f'PASS: {cases} table rename/scope/metadata/transaction/image scenarios against SQLite 3.53.4')


if __name__ == '__main__':
    main()
