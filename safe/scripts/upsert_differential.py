#!/usr/bin/env python3
"""Compare UPSERT rows, errors and transaction/counter state with pinned SQLite."""
import itertools
from pathlib import Path
import subprocess
import tempfile
from conflict_differential import reference_library, compare, POLICIES
from sql_differential import ROOT, run_engine
from index_differential import native


def main():
    subprocess.run(['cargo', 'build', '--example', 'statement_probe', '--bin', 'sqlite-safe-sql'], cwd=ROOT, check=True)
    lib = reference_library()
    cases = 0
    for schema, targets in [
        ('id INTEGER PRIMARY KEY,x,v', ['id', 'rowid', 't.id DESC']),
        ('id,x UNIQUE,v', ['x', '(x)', 'x COLLATE BINARY']),
        ('id,x,v,UNIQUE(id,x)', ['id,x', 'x,id', 'id DESC,x ASC']),
    ]:
        for target, override, action in itertools.product(targets, ['', *[' OR '+p for p in POLICIES]], [
            'DO NOTHING', 'DO UPDATE SET v=excluded.v',
            'DO UPDATE SET x=x+10,v=excluded.v+v WHERE excluded.v>v',
        ]):
            compare(lib, [f'CREATE TABLE t({schema})', 'INSERT INTO t VALUES(1,1,10),(2,2,20)',
                          f'INSERT{override} INTO t VALUES(3,3,30),(1,1,40),(4,4,50) ON CONFLICT({target}) {action}',
                          'SELECT rowid AS storage_id,* FROM t ORDER BY rowid'])
            cases += 1
    for policy, override, order in itertools.product(POLICIES, ['', *[' OR '+p for p in POLICIES]], itertools.permutations(['id','x','v'])):
        clauses = ' '.join(f'ON CONFLICT({key}) DO UPDATE SET v=excluded.v+1000' for key in order)
        compare(lib, [f'CREATE TABLE t(id INTEGER PRIMARY KEY ON CONFLICT {policy},x UNIQUE ON CONFLICT {policy},v UNIQUE ON CONFLICT {policy})',
                      'INSERT INTO t VALUES(1,10,100),(2,20,200),(3,30,300)',
                      f'INSERT{override} INTO t VALUES(4,40,400),(1,20,300),(5,50,500) {clauses}',
                      'SELECT * FROM t ORDER BY id'])
        cases += 1
    for override, begin, action in itertools.product(['', *[' OR '+p for p in POLICIES]], [[], ['BEGIN'], ['SAVEPOINT s']], [
        'ON CONFLICT(x) DO UPDATE SET v=NULL',
        'ON CONFLICT DO UPDATE SET v=-1',
        'ON CONFLICT(x) DO NOTHING',
        'ON CONFLICT(x) DO UPDATE SET v=excluded.v WHERE 0',
        'ON CONFLICT(x) DO UPDATE SET v=abs(-9223372036854775808)',
    ]):
        compare(lib, ['CREATE TABLE t(id INTEGER PRIMARY KEY,x UNIQUE,v NOT NULL DEFAULT 7 CHECK(v>0))',
                      'INSERT INTO t VALUES(1,10,100),(2,20,200)', *begin,
                      f'INSERT{override} INTO t VALUES(3,30,300),(4,10,400),(5,50,500) {action}',
                      'SELECT * FROM t ORDER BY id', 'ROLLBACK TO s', 'COMMIT', 'SELECT * FROM t ORDER BY id'])
        cases += 1
    for first, second, action in itertools.product(POLICIES, POLICIES, [
        'ON CONFLICT DO UPDATE SET id=id+100',
        'ON CONFLICT(id) DO NOTHING ON CONFLICT DO UPDATE SET id=id+100',
        'ON CONFLICT(x) DO NOTHING ON CONFLICT(v) DO UPDATE SET id=id+100',
        'ON CONFLICT(v) DO UPDATE SET id=id+100 WHERE 0 ON CONFLICT(x) DO UPDATE SET id=id+100',
    ]):
        compare(lib, [f'CREATE TABLE t(id INTEGER PRIMARY KEY,x UNIQUE ON CONFLICT {first},v UNIQUE ON CONFLICT {second})',
                      'INSERT INTO t VALUES(1,10,100),(2,20,200)',
                      f'INSERT OR REPLACE INTO t VALUES(99,10,200) {action}', 'SELECT * FROM t ORDER BY id'])
        cases += 1
    for target in ['x', 'x COLLATE NOCASE', 'x COLLATE BINARY', 'x DESC', 'id', 'id COLLATE BINARY',
                   'rowid', '_rowid_', 'oid', '+x', 'lower(x)', 'x,x', 'x,id', 'absent', "'x'", 't.x']:
        compare(lib, ['CREATE TABLE t(id INTEGER PRIMARY KEY,x TEXT,v)', 'CREATE UNIQUE INDEX u ON t(x COLLATE NOCASE)',
                      "INSERT INTO t VALUES(1,'a',10),(2,'b',20)",
                      f"INSERT INTO t VALUES(3,'A',30) ON CONFLICT({target}) DO UPDATE SET v=excluded.v",
                      'SELECT * FROM t ORDER BY id'])
        cases += 1
    for target in ['rowid,x', 'x,_rowid_', 't.oid,x', 'rowid COLLATE BINARY,x', 'id,x', 'x,id', 'id COLLATE BINARY,x']:
        compare(lib, ['CREATE TABLE t(id INTEGER PRIMARY KEY,x,v,UNIQUE(id,x))', 'INSERT INTO t VALUES(1,10,100)',
                      f'INSERT INTO t VALUES(1,10,999) ON CONFLICT({target}) DO UPDATE SET v=excluded.v',
                      'SELECT * FROM t'])
        cases += 1
    for sql in [
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT(id) DO NOTHING ON CONFLICT(id) DO UPDATE SET v=missing',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT(x) DO NOTHING ON CONFLICT(x) DO UPDATE SET v=missing',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT(id) DO UPDATE SET v=missing',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT(id) DO UPDATE SET missing=1',
        'INSERT INTO t AS dst VALUES(1,2,3) ON CONFLICT(dst.id) DO UPDATE SET v=dst.v+excluded.v',
        'INSERT INTO t AS dst VALUES(1,2,3) ON CONFLICT(t.id) DO UPDATE SET v=excluded.v',
        'INSERT INTO t AS excluded VALUES(1,2,3) ON CONFLICT(id) DO UPDATE SET v=excluded.v',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT(id) DO UPDATE SET v=(SELECT excluded.v+t.v)',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT(id) DO UPDATE SET v=(SELECT v FROM (SELECT excluded.v+10 AS v))',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT(id) DO UPDATE SET v=(SELECT excluded.v FROM t AS excluded WHERE id=1)',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT(id) DO UPDATE SET v=v+excluded.v WHERE EXISTS(SELECT 1 WHERE excluded.v<t.v)',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT(id) DO UPDATE SET id=9,v=excluded.rowid',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT(id) DO UPDATE SET id=NULL',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT DO UPDATE SET v=99',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT(x) WHERE 0 DO UPDATE SET v=99',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT(x) WHERE EXISTS(SELECT 1) DO NOTHING',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT(x) WHERE missing DO NOTHING',
        'INSERT INTO t SELECT 1,2,3 WHERE 1 ON CONFLICT(id) DO UPDATE SET v=excluded.v',
        'WITH q AS(SELECT 1 a,2 b,3 c) INSERT INTO t SELECT * FROM q WHERE 1 ON CONFLICT(id) DO UPDATE SET v=(SELECT c FROM q)+excluded.v',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT(x) DO NOTHING ON CONFLICT DO UPDATE SET v=88',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT DO NOTHING ON CONFLICT(x) DO NOTHING',
        'INSERT INTO t DEFAULT VALUES ON CONFLICT DO NOTHING',
        'REPLACE INTO t VALUES(1,2,3) ON CONFLICT(x) DO UPDATE SET v=excluded.v',
        'INSERT INTO t VALUES(1,2,3),(1,2,4) ON CONFLICT(id) DO UPDATE SET v=v+excluded.v',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT(id) DO UPDATE SET v=(SELECT changes()+total_changes()+last_insert_rowid())',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT(id) DO UPDATE SET v=(SELECT t.v+excluded.v FROM (SELECT 1))',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT(id) DO UPDATE SET v=(WITH q AS(SELECT excluded.v a) SELECT a+t.v FROM q)',
        'INSERT INTO t VALUES(1,2,3) ON CONFLICT(x) WHERE abs(-9223372036854775808) DO NOTHING',
    ]:
        compare(lib, ['CREATE TABLE t(id INTEGER PRIMARY KEY,x UNIQUE,v)', 'INSERT INTO t VALUES(1,10,100),(2,20,200)', sql, 'SELECT * FROM t ORDER BY id'])
        cases += 1
    with tempfile.TemporaryDirectory(prefix='safe-upsert-') as directory:
        directory = Path(directory)
        for encoding, mode in itertools.product(['UTF-8','UTF-16le','UTF-16be'], [0,1,2]):
            original = directory/f'{encoding}-{mode}.db'; saved = directory/f'{encoding}-{mode}-out.db'
            native(original, f"PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(id INTEGER PRIMARY KEY,x TEXT UNIQUE,v);INSERT INTO t VALUES(1,'é',1),(2,'🦀',2);")
            sql = "INSERT INTO t(x,v) VALUES('é',9),('new',10) ON CONFLICT(x) DO UPDATE SET v=excluded.v+v;"
            run_engine(sql, '--load', original, '--save', saved)
            native(original, sql)
            assert native(saved, 'SELECT * FROM t ORDER BY id;') == native(original, 'SELECT * FROM t ORDER BY id;')
            assert native(saved, 'PRAGMA integrity_check;') == [{'integrity_check':'ok'}]
            run_engine('SELECT * FROM t;', '--load', saved)
            cases += 1
    print(f'PASS: {cases} UPSERT target/action/precedence/error/counter/image scenarios against SQLite 3.53.4')


if __name__ == '__main__':
    main()
