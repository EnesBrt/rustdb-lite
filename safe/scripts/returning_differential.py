#!/usr/bin/env python3
"""Compare buffered RETURNING rows, metadata, errors and counters to SQLite."""
import itertools
import subprocess
from conflict_differential import reference_library, compare, POLICIES
from sql_differential import ROOT


def main():
    subprocess.run(['cargo', 'build', '--example', 'statement_probe'], cwd=ROOT, check=True)
    lib = reference_library()
    cases = 0
    setup = ['CREATE TABLE t(id INTEGER PRIMARY KEY,x NUMERIC,y TEXT UNIQUE,z BLOB)',
             "INSERT INTO t VALUES(1,1,'a',x'00'),(2,2,'b',NULL),(3,NULL,'c',x'FF')",
             'CREATE VIEW v AS SELECT * FROM t']
    for dml, projection in itertools.product([
        "INSERT INTO t VALUES(4,'4','d',x'0123'),(5,5.5,'e',NULL)",
        'INSERT INTO t DEFAULT VALUES',
        "INSERT INTO t(x,y) SELECT x+10,y||'x' FROM t",
        "REPLACE INTO t VALUES(4,4,'a',NULL),(5,5,'b',NULL)",
        "INSERT OR IGNORE INTO t VALUES(4,4,'a',NULL),(5,5,'e',NULL)",
        "INSERT INTO t VALUES(4,4,'a',NULL),(5,5,'e',NULL) ON CONFLICT(y) DO UPDATE SET x=excluded.x+t.x",
        "INSERT INTO t VALUES(4,4,'a',NULL) ON CONFLICT DO NOTHING",
        "INSERT INTO t VALUES(4,4,'a',NULL) ON CONFLICT DO UPDATE SET x=999 WHERE 0",
        "INSERT INTO t(x) SELECT 0 WHERE 0",
        'UPDATE t SET x=x+10', 'UPDATE t SET id=id+10,y=y||y WHERE id<3',
        'UPDATE t SET x=99 WHERE 0', 'DELETE FROM t WHERE id<3', 'DELETE FROM t WHERE 0',
    ], [
        '*', 'ROWID,oid,_rowid_,t.ID,t.X AS foo', 'id,x,y,z,typeof(x),hex(z)',
        "id,coalesce(x,-1)+id AS n,length(y),upper(y),CASE WHEN z IS NULL THEN 1 ELSE 0 END",
        'id,changes(),total_changes(),last_insert_rowid(),(SELECT last_insert_rowid())',
        'id,(SELECT count(*) FROM t),(SELECT sum(x) FROM main.t)',
        'id,(SELECT t.x),(SELECT x FROM t AS other WHERE other.id=t.id)',
        'id,EXISTS(SELECT 1 FROM t WHERE x>10),x IN(SELECT x FROM t)',
        'id,(SELECT (SELECT sum(x) FROM t)),(SELECT sum(x) FROM (SELECT * FROM t))',
        'id,(SELECT sum(x) FROM v),(WITH q AS(SELECT sum(x) s FROM t) SELECT s FROM q)',
        'id,(SELECT sum(x) FROM t UNION ALL SELECT 99),(SELECT 99 UNION ALL SELECT sum(x) FROM t)',
    ]):
        compare(lib, [*setup, f'{dml} RETURNING {projection}', 'SELECT * FROM t ORDER BY id'])
        cases += 1
    for policy, begin, dml in itertools.product(POLICIES, [[], ['BEGIN'], ['SAVEPOINT s']], [
        'INSERT OR {policy} INTO t VALUES(4,4),(5,1),(6,6)',
        'UPDATE OR {policy} t SET x=CASE id WHEN 1 THEN 9 ELSE 3 END',
        'INSERT OR {policy} INTO t VALUES(4,4),(5,5),(6,6) RETURNING id,abs(CASE id WHEN 5 THEN -9223372036854775808 ELSE 1 END)',
        'DELETE FROM t RETURNING id,abs(CASE id WHEN 2 THEN -9223372036854775808 ELSE 1 END)',
    ]):
        dml = dml.format(policy=policy)
        if 'RETURNING' not in dml:
            dml += ' RETURNING *,changes(),total_changes(),last_insert_rowid()'
        compare(lib, ['CREATE TABLE t(id INTEGER PRIMARY KEY,x UNIQUE)', 'INSERT INTO t VALUES(1,1),(2,2),(3,3)',
                      *begin, dml, 'SELECT * FROM t ORDER BY id', 'ROLLBACK TO s', 'COMMIT', 'SELECT * FROM t ORDER BY id'])
        cases += 1
    for dml, projection in itertools.product([
        'INSERT INTO t VALUES(4,4)', 'INSERT INTO t SELECT 4,4 WHERE 0',
        'UPDATE t SET x=0', 'DELETE FROM t WHERE 0',
    ], ['missing', 'old.x', 'new.x', 'excluded.x', 't.*', 'sum(x)', 'count(*)', 'abs()',
        '(SELECT missing)', 'x AS a,a', 'id AS x,x', 'x AS returning', 'x AS "from"', '* , t.x']):
        compare(lib, ['CREATE TABLE t(id INTEGER PRIMARY KEY,x)', 'INSERT INTO t VALUES(1,1),(2,2),(3,3)',
                      f'{dml} RETURNING {projection}', 'SELECT * FROM t ORDER BY id'])
        cases += 1
    for hint, dml in itertools.product(['', 'MATERIALIZED', 'NOT MATERIALIZED'], [
        'INSERT INTO t(x) SELECT s FROM q RETURNING x,(SELECT s FROM q)',
        'UPDATE t SET x=(SELECT s FROM q) RETURNING x,(SELECT s FROM q)',
        'DELETE FROM t WHERE x<(SELECT s FROM q) RETURNING x,(SELECT s FROM q)',
        'UPDATE t SET x=CASE id WHEN 1 THEN 10 ELSE (SELECT s FROM q) END RETURNING x,(SELECT s FROM q)',
    ]):
        compare(lib, ['CREATE TABLE t(id INTEGER PRIMARY KEY,x)', 'INSERT INTO t VALUES(1,1),(2,2),(3,3)',
                      f'WITH q AS {hint}(SELECT sum(x) s FROM t) {dml}', 'SELECT * FROM t ORDER BY id'])
        cases += 1
    for sql in [
        'INSERT INTO t AS dst VALUES(4,4) RETURNING dst.x',
        'INSERT INTO t AS dst VALUES(4,4) RETURNING t.x',
        'INSERT INTO t AS dst VALUES(1,4) ON CONFLICT(id) DO UPDATE SET x=excluded.x RETURNING t.x',
        'UPDATE t AS dst SET x=4 RETURNING dst.x',
        'UPDATE t AS dst SET x=4 RETURNING t.x',
        'UPDATE t AS dst SET x=dst.x+1 WHERE dst.id<3 RETURNING t.id,x',
        'DELETE FROM t AS dst WHERE dst.id<3 RETURNING t.id,x',
        'DELETE FROM t AS dst WHERE dst.id<3 RETURNING dst.x',
        'INSERT INTO t VALUES(1,20),(3,3),(1,40),(4,4) ON CONFLICT(id) DO UPDATE SET x=excluded.x RETURNING id,(SELECT last_insert_rowid()),(SELECT sum(x) FROM (SELECT x FROM t))',
        'WITH t(x) AS(SELECT 99) UPDATE main.t SET x=x+1 RETURNING x,(SELECT x FROM t)',
        'WITH t(x) AS(SELECT 99) UPDATE main.t SET x=x+1 RETURNING x,(SELECT sum(x) FROM main.t)',
        'INSERT INTO t VALUES(4,4) RETURNING (WITH t(x) AS(SELECT 99) SELECT x FROM t)',
        'UPDATE t SET x=x+1 RETURNING id,(SELECT sum(x) FROM t WHERE id<=t.id)',
        'DELETE FROM t RETURNING id,(SELECT t.x),EXISTS(SELECT 1 WHERE t.x>1)',
    ]:
        compare(lib, ['CREATE TABLE t(id INTEGER PRIMARY KEY,x)', 'INSERT INTO t VALUES(1,1),(2,2),(3,3)', sql, 'SELECT * FROM t ORDER BY id'])
        cases += 1
    for hint in ['', 'MATERIALIZED', 'NOT MATERIALIZED']:
        compare(lib, ['CREATE TABLE t(id INTEGER PRIMARY KEY,x UNIQUE)', 'INSERT INTO t VALUES(1,1),(2,2)',
                      f'WITH q AS {hint}(SELECT sum(x) s FROM t) INSERT INTO t VALUES(1,40),(9,2),(3,3),(1,50),(9,2) ON CONFLICT(id) DO UPDATE SET x=excluded.x ON CONFLICT(x) DO UPDATE SET x=excluded.x RETURNING id,(SELECT last_insert_rowid()),(SELECT s FROM q)',
                      'SELECT * FROM t ORDER BY id'])
        cases += 1
    for schema, dml in itertools.product(['x', 'oid,x', 'id INTEGER PRIMARY KEY DESC,x', 'ID INTEGER PRIMARY KEY,x'], [
        'INSERT INTO t(x) VALUES(4),(5)', 'UPDATE t SET x=x+10', 'DELETE FROM t',
    ]):
        compare(lib, [f'CREATE TABLE t({schema})', 'INSERT INTO t(x) VALUES(1),(2)',
                      f'{dml} RETURNING ROWID,oid,_ROWID_,X,t.X,*'])
        cases += 1
    print(f'PASS: {cases} RETURNING row/metadata/cache/conflict/error/counter scenarios against SQLite 3.53.4')


if __name__ == '__main__':
    main()
