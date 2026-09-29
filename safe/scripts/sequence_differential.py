#!/usr/bin/env python3
"""Compare AUTOINCREMENT allocation, sqlite_sequence and transaction state."""
import itertools
from pathlib import Path
import subprocess
import tempfile
from conflict_differential import reference_library, compare, POLICIES
from sql_differential import ROOT, ORACLE, run_engine
from index_differential import native

SEQUENCE = 'SELECT rowid AS rid,name,seq,typeof(seq) AS typ FROM sqlite_sequence ORDER BY rowid'
ROWS = 'SELECT * FROM t ORDER BY id'


def main():
    subprocess.run(['cargo', 'build', '--example', 'statement_probe', '--bin', 'sqlite-safe-sql'], cwd=ROOT, check=True)
    lib = reference_library()
    cases = 0
    compare(lib, ['DROP TABLE IF EXISTS sqlite_sequence', 'DROP TABLE sqlite_sequence', 'CREATE TABLE IF NOT EXISTS sqlite_sequence(x)', 'CREATE VIEW IF NOT EXISTS sqlite_sequence AS SELECT 1', 'CREATE TABLE sqlite_sequence AS SELECT 1'])
    cases += 1
    for schema in [
        'id INTEGER PRIMARY KEY AUTOINCREMENT,x',
        'id INTEGER PRIMARY KEY ASC ON CONFLICT IGNORE AUTOINCREMENT,x',
        'id INTEGER,x,PRIMARY KEY(id AUTOINCREMENT)',
        'id INTEGER,x,PRIMARY KEY(id DESC AUTOINCREMENT) ON CONFLICT REPLACE',
        'id INT PRIMARY KEY AUTOINCREMENT,x',
        'id INTEGER AUTOINCREMENT,x',
        'id INTEGER PRIMARY KEY DESC AUTOINCREMENT,x',
        'id INTEGER,x,PRIMARY KEY(id,x AUTOINCREMENT)',
        'id INTEGER,x,PRIMARY KEY(id) AUTOINCREMENT',
        'id INTEGER PRIMARY KEY AUTOINCREMENT AUTOINCREMENT,x',
        'id INTEGER PRIMARY KEY ON CONFLICT FAIL AUTOINCREMENT DEFAULT 5,x',
        'id "INTEGER" PRIMARY KEY AUTOINCREMENT,x',
    ]:
        compare(lib, [f'CREATE TABLE t({schema})', SEQUENCE, 'PRAGMA table_info(sqlite_sequence)',
                      'INSERT INTO t(x) VALUES(1),(2)', ROWS, SEQUENCE,
                      'DELETE FROM t', 'INSERT INTO t(x) VALUES(3)', ROWS, SEQUENCE, 'DROP TABLE t', SEQUENCE])
        cases += 1
    for policy, begin, dml in itertools.product(POLICIES, [[], ['BEGIN'], ['SAVEPOINT s']], [
        'INSERT OR {p} INTO t(x) VALUES(3),(1),(4)',
        'INSERT OR {p} INTO t VALUES(50,3),(90,1),(NULL,4)',
        'INSERT OR {p} INTO t(x) VALUES(3),(NULL),(4)',
        'INSERT OR {p} INTO t(x) VALUES(3),(-1),(4)',
        'INSERT OR {p} INTO t(x) SELECT 5 WHERE 0',
        'INSERT OR {p} INTO t(x) VALUES(3),(abs(-9223372036854775808))',
        'INSERT OR {p} INTO t(x) VALUES(3),(1),(4) ON CONFLICT(x) DO NOTHING',
        'INSERT OR {p} INTO t(x) VALUES(3),(1),(4) ON CONFLICT(x) DO UPDATE SET id=id+100',
        'INSERT OR {p} INTO t(x) VALUES(3),(1),(4) ON CONFLICT(x) DO UPDATE SET x=NULL',
        'INSERT OR {p} INTO t(x) VALUES(3),(1),(4) RETURNING id,(SELECT seq FROM sqlite_sequence WHERE name=\'t\')',
    ]):
        compare(lib, ['CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,x UNIQUE NOT NULL CHECK(x>0))',
                      'INSERT INTO t(x) VALUES(1),(2)', *begin, 'INSERT INTO t VALUES(20,20)',
                      dml.format(p=policy), ROWS, SEQUENCE, 'ROLLBACK TO s', ROWS, SEQUENCE,
                      'COMMIT', 'INSERT INTO t(x) VALUES(99)', ROWS, SEQUENCE])
        cases += 1
    for seq, mutation in itertools.product(['NULL','-10','0','3','3.5',"'100x'", "'abc'", "x'3132'", '9223372036854775807'], [
        'INSERT INTO t VALUES(-5,1)', 'INSERT INTO t DEFAULT VALUES',
        'INSERT INTO t(x) SELECT 1 WHERE 0', 'INSERT OR IGNORE INTO t VALUES(0,1)',
    ]):
        compare(lib, ['CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,x)',
                      f"INSERT INTO sqlite_sequence VALUES('t',{seq})", mutation, SEQUENCE, ROWS,
                      'INSERT INTO t DEFAULT VALUES RETURNING id,(SELECT seq FROM sqlite_sequence)', SEQUENCE, ROWS])
        cases += 1
    for begin, policy, seed in itertools.product([[], ['BEGIN'], ['SAVEPOINT s']], POLICIES, [
        'INSERT INTO t VALUES(9223372036854775807,1)',
        "INSERT INTO sqlite_sequence VALUES('t',9223372036854775807)",
        'INSERT INTO t VALUES(9223372036854775806,1)',
    ]):
        compare(lib, ['CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,x)', seed, *begin,
                      'INSERT INTO t VALUES(-10,99)',
                      f'INSERT OR {policy} INTO t(x) VALUES(2),(3)', ROWS, SEQUENCE, 'ROLLBACK TO s', 'COMMIT', ROWS, SEQUENCE])
        cases += 1
    for statements in [
        ['INSERT INTO t VALUES(0,1),(-10,2)', 'INSERT INTO t(x) VALUES(3)', 'DELETE FROM t', 'INSERT INTO t(x) VALUES(4)'],
        ['INSERT INTO t VALUES(100,1)', 'DELETE FROM t', 'DELETE FROM sqlite_sequence', 'INSERT INTO t(x) VALUES(2)'],
        ['INSERT INTO t(x) VALUES(1)', 'UPDATE t SET id=100', 'INSERT INTO t VALUES(2,2)', 'INSERT INTO t(x) VALUES(3)'],
        ["INSERT INTO sqlite_sequence VALUES('t',10),('t',100)", 'INSERT INTO t DEFAULT VALUES', 'DROP TABLE t'],
        ["INSERT INTO sqlite_sequence VALUES('T',10),(x'74',20),(NULL,30)", 'INSERT INTO t DEFAULT VALUES', 'DROP TABLE t'],
        ['CREATE TABLE u(id INTEGER PRIMARY KEY AUTOINCREMENT,x)', 'INSERT INTO u DEFAULT VALUES', 'INSERT INTO t DEFAULT VALUES', 'DROP TABLE u', 'DROP TABLE t', 'CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,x)', 'INSERT INTO t DEFAULT VALUES'],
        ['INSERT INTO t DEFAULT VALUES', "UPDATE sqlite_sequence SET seq='50'", 'INSERT INTO t DEFAULT VALUES', 'DELETE FROM sqlite_sequence WHERE name=\'t\'', 'INSERT INTO t DEFAULT VALUES'],
        ['INSERT INTO t DEFAULT VALUES', 'DROP TABLE sqlite_sequence', 'DROP TABLE IF EXISTS sqlite_sequence', 'CREATE INDEX idx ON sqlite_sequence(name)', 'CREATE TABLE IF NOT EXISTS sqlite_sequence(x)', 'CREATE VIEW IF NOT EXISTS sqlite_sequence AS SELECT 1'],
        ['INSERT INTO t DEFAULT VALUES', 'BEGIN', 'INSERT INTO t VALUES(100,1)', 'SAVEPOINT s', 'INSERT INTO t VALUES(200,2)', 'ROLLBACK TO s', 'INSERT INTO t DEFAULT VALUES', 'ROLLBACK', 'INSERT INTO t DEFAULT VALUES'],
    ]:
        steps=['CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,x)']
        for sql in statements: steps += [sql, ROWS, SEQUENCE]
        compare(lib, steps)
        cases += 1
    with tempfile.TemporaryDirectory(prefix='safe-sequence-') as folder:
        folder=Path(folder)
        for encoding,mode in itertools.product(['UTF-8','UTF-16le','UTF-16be'],[0,1,2]):
            original=folder/f'{encoding}-{mode}.db';saved=folder/f'{encoding}-{mode}-out.db'
            native(original,f"PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,x TEXT UNIQUE);INSERT INTO t VALUES(100,'é');DELETE FROM t;")
            sql="INSERT INTO t(x) VALUES('🦀'),('é');INSERT OR IGNORE INTO t(x) VALUES('é');INSERT INTO t(x) VALUES('new') RETURNING *;"
            run_engine(sql,'--load',original,'--save',saved)
            native(original,sql)
            for query in [ROWS,SEQUENCE]: assert native(saved,query)==native(original,query)
            assert native(saved,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
            native(saved,"INSERT INTO t(x) VALUES('native');")
            final=folder/f'{encoding}-{mode}-final.db'
            run_engine("INSERT INTO t(x) VALUES('Rust');",'--load',saved,'--save',final)
            assert native(final,'SELECT max(id) AS id FROM t;')==[{'id':106}]
            assert native(final,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
            cases+=1
        # A Rust-created schema must also allocate correctly in native SQLite.
        fresh=folder/'fresh.db'
        run_engine("CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,x UNIQUE);INSERT INTO t VALUES(100,'old');DELETE FROM t;", '--save',fresh)
        native(fresh,"INSERT INTO t(x) VALUES('native');")
        assert native(fresh,ROWS)==[{'id':101,'x':'native'}]
        assert native(fresh,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
        cases+=1
        # sqlite_sequence survives after its final AUTOINCREMENT table is gone.
        empty=folder/'empty.db'
        native(empty,'CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT);DROP TABLE t;')
        restored=folder/'restored.db'
        run_engine('CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,x);INSERT INTO t DEFAULT VALUES;', '--load',empty,'--save',restored)
        assert native(restored,ROWS)==[{'id':1,'x':None}]
        assert native(restored,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
        cases+=1
        # Damaged system-table declarations must not silently reset high water.
        for n,sql in enumerate([
            "DELETE FROM sqlite_schema WHERE name='sqlite_sequence'",
            "UPDATE sqlite_schema SET sql='CREATE TABLE sqlite_sequence(name)' WHERE name='sqlite_sequence'",
            "UPDATE sqlite_schema SET tbl_name='t' WHERE name='sqlite_sequence'",
            "INSERT INTO sqlite_schema SELECT type,name,tbl_name,rootpage,sql FROM sqlite_schema WHERE name='sqlite_sequence'",
        ]):
            broken=folder/f'broken-{n}.db'
            native(broken,'CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT);INSERT INTO t VALUES(100);DELETE FROM t;')
            changed=subprocess.run([str(ORACLE),str(broken)], input=f'.dbconfig defensive off\nPRAGMA writable_schema=ON;{sql};\n', text=True,capture_output=True)
            assert changed.returncode==0,changed.stderr
            run_engine('SELECT * FROM t;', '--load',broken,fail=True)
            cases+=1
    print(f'PASS: {cases} AUTOINCREMENT/sequence/conflict/counter/transaction/image scenarios against SQLite 3.53.4')


if __name__ == '__main__':
    main()
