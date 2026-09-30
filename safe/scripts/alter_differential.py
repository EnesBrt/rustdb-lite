#!/usr/bin/env python3
"""ALTER ADD/DROP COLUMN, atomic state and images against SQLite 3.53.4."""
import itertools
import subprocess
import tempfile
from pathlib import Path
from conflict_differential import reference_library, compare
from index_differential import native
from sql_differential import ROOT, run_engine


def main():
    subprocess.run(['cargo', 'build', '--example', 'statement_probe', '--bin', 'sqlite-safe-sql'], cwd=ROOT, check=True)
    lib = reference_library()
    cases = 0

    def check(statements):
        nonlocal cases
        compare(lib, statements)
        cases += 1

    defaults = ['', ' DEFAULT NULL', ' DEFAULT (+NULL)', ' DEFAULT (-NULL)',
        ' DEFAULT 42', ' DEFAULT -9223372036854775808', ' DEFAULT 9223372036854775808',
        " DEFAULT '003'", " DEFAULT 'bad'", " DEFAULT x'32'", ' DEFAULT true',
        ' DEFAULT false', ' DEFAULT "literal"', ' DEFAULT bareword',
        " DEFAULT (-'42')", " DEFAULT (-'2x')", " DEFAULT (-(+'42'))",
        ' DEFAULT (-(-9223372036854775808))', " DEFAULT (CAST('42' AS TEXT))",
        " DEFAULT (CAST(x'32' AS INT))", ' DEFAULT (1+2)', ' DEFAULT (abs(-3))',
        ' DEFAULT (NULL COLLATE nocase)', ' DEFAULT (coalesce(NULL,4))',
        ' DEFAULT (true)', ' DEFAULT (false)', ' DEFAULT +true', ' DEFAULT -false',
        ' DEFAULT abs(-3)', ' DEFAULT ++1', ' DEFAULT -(1)', ' DEFAULT 1.20e2', ' DEFAULT -1.20e2']
    for empty, schema, typ, default, nullable in itertools.product([False, True], ['', ' STRICT', ' WITHOUT ROWID'], ['INT', 'TEXT', 'REAL', 'BLOB', 'ANY'], defaults, ['', ' NOT NULL']):
        check([f'CREATE TABLE t(a INT PRIMARY KEY){schema}']
            + ([] if empty else ['INSERT INTO t VALUES(1)'])
            + [f'ALTER TABLE main.t ADD COLUMN b {typ}{nullable}{default}',
                'SELECT * FROM t', 'PRAGMA table_xinfo(t)',
                'INSERT INTO t(a) VALUES(3)', 'SELECT * FROM t ORDER BY a'])
    for empty, schema, definition in itertools.product([False, True], ['', ' STRICT', ' WITHOUT ROWID'], [
        'b INT CHECK(b>2) DEFAULT 1', 'b INT CHECK(b>2) DEFAULT 3',
        'b INT CHECK(a>2)', 'b INT CHECK(NULL)', 'b INT CHECK(0)',
        'b INT AS(a+1)', 'b INT AS(a+1) STORED', 'b TEXT AS(a+1) NOT NULL',
        'b INT AS(NULL) NOT NULL ON CONFLICT IGNORE', 'b INT AS(NULL) CHECK(b>0)',
        'b INT AS(b)', 'b INT AS(a+1) CHECK(b>2)', 'b INT AS(a+1) CHECK(b>0)',
        'b INT AS(99) DEFAULT 1', 'b INT AS(abs(a))', 'b INT AS(rowid)',
        'b INT AS(missing)', 'b INT AS((SELECT a))', 'b INT AS(?)',
        'b INT PRIMARY KEY', 'b INT UNIQUE', 'b INT PRIMARY KEY AUTOINCREMENT',
        'b INT UNIQUE ON CONFLICT REPLACE', 'b INT DEFAULT ?', 'a TEXT',
        'b INT CHECK(missing)', 'b INT CHECK(?)', 'b INT DEFAULT (a)',
        'b INT DEFAULT ("literal")', 'b INT CHECK(abs(-9223372036854775808))',
    ]):
        check([f'CREATE TABLE t(a INT PRIMARY KEY){schema}']
            + ([] if empty else ['INSERT INTO t VALUES(1)'])
            + [f'ALTER TABLE t ADD {definition}', 'SELECT * FROM t', 'PRAGMA table_xinfo(t)',
                'INSERT INTO t(a) VALUES(3)', 'SELECT * FROM t ORDER BY a'])
    for empty, schema, target, dependent in itertools.product([False, True], ['', ' STRICT', ' WITHOUT ROWID'], ['a', 'b', 'c', 'g', 'rowid', 'missing'], [
        '', 'CREATE INDEX ix ON t(b)', 'CREATE INDEX ix ON t(c)',
        'CREATE INDEX ix ON t((b+1))', 'CREATE INDEX ix ON t(("b"+1))',
        'CREATE INDEX ix ON t(a) WHERE b>0', 'CREATE INDEX ix ON t(a) WHERE "b">0',
        'CREATE UNIQUE INDEX ix ON t(c,b)', 'CREATE VIEW v AS SELECT * FROM t',
        'CREATE VIEW v AS SELECT b FROM t', 'CREATE VIEW v AS SELECT c FROM t',
        'CREATE VIEW v AS SELECT * FROM missing', 'CREATE VIEW v AS SELECT missing FROM t',
        'CREATE VIEW v(x) AS SELECT * FROM t', 'CREATE VIEW v AS SELECT sum(c) FROM t',
    ]):
        check([f'CREATE TABLE t(a INT PRIMARY KEY,b TEXT,c INT,g INT AS(c+1)){schema}']
            + ([] if empty else ["INSERT INTO t(a,b,c) VALUES(1,'one',10),(2,'two',20)"])
            + ([dependent] if dependent else [])
            + [f'ALTER TABLE t DROP COLUMN {target}', 'SELECT * FROM t ORDER BY a',
                'PRAGMA table_xinfo(t)', 'PRAGMA index_list(t)', 'SELECT * FROM v'])
    for definition, target in itertools.product([
        'a,b,c', 'a,b CHECK(b>0),c', 'a,b CHECK(c>0),c', 'a,b,c CHECK(b>0)',
        'a,b,c,CHECK(b>0)', 'a,b,c,UNIQUE(b,c)', 'a,b,c,PRIMARY KEY(b,c)',
        'a INTEGER PRIMARY KEY AUTOINCREMENT,b,c',
        'a,b,g AS(b+1)', 'a,b,g AS("b"+1)', 'a,b,g AS(a+1) STORED',
        'a,b,g AS(a+1) STORED,h AS(g+1)', 'a,b CHECK(b>0)',
        'a,g AS(a+1)', 'a,g AS(a+1) STORED',
    ], ['a','b','c','g','h']):
        check([f'CREATE TABLE t({definition})', f'ALTER TABLE t DROP {target}',
            'PRAGMA table_xinfo(t)', 'PRAGMA index_list(t)', 'SELECT * FROM t'])
    for schema in ['', ' STRICT', ' WITHOUT ROWID']:
        check([f'CREATE TABLE t(id INT PRIMARY KEY,a TEXT,b INT){schema}',
            "INSERT INTO t VALUES(1,'old',2)", 'BEGIN', 'ALTER TABLE t ADD c INT DEFAULT 3',
            'SAVEPOINT s', 'ALTER TABLE t DROP a', 'SELECT * FROM t', 'ROLLBACK TO s',
            'SELECT * FROM t', 'ALTER TABLE t ADD z INT CHECK(z<0) DEFAULT 3',
            'SELECT * FROM t', 'ROLLBACK', 'SELECT * FROM t',
            'CREATE VIEW v AS SELECT * FROM t', 'ALTER TABLE v ADD c INT',
            'ALTER TABLE missing ADD c INT', 'ALTER TABLE t DROP rowid'])
    check(['CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,a,b)',
        "INSERT INTO t VALUES(8,'eight',3)", 'DELETE FROM t',
        'ALTER TABLE t DROP a', 'ALTER TABLE t ADD c INT DEFAULT 9',
        'INSERT INTO t(b) VALUES(4)', 'SELECT * FROM t', 'SELECT * FROM sqlite_sequence',
        'ALTER TABLE sqlite_sequence ADD x', 'ALTER TABLE sqlite_sequence DROP seq'])
    with tempfile.TemporaryDirectory(prefix='safe-alter-') as folder:
        folder = Path(folder)
        for n, (size, encoding, mode, wr) in enumerate(itertools.product(
                [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536],
                ['UTF-8', 'UTF-16le', 'UTF-16be'], [0, 1, 2], [False, True])):
            source, saved, final = [folder / f'{n}-{part}.db' for part in ['source','saved','final']]
            options = ' STRICT, WITHOUT ROWID' if wr else ' STRICT'
            native(source, f"PRAGMA page_size={size};PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(id INT PRIMARY KEY, discarded TEXT, a INT, v INT AS(a+1), s INT AS(a*2) STORED, CHECK(a>0)){options};INSERT INTO t(id,discarded,a) VALUES(1,'é🦀',4),(2,'large',6);CREATE INDEX ix ON t((s+v)) WHERE a>0;CREATE VIEW v AS SELECT id,a,s,v FROM t;")
            # Native records omit the freshly added value; import must apply the
            # column affinity before Rust performs the next schema edit.
            native(source, "ALTER TABLE t ADD d TEXT DEFAULT 42")
            sql = "ALTER TABLE t ADD e INT DEFAULT '003';ALTER TABLE t ADD g TEXT AS(d||':'||e);ALTER TABLE t DROP discarded;"
            run_engine(sql, '--load', source, '--save', saved)
            native(source, sql)
            for q in ['SELECT * FROM t ORDER BY id', 'SELECT * FROM v ORDER BY id',
                    'PRAGMA table_xinfo(t)', 'PRAGMA index_xinfo(ix)',
                    'SELECT type,name,sql FROM sqlite_schema ORDER BY name', 'PRAGMA integrity_check']:
                assert native(saved,q) == native(source,q), (n,q,native(saved,q),native(source,q))
            native(saved, "INSERT INTO t(id,a) VALUES(3,8);ALTER TABLE t ADD z BLOB DEFAULT x'0032'")
            sql = 'ALTER TABLE t DROP g;ALTER TABLE t DROP e;UPDATE t SET a=a+1;'
            run_engine(sql, '--load', saved, '--save', final)
            native(saved, sql)
            for q in ['SELECT * FROM t ORDER BY id','SELECT * FROM v ORDER BY id',
                    'SELECT type,name,sql FROM sqlite_schema ORDER BY name','PRAGMA integrity_check']:
                assert native(final,q) == native(saved,q), (n,q)
            cases += 1
        for n, schema in enumerate([
            'CREATE TABLE t("é" INT, /* a,b */ [weird,b] TEXT, `c` REAL, CHECK("é">0))',
            "CREATE TABLE t(a DEFAULT 'x,y', b DECIMAL(8,2) /* trailing */, c /* tail */)",
            'CREATE TABLE t(a, b, c /* tail */, UNIQUE(a))',
            'CREATE TABLE t(a, b, c /* tail */)',
        ]):
            for column in ['a','b','c','é','weird,b']:
                source, saved = folder/f'comments-{n}-{column}.db', folder/'comments-saved.db'
                native(source,schema)
                add = 'ALTER TABLE t ADD [new,col] TEXT DEFAULT \'x,y\''
                saved.unlink(missing_ok=True)
                run_engine(add,'--load',source,'--save',saved)
                native(source,add)
                assert native(saved,'SELECT sql FROM sqlite_schema') == native(source,'SELECT sql FROM sqlite_schema'), (n,column,native(saved,'SELECT sql FROM sqlite_schema'),native(source,'SELECT sql FROM sqlite_schema'))
                # Drop each known column, including the new last column.
                names = [v['name'] for v in native(source,'PRAGMA table_info(t)')]
                if column in names:
                    sql = 'ALTER TABLE t DROP "'+column.replace('"','""')+'"'
                    if column not in ['a','é']:
                        saved.unlink(missing_ok=True)
                        run_engine(sql,'--load',source,'--save',saved)
                        native(source,sql)
                        assert native(saved,'SELECT sql FROM sqlite_schema') == native(source,'SELECT sql FROM sqlite_schema'), (n,column,native(saved,'SELECT sql FROM sqlite_schema'),native(source,'SELECT sql FROM sqlite_schema'))
                cases += 1
    print(f'PASS: {cases} ALTER column/default/dependency/state/image scenarios against SQLite 3.53.4')

if __name__ == '__main__':
    main()
