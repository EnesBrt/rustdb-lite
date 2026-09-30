#!/usr/bin/env python3
"""DROP COLUMN schema-wide literal normalization and dependencies vs SQLite 3.53.4."""
import itertools
import subprocess
import tempfile
from pathlib import Path
from conflict_differential import reference_library, compare
from index_differential import native
from sql_differential import ROOT, ORACLE, run_engine


def main():
    subprocess.run(['cargo', 'build', '--example', 'statement_probe', '--bin', 'sqlite-safe-sql'], cwd=ROOT, check=True)
    lib = reference_library()
    cases = 0

    def check(statements):
        nonlocal cases
        compare(lib, statements)
        cases += 1

    views = []
    for expr, template in itertools.product(['"future"', '"b"', '"a"', '"a\'b"', '[future]', '`future`'], [
        'SELECT {e} FROM t', 'SELECT ({e}) AS value FROM t',
        'SELECT (SELECT {e}) AS value FROM t',
        'WITH q AS (SELECT {e} AS value FROM t) SELECT * FROM q',
        'WITH unused AS (SELECT {e} FROM t) SELECT a FROM t',
        'SELECT {e} AS value FROM t UNION ALL SELECT {e} FROM t ORDER BY value',
        'SELECT group_concat(a ORDER BY {e}) FILTER(WHERE {e}>0) FROM t',
    ]):
        views.append(template.format(e=expr))
    for view, suffix, populated in itertools.product(views, ['', ' STRICT', ' WITHOUT ROWID'], [False, True]):
        check([f'CREATE TABLE t(a INT PRIMARY KEY,b INT){suffix}']
            + (['INSERT INTO t VALUES(1,2),(2,3)'] if populated else [])
            + [f'CREATE VIEW v AS {view}', 'SELECT * FROM v', 'ALTER TABLE t DROP b',
               'SELECT * FROM v', 'PRAGMA table_xinfo(t)', 'ALTER TABLE t ADD future TEXT DEFAULT \'capture\'',
               'SELECT * FROM v', 'SELECT * FROM t ORDER BY a'])
    schemas = [
        'a,b,g AS("future")', 'a,b,g AS("future") STORED', 'a CHECK("future">0),b',
        'a,b,CHECK("future">0)', 'a CHECK("b">0),b', 'a,b,CHECK("b">0)',
        'a,b CHECK("b">0)', 'a,b CHECK("future">0)', 'a,b,g AS("b")',
        'a,b,g AS("b") STORED', 'a,b,g AS(a+1),CHECK("g">0)',
        'a DEFAULT "future",b', 'a,b,g AS("future"),h AS(g||"suffix")',
    ]
    indexes = ['', 'CREATE INDEX ix ON t(a) WHERE "future"',
        'CREATE INDEX ix ON t(a) WHERE "b">0', 'CREATE INDEX ix ON t("future")',
        'CREATE INDEX ix ON t("a")', 'CREATE INDEX ix ON t(\'a\')']
    for schema, index in itertools.product(schemas, indexes):
        check([f'CREATE TABLE t({schema})', *([index] if index else []),
            'INSERT INTO t(a,b) VALUES(1,2)', 'ALTER TABLE t DROP b',
            'SELECT * FROM t', 'PRAGMA table_xinfo(t)', 'PRAGMA index_xinfo(ix)',
            'ALTER TABLE t ADD future TEXT DEFAULT \'capture\'', 'SELECT * FROM t'])
    for view in ['SELECT b FROM t', 'SELECT "b" FROM t', 'SELECT * FROM t', 'SELECT * FROM absent']:
        check(['CREATE TABLE t(a,b)', 'CREATE TABLE u(x,g AS("future") STORED)',
            'INSERT INTO t VALUES(1,2)', 'INSERT INTO u(x) VALUES(3)', f'CREATE VIEW v AS {view}',
            'BEGIN', 'ALTER TABLE t DROP b', 'ALTER TABLE u ADD future TEXT DEFAULT \'capture\'',
            'SELECT * FROM v', 'SELECT * FROM u', 'ROLLBACK', 'SELECT * FROM t',
            'ALTER TABLE u ADD future TEXT DEFAULT \'capture\'', 'SELECT * FROM u'])
    # Ordinary names must fail when unresolved. Rowid spellings and unquoted
    # booleans may legally resolve to a different built-in after removal.
    for name, quoting, kind in itertools.product(['rowid', 'oid', '_rowid_', 'true', 'false', 'TRUE', 'b'],
            ['{}', '"{}"', '[{}]', '`{}`', "'{}'"], ['check', 'virtual', 'stored', 'key', 'predicate']):
        expr = quoting.format(name)
        definition = f'a INT,"{name}" INT'
        index = []
        if kind == 'check':
            definition += f',CHECK({expr}>0)'
        elif kind in ['virtual', 'stored']:
            definition += f',g AS({expr})' + (' STORED' if kind == 'stored' else '')
        else:
            index = [f'CREATE INDEX ix ON t({expr})' if kind == 'key' else f'CREATE INDEX ix ON t(a) WHERE {expr}>0']
        check([f'CREATE TABLE t({definition})', *index, f'INSERT INTO t(a,"{name}") VALUES(1,2)',
            f'ALTER TABLE t DROP "{name}"', 'SELECT * FROM t', 'PRAGMA table_xinfo(t)',
            'PRAGMA index_xinfo(ix)', *(['DROP INDEX ix'] if index else []),
            'INSERT INTO t(a) VALUES(3)', 'SELECT * FROM t ORDER BY a'])
    with tempfile.TemporaryDirectory(prefix='safe-drop-quotes-') as directory:
        folder = Path(directory)
        for n, view in enumerate(views):
            source, saved, expected = [folder/f'view-{n}-{part}.db' for part in ['source','saved','expected']]
            native(source, f'CREATE TABLE t(a INT PRIMARY KEY,b INT);CREATE VIEW v AS {view};CREATE TABLE u(x,y AS("future"));')
            expected.write_bytes(source.read_bytes())
            sql = 'ALTER TABLE t DROP b'
            result = subprocess.run([str(ORACLE), str(expected)], input=sql, text=True, capture_output=True, timeout=30)
            if result.returncode:
                run_engine(sql, '--load', source, '--save', saved, fail=True)
                assert expected.read_bytes() == source.read_bytes()
            else:
                run_engine(sql, '--load', source, '--save', saved)
                for q in ['SELECT name,sql FROM sqlite_schema ORDER BY name', 'SELECT * FROM v', 'PRAGMA integrity_check']:
                    assert native(expected,q) == native(saved,q), (view,q,native(expected,q),native(saved,q))
            cases += 1
        for n, (size, encoding, mode, wr) in enumerate(itertools.product(
                [512,1024,2048,4096,8192,16384,32768,65536], ['UTF-8','UTF-16le','UTF-16be'], [0,1,2], [False,True])):
            source, saved, final = [folder/f'image-{n}-{part}.db' for part in ['source','saved','final']]
            native(source, f"PRAGMA page_size={size};PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(id INT PRIMARY KEY,b INT,g TEXT AS(\"future\"),s TEXT AS(\"a'b\") STORED,CHECK(\"constant\">0)) STRICT{' , WITHOUT ROWID' if wr else ''};INSERT INTO t(id,b) VALUES(1,2),(2,3);CREATE INDEX ix ON t(g) WHERE \"future\">0;CREATE VIEW v AS SELECT id,g,s,\"future\" AS literal FROM t;CREATE TABLE u(x INT,y TEXT AS(\"future\") STORED,z TEXT DEFAULT \"default\");INSERT INTO u(x) VALUES(7);")
            sql = 'ALTER TABLE t DROP b;'
            run_engine(sql, '--load', source, '--save', saved)
            native(source, sql)
            queries = ['SELECT * FROM t ORDER BY id', 'SELECT * FROM v ORDER BY id', 'SELECT * FROM u', 'PRAGMA index_xinfo(ix)', 'SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name', 'PRAGMA integrity_check']
            for q in queries:
                assert native(saved,q) == native(source,q), (n,q)
            native(saved, "ALTER TABLE t ADD future TEXT DEFAULT 'capture';ALTER TABLE u ADD future TEXT DEFAULT 'capture';INSERT INTO t(id) VALUES(3);")
            sql = 'ALTER TABLE t DROP future;ALTER TABLE u DROP future;'
            run_engine(sql, '--load', saved, '--save', final)
            native(saved, sql)
            for q in queries:
                assert native(final,q) == native(saved,q), (n,q)
            cases += 1
    print(f'PASS: {cases} DROP literal/dependency/transaction/schema/image scenarios against SQLite 3.53.4')


if __name__ == '__main__':
    main()
