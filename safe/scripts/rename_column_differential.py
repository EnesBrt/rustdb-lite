#!/usr/bin/env python3
"""Column rename bindings, quote normalization and persisted SQL vs SQLite 3.53.4."""
import itertools
import subprocess
import tempfile
from pathlib import Path
from conflict_differential import reference_library, compare
from index_differential import native
from sql_differential import ROOT, ORACLE, run_engine
from rename_differential import quote

QUERIES = [
    'SELECT * FROM t', 'SELECT t.* FROM t', 'SELECT a FROM t', 'SELECT "a" FROM t',
    'SELECT t.a,t.b FROM t', 'SELECT a AS a FROM t ORDER BY a',
    'SELECT a AS x FROM t ORDER BY x', 'SELECT a FROM t ORDER BY a',
    'SELECT a FROM t UNION ALL SELECT a FROM t ORDER BY a',
    'SELECT a AS a FROM t UNION ALL SELECT a FROM t ORDER BY a',
    'SELECT a+1 FROM t UNION ALL SELECT a+1 FROM t ORDER BY a+1',
    'SELECT * FROM t UNION ALL SELECT * FROM t ORDER BY a',
    'SELECT a,* FROM t UNION ALL SELECT a,* FROM t ORDER BY a',
    'SELECT a+1 FROM t UNION ALL SELECT a+1 FROM t ORDER BY "a+1"',
    'SELECT * FROM t ORDER BY a', 'SELECT * FROM t ORDER BY t.a',
    'SELECT *,a FROM t ORDER BY a', 'SELECT a,* FROM t ORDER BY a',
    'SELECT a,* FROM t GROUP BY a', 'SELECT * FROM t ORDER BY a+0',
    'SELECT a AS x,* FROM t ORDER BY x',
    'SELECT a FROM t GROUP BY a HAVING a>0 ORDER BY a',
    'SELECT a AS a FROM t GROUP BY a HAVING a>0 ORDER BY a',
    'SELECT count(*) FROM t WHERE a>0', 'SELECT sum(a) FILTER(WHERE b>0) FROM t',
    'SELECT group_concat(b ORDER BY a) FROM t', 'SELECT (SELECT a) FROM t',
    'SELECT (SELECT t.a FROM u AS t) FROM t', 'SELECT t.a FROM t AS t',
    'SELECT q.a FROM t q', 'SELECT t.a FROM main.t', 'SELECT t.a FROM (t)',
    'SELECT p.a FROM (t JOIN u USING(b)) AS p', 'SELECT t.a FROM t JOIN u ON t.a=u.b',
    'SELECT a FROM t JOIN u USING(b)', 'SELECT a FROM t NATURAL JOIN u',
    'SELECT t.a FROM t WHERE EXISTS(SELECT 1 FROM u WHERE u.b=t.a)',
    'SELECT * FROM (SELECT a FROM t) q', 'SELECT q.a FROM (SELECT a FROM t) q',
    'SELECT q."a" FROM (SELECT a FROM t) q', 'SELECT "a" FROM (SELECT a FROM t) q',
    'WITH q AS (SELECT a FROM t) SELECT * FROM q',
    'WITH q AS (SELECT a FROM t) SELECT a FROM q',
    'WITH q(a) AS (SELECT a FROM t) SELECT a FROM q',
    'WITH q AS (SELECT a FROM t UNION ALL SELECT a+1 FROM q WHERE a<3) SELECT * FROM q',
    'WITH q(a) AS (SELECT a FROM t UNION ALL SELECT a+1 FROM q WHERE a<3) SELECT * FROM q',
    'WITH unused AS (SELECT a FROM t) SELECT 1',
    'WITH unused AS (SELECT a,missing FROM t) SELECT 1',
    'WITH unused AS (SELECT missing,a FROM t) SELECT 1',
    'WITH unused AS (SELECT "future",a FROM t) SELECT 1',
    'WITH t(a) AS (VALUES(7)) SELECT a FROM t',
    'SELECT "constant",a FROM t', 'SELECT ("constant"),a FROM t',
    'SELECT "future",a FROM t', 'SELECT [missing] FROM t',
    'SELECT `missing` FROM t', 'SELECT missing FROM t', 'SELECT * FROM absent',
]


def main():
    subprocess.run(['cargo', 'build', '--example', 'statement_probe', '--bin', 'sqlite-safe-sql'], cwd=ROOT, check=True)
    lib = reference_library()
    cases = 0

    def check(statements):
        nonlocal cases
        compare(lib, statements)
        cases += 1

    for query, new, suffix, populated in itertools.product(QUERIES, ['future', '"a b"'], ['', ' STRICT', ' WITHOUT ROWID'], [False, True]):
        initial = [f'CREATE TABLE t(a INT PRIMARY KEY,b INT){suffix}', 'CREATE TABLE u(b INT)']
        if populated:
            initial += ['INSERT INTO t VALUES(1,2),(2,3)', 'INSERT INTO u VALUES(2)']
        check(initial + [f'CREATE VIEW v AS {query}', f'ALTER TABLE t RENAME a TO {new}',
            'SELECT * FROM t ORDER BY 1', 'SELECT * FROM v', 'PRAGMA table_xinfo(v)', 'PRAGMA table_xinfo(t)'])
    for schema, index in itertools.product([
        'a', 'a INTEGER PRIMARY KEY', 'a INT PRIMARY KEY', 'a UNIQUE',
        'a,b,UNIQUE(a,b)', 'a CHECK(a>0)', 'a,b AS(a*2) STORED,c AS(b+1)',
        'a,b AS("future")', 'a CHECK("constant")', 'a DEFAULT "default"',
        'a DEFAULT ("default")', 'a CHECK(t.a>0)',
    ], ['', 'CREATE INDEX ix ON t(a)', 'CREATE INDEX ix ON t(\'a\')',
        'CREATE INDEX ix ON t("a") WHERE a>0', 'CREATE INDEX ix ON t(a+1) WHERE t.a>0',
        'CREATE INDEX ix ON t("literal")', 'CREATE INDEX ix ON t(a) WHERE "literal"']):
        check([f'CREATE TABLE t({schema})', *([index] if index else []), 'INSERT INTO t(a) VALUES(1)',
            'ALTER TABLE t RENAME COLUMN a TO future', 'SELECT rowid,* FROM t', 'PRAGMA table_xinfo(t)',
            'PRAGMA index_xinfo(ix)', 'INSERT INTO t(future) VALUES(2)', 'SELECT * FROM t ORDER BY 1'])
    for name in ['a', 'A', 'b', 'rowid', '_rowid_', 'oid', 'sqlite_sequence', '""', '"a b"', '"a""b"', '"é🦀"']:
        check(['CREATE TABLE t(a INTEGER PRIMARY KEY AUTOINCREMENT,b UNIQUE)', 'INSERT INTO t VALUES(7,2)',
            'CREATE VIEW v AS SELECT rowid,oid,_rowid_,a,b FROM t',
            f'ALTER TABLE t RENAME COLUMN a TO {name}', 'SELECT * FROM v', 'SELECT * FROM sqlite_sequence',
            'PRAGMA table_xinfo(t)', 'INSERT INTO t(b) VALUES(3)', 'SELECT * FROM v'])
    for statement in ['ALTER TABLE t RENAME rowid TO x', 'ALTER TABLE t RENAME absent TO x',
            'ALTER TABLE absent RENAME a TO x', 'ALTER TABLE v RENAME a TO x',
            'ALTER TABLE sqlite_sequence RENAME seq TO x']:
        check(['CREATE TABLE t(a INTEGER PRIMARY KEY AUTOINCREMENT)', 'CREATE VIEW v AS SELECT * FROM t',
            'INSERT INTO t VALUES(4)', statement, 'SELECT * FROM t', 'SELECT * FROM v'])
    check(['CREATE TABLE t(a INT UNIQUE,b AS(a*2) STORED)', 'INSERT INTO t(a) VALUES(1)',
        'CREATE VIEW v AS SELECT a AS x,b FROM t', 'BEGIN', 'ALTER TABLE t RENAME a TO c',
        'INSERT INTO t(c) VALUES(2)', 'SAVEPOINT s', 'ALTER TABLE t RENAME b TO d',
        'SELECT * FROM v', 'ROLLBACK TO s', 'SELECT * FROM t', 'ROLLBACK', 'SELECT * FROM v'])
    for expression in ['"text"', '("text")', '"st""r"', '[text]', '`text`', '""']:
        check([f'SELECT {expression}', f'SELECT {expression} AS value', 'CREATE TABLE t(a INTEGER PRIMARY KEY,b)',
            'INSERT INTO t VALUES(1,2)', f'SELECT {expression},A,RowId,oid,_rowid_ FROM t'])
    with tempfile.TemporaryDirectory(prefix='safe-column-rename-') as directory:
        folder = Path(directory)
        for n, query in enumerate(QUERIES):
            source, saved, expected = [folder/f'view-{n}-{part}.db' for part in ['source','saved','expected']]
            native(source, f'CREATE TABLE t(a INT PRIMARY KEY,b INT);CREATE TABLE u(b INT);CREATE VIEW v AS {query};')
            expected.write_bytes(source.read_bytes())
            sql = 'ALTER TABLE t RENAME a TO future'
            result = subprocess.run([str(ORACLE), str(expected)], input=sql, text=True, capture_output=True, timeout=30)
            if result.returncode:
                run_engine(sql, '--load', source, '--save', saved, fail=True)
                assert expected.read_bytes() == source.read_bytes(), query
            else:
                run_engine(sql, '--load', source, '--save', saved)
                for q in ['SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name', 'SELECT * FROM v', 'PRAGMA integrity_check']:
                    assert native(expected,q) == native(saved,q), (query,q,native(expected,q),native(saved,q))
            cases += 1
        for n, (old, new) in enumerate(itertools.product(['a', '[a]', '`a`', '"a"', "'a'"], ['future', '"future"', '[é 🦀]', '`x"y`', "'single'", '""'])):
            source, saved = folder/f'quotes-{n}.db', folder/f'quotes-{n}-saved.db'
            native(source, f'CREATE TABLE t({old} INTEGER PRIMARY KEY,b CHECK({old}>0),c AS({old}+1),UNIQUE({old},b));CREATE INDEX ix ON t(\'a\');CREATE VIEW v AS SELECT "a",[a],`a`,a,"literal\'s" FROM t;CREATE TABLE u(x CHECK("text"),y AS("future"),z DEFAULT "default");')
            sql = f'ALTER TABLE t RENAME a TO {new}'
            run_engine(sql, '--load', source, '--save', saved)
            native(source, sql)
            q = 'SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name'
            assert native(source,q) == native(saved,q), (old,new,native(source,q),native(saved,q))
            assert native(saved,'PRAGMA integrity_check') == [{'integrity_check':'ok'}]
            cases += 1
        for n, (size, encoding, mode, wr) in enumerate(itertools.product(
                [512,1024,2048,4096,8192,16384,32768,65536], ['UTF-8','UTF-16le','UTF-16be'], [0,1,2], [False,True])):
            source, saved, final = [folder/f'image-{n}-{part}.db' for part in ['source','saved','final']]
            native(source, f"PRAGMA page_size={size};PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(id INT PRIMARY KEY,a INT CHECK(t.a>0),s INT AS(a*2) STORED,g INT AS(s+1),UNIQUE(a)) STRICT{' , WITHOUT ROWID' if wr else ''};INSERT INTO t(id,a) VALUES(1,2),(2,3);CREATE INDEX ix ON t((a+s+g)) WHERE t.a>0;CREATE VIEW v AS SELECT id,a AS value,s,g FROM t;CREATE VIEW w AS SELECT * FROM v;")
            sql = 'ALTER TABLE t RENAME COLUMN a TO "é new🦀";'
            run_engine(sql, '--load', source, '--save', saved)
            native(source, sql)
            queries = ['SELECT * FROM v ORDER BY id', 'SELECT * FROM w ORDER BY id', 'PRAGMA index_xinfo(ix)', 'SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name', 'PRAGMA integrity_check']
            for q in queries:
                assert native(saved,q) == native(source,q), (n,q)
            native(saved, 'ALTER TABLE t RENAME "é new🦀" TO again;INSERT INTO t(id,again) VALUES(3,4);')
            sql = 'ALTER TABLE t RENAME again TO final;UPDATE t SET final=final+10;'
            run_engine(sql, '--load', saved, '--save', final)
            native(saved, sql)
            for q in queries:
                assert native(final,q) == native(saved,q), (n,q)
            cases += 1
    print(f'PASS: {cases} column rename/quote/scope/metadata/transaction/image scenarios against SQLite 3.53.4')


if __name__ == '__main__':
    main()
