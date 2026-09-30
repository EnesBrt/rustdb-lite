#!/usr/bin/env python3
"""Join values, merged columns, outer rows, correlated scopes and persisted views."""
import itertools
from pathlib import Path
import subprocess
import tempfile
from conflict_differential import reference_library, compare
from sql_differential import ROOT, run_engine
from index_differential import native

JOINS = ['JOIN', 'LEFT JOIN', 'RIGHT JOIN', 'FULL JOIN', 'INNER JOIN', 'CROSS JOIN',
         'NATURAL JOIN', 'NATURAL LEFT JOIN', 'NATURAL RIGHT JOIN', 'NATURAL FULL JOIN']


def main():
    subprocess.run(['cargo', 'build', '--example', 'statement_probe', '--bin', 'sqlite-safe-sql'], cwd=ROOT, check=True)
    lib = reference_library()
    cases = 0
    for jt, condition, left_type, right_type in itertools.product(
            JOINS, ['ON a.x=b.x', 'USING(x)', 'ON 0', 'ON NULL', 'ON 1', ''],
            ['TEXT COLLATE NOCASE', 'INT', 'BLOB'], ['TEXT COLLATE RTRIM', 'INT', 'BLOB']):
        if jt.startswith('NATURAL') and condition:
            continue
        compare(lib, [f'CREATE TABLE a(x {left_type},y INT)', f'CREATE TABLE b(x {right_type},z INT)',
            "INSERT INTO a VALUES('1',10),('2',20),(NULL,30),('a',40),('A ',50),('1',60)",
            "INSERT INTO b VALUES(1,100),(3,300),(NULL,400),('A',200),('a ',500),(1,600)",
            f'SELECT *,a.*,b.*,a.x,b.x FROM a {jt} b {condition} ORDER BY a.rowid,b.rowid',
            f'CREATE TABLE result AS SELECT * FROM a {jt} b {condition}', 'PRAGMA table_info(result)',
            'SELECT * FROM result ORDER BY y,z'])
        cases += 1
    for first, second, natural in itertools.product(['JOIN','LEFT JOIN','RIGHT JOIN','FULL JOIN'], repeat=3):
        # The third dimension varies the position of unmatched keys, not SQL syntax.
        values = {'JOIN':'(1),(2),(NULL)', 'LEFT JOIN':'(2),(3)', 'RIGHT JOIN':'(3),(4)', 'FULL JOIN':'(NULL),(4)'}[natural]
        compare(lib, ['CREATE TABLE a(x TEXT)', 'CREATE TABLE b(x INT)', 'CREATE TABLE c(x TEXT)',
            "INSERT INTO a VALUES('1'),('2'),(NULL)", 'INSERT INTO b VALUES(2),(3),(NULL)',
            f'INSERT INTO c VALUES{values}',
            f'SELECT *,a.*,b.*,c.*,a.x,b.x,c.x,(SELECT x),typeof(x) FROM a {first} b USING(x) {second} c USING(x) ORDER BY a.rowid,b.rowid,c.rowid',
            f'CREATE TABLE result AS SELECT x FROM a {first} b USING(x) {second} c USING(x)', 'PRAGMA table_info(result)'])
        cases += 1
    for jt, a_rows, b_rows in itertools.product(JOINS, [False, True], [False, True]):
        condition = '' if jt.startswith('NATURAL') else 'USING(x)'
        compare(lib, ['CREATE TABLE a(x INT,g INT AS(coalesce(x,99)))', 'CREATE TABLE b(x INT,h INT AS(coalesce(x,88)))',
            *(['INSERT INTO a(x) VALUES(1),(NULL)'] if a_rows else []),
            *(['INSERT INTO b(x) VALUES(2),(NULL)'] if b_rows else []),
            f'SELECT *,a.g,b.h,(SELECT x),(SELECT a.g+b.h) FROM a {jt} b {condition} ORDER BY a.rowid,b.rowid',
            f'SELECT count(*),count(a.x),count(b.x),sum(x) FROM a {jt} b {condition}'])
        cases += 1
    for sql in [
        'SELECT * FROM a ON 1', 'SELECT * FROM a USING(x)', 'SELECT * FROM a NATURAL JOIN b ON 1',
        'SELECT * FROM a NATURAL JOIN b USING(x)', 'SELECT * FROM a JOIN b ON 1 USING(x)',
        'SELECT * FROM a JOIN b USING(missing)', 'SELECT * FROM a JOIN b USING(rowid)',
        'SELECT * FROM a JOIN b USING()', 'SELECT * FROM a JOIN b USING(x,x)',
        'SELECT * FROM a OUTER JOIN b', 'SELECT * FROM a LEFT INNER JOIN b',
        'SELECT * FROM a LEFT RIGHT JOIN b USING(x)', 'SELECT * FROM a LEFT NATURAL JOIN b',
        'SELECT * FROM a NATURAL NATURAL LEFT JOIN b', 'SELECT * FROM a INNER CROSS JOIN b ON 1',
        'SELECT x FROM a JOIN b ON 1', 'SELECT a.x,b.x FROM a RIGHT JOIN b USING(x)',
        'SELECT * FROM a,b LEFT JOIN c USING(x)', 'SELECT * FROM a,b RIGHT JOIN c USING(x)',
        'SELECT * FROM a FULL JOIN b USING(x) JOIN c USING(x)',
        'SELECT * FROM a LEFT JOIN b ON c.x=b.x JOIN c ON 1',
    ]:
        compare(lib, ['CREATE TABLE a(x)', 'CREATE TABLE b(x)', 'CREATE TABLE c(x)', 'INSERT INTO a VALUES(1)', 'INSERT INTO b VALUES(2)', 'INSERT INTO c VALUES(3)', sql])
        cases += 1
    for jt in ['LEFT JOIN','RIGHT JOIN','FULL JOIN']:
        compare(lib, ['CREATE TABLE a(x INT,y TEXT)', 'CREATE TABLE b(x INT,z TEXT)',
            "INSERT INTO a VALUES(1,'a'),(2,'b')", "INSERT INTO b VALUES(2,'c'),(3,'d')",
            f'WITH q AS(SELECT * FROM a {jt} b USING(x)) SELECT * FROM q ORDER BY x',
            f'SELECT x,count(*),sum(a.x),sum(b.x) FROM a {jt} b USING(x) GROUP BY x ORDER BY x',
            f'SELECT * FROM a {jt} b ON EXISTS(SELECT 1 WHERE a.x=b.x) ORDER BY a.x,b.x',
            f'SELECT a.x,b.x FROM a {jt} b ON a.x=b.x WHERE a.x IS NULL OR b.x IS NULL ORDER BY a.x,b.x',
            f'CREATE VIEW v AS SELECT * FROM a {jt} b USING(x)', 'PRAGMA table_info(v)', 'SELECT * FROM v ORDER BY x',
            'CREATE TABLE output(x,y,z)', 'INSERT INTO output SELECT * FROM v RETURNING *', 'SELECT * FROM output ORDER BY x'])
        cases += 1
    for jt,condition,projection in itertools.product(['JOIN','LEFT JOIN','RIGHT JOIN','FULL JOIN'], ['USING(x,y)','USING(y,x)','USING(x,x)','ON a.x=b.x AND a.y=b.y'], ['*','a.*,b.*','x,y','a.x,b.x,(SELECT a.y),(SELECT b.y)']):
        compare(lib,['CREATE TABLE a(x TEXT COLLATE NOCASE,y INT)', 'CREATE TABLE b(x TEXT,y TEXT)',
            "INSERT INTO a VALUES('a',1),('b',2),(NULL,3),('c',NULL)", "INSERT INTO b VALUES('A','1'),('b','3'),(NULL,'3'),('c',NULL)",
            f'SELECT {projection} FROM a {jt} b {condition} ORDER BY a.rowid,b.rowid'])
        cases+=1
    for jt,condition in itertools.product(['LEFT JOIN','RIGHT JOIN','FULL JOIN'], ['x=2',"x='2'",'x IS NULL',"x='A'",'x IN(1,2,3)',"x IN('1','2','3')"]):
        compare(lib,['CREATE TABLE a(x TEXT COLLATE NOCASE)', 'CREATE TABLE b(x INT)', "INSERT INTO a VALUES('1'),('2'),('a')", "INSERT INTO b VALUES(2),(3),('A')",
            f'SELECT x,typeof(x) FROM a {jt} b USING(x) WHERE {condition} ORDER BY a.rowid,b.rowid',
            f'WITH q AS(SELECT x FROM a {jt} b USING(x)) SELECT x,typeof(x) FROM q WHERE {condition} ORDER BY x'])
        cases+=1
    with tempfile.TemporaryDirectory(prefix='safe-joins-') as folder:
        folder = Path(folder)
        for encoding,mode in itertools.product(['UTF-8','UTF-16le','UTF-16be'], [0,1,2]):
            source=folder/f'{encoding}-{mode}.db'; result=folder/f'{encoding}-{mode}-rust.db'; final=folder/f'{encoding}-{mode}-final.db'
            native(source, f"PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE a(x TEXT,y INT);CREATE TABLE b(x TEXT,z INT);CREATE VIEW v AS SELECT * FROM a FULL JOIN b USING(x);INSERT INTO a VALUES('é',1),('Rust',2);INSERT INTO b VALUES('Rust',3),('🦀',4);")
            run_engine('CREATE TABLE copied AS SELECT * FROM v;UPDATE a SET y=y+10;', '--load',source,'--save',result)
            native(source,'CREATE TABLE copied AS SELECT * FROM v;UPDATE a SET y=y+10;')
            for sql in ['SELECT * FROM v ORDER BY x','SELECT * FROM copied ORDER BY x','PRAGMA table_info(v)','PRAGMA table_info(copied)']:
                assert native(result,sql)==native(source,sql), (encoding,mode,sql)
            assert native(result,'PRAGMA integrity_check')==[{'integrity_check':'ok'}]
            native(result,"INSERT INTO b VALUES('native',5)")
            run_engine('INSERT INTO a SELECT x,6 FROM v WHERE y IS NULL;', '--load',result,'--save',final)
            native(result,'INSERT INTO a SELECT x,6 FROM v WHERE y IS NULL;')
            assert native(final,'SELECT * FROM v ORDER BY x')==native(result,'SELECT * FROM v ORDER BY x')
            assert native(final,'PRAGMA integrity_check')==[{'integrity_check':'ok'}]
            cases+=1
    print(f'PASS: {cases} join/value/metadata/correlation/constraint/image scenarios against SQLite 3.53.4')


if __name__ == '__main__':
    main()
