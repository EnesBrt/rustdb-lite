#!/usr/bin/env python3
"""Parenthesized join namespaces and ON dependency resolution against SQLite."""
import itertools
import subprocess
import tempfile
from pathlib import Path
from conflict_differential import reference_library, compare
from sql_differential import ROOT, run_engine
from index_differential import native

SEED = ['CREATE TABLE a(x INT,y TEXT)', 'CREATE TABLE b(x INT,z TEXT)', 'CREATE TABLE c(w INT)',
        "INSERT INTO a VALUES(1,'a'),(2,'b')", "INSERT INTO b VALUES(2,'c'),(3,'d')", 'INSERT INTO c VALUES(1)']

def main():
    subprocess.run(['cargo','build','--example','statement_probe','--bin','sqlite-safe-sql'],cwd=ROOT,check=True)
    lib=reference_library(); cases=0
    for group,alias,prefix,columns in itertools.product(
        ['(a JOIN b USING(x))','(a FULL JOIN b USING(x))','(a RIGHT JOIN b USING(x))','(a JOIN b ON 1)', '(a LEFT JOIN b ON a.x=b.x)'],
        ['', ' g'], ['', 'c,'],
        ['*','a.*','b.*','g.*','a.x','b.x','g.x','x','a.rowid','b.rowid','g.rowid','g."x:1"','g."x:2"','g._ROWID_','g."_ROWID_:1"','(SELECT a.x)','(SELECT x)','g.y,g.z']):
        compare(lib,SEED+[f'SELECT {columns} FROM {prefix}{group}{alias}'])
        cases+=1
    for group,columns in itertools.product(['(a)','((a))','(a aa)','((a aa))','(a) aa','(a aa) bb','((SELECT * FROM a)) aa'], ['*','a.x','aa.x','bb.x','a.rowid','aa.rowid']):
        for prefix in ['', 'c,']:
            compare(lib,SEED+[f'SELECT {columns} FROM {prefix}{group}'])
            cases+=1
    for kind,condition,tail in itertools.product(['JOIN','LEFT JOIN','RIGHT JOIN','FULL JOIN'],
        ['c.w=a.x','(SELECT c.w)=a.x','(SELECT (SELECT c.w))=a.x',
         'EXISTS(SELECT 1 WHERE c.w=a.x)','EXISTS(SELECT 1 FROM c WHERE c.w=a.x)',
         'EXISTS(WITH q AS(SELECT c.w w) SELECT * FROM q WHERE w=a.x)',
         'a.x=b.x','(SELECT a.x)=b.x','alias_x=b.x','missing=b.x'],
        ['JOIN','LEFT JOIN','RIGHT JOIN','FULL JOIN']):
        compare(lib,SEED+[f'SELECT a.x AS alias_x,b.x,c.w FROM a {kind} b ON {condition} {tail} c ON c.w=a.x ORDER BY a.rowid,b.rowid,c.rowid'])
        cases+=1
    for outer,inner,constraint in itertools.product(['JOIN','LEFT JOIN','RIGHT JOIN','FULL JOIN'], ['JOIN','LEFT JOIN','RIGHT JOIN','FULL JOIN'], ['ON a.x=b.x','USING(x)']):
        seed=['CREATE TABLE a(x TEXT,y INT)', 'CREATE TABLE b(x INT,z INT)', 'CREATE TABLE c(x INT,w INT)',
            "INSERT INTO a VALUES('1',10),('2',20)",'INSERT INTO b VALUES(2,200),(3,300)','INSERT INTO c VALUES(1,1000),(3,3000)']
        compare(lib,seed+[f'SELECT * FROM c {outer} (a {inner} b {constraint}) USING(x) ORDER BY w,y,z',
            f'SELECT a.x,b.x,c.x FROM c {outer} (a {inner} b {constraint}) USING(x) ORDER BY c.rowid,a.rowid,b.rowid'])
        cases+=1
    for sql in [
        'SELECT * FROM c,((a FULL JOIN b USING(x)) g JOIN c cc ON 1)',
        'SELECT a.x,b.x FROM c,((a FULL JOIN b USING(x)) g JOIN c cc ON 1)',
        'SELECT * FROM c,(a JOIN b USING(x) JOIN a aa USING(x))',
        'SELECT x FROM c,(a FULL JOIN b USING(x) FULL JOIN a aa USING(x))',
        'SELECT * FROM c,(a JOIN b ON a.x=c.w)',
        'SELECT * FROM c,(a LEFT JOIN b ON (SELECT c.w)=a.x)',
        'SELECT * FROM (a JOIN b USING(x)) AS g JOIN c ON g.x=c.w',
        'SELECT * FROM (a JOIN b ON 1) AS g JOIN c ON g.x=c.w',
        'WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM (q) WHERE x<3) SELECT * FROM q',
    ]:
        compare(lib,SEED+[sql]);cases+=1
    for suffix, generated, projection in itertools.product(['', ' WITHOUT ROWID'], ['VIRTUAL','STORED'],
        ['*','g.x,a.x,b.x,a.y,b.z','(SELECT g.x), (SELECT a.y+b.z)','a.*,b.*']):
        compare(lib,[f'CREATE TABLE a(id INT PRIMARY KEY,x TEXT,y INT AS(length(x)) {generated}){suffix}',f'CREATE TABLE b(id INT PRIMARY KEY,x TEXT,z INT AS(length(x)) {generated}){suffix}',
            "INSERT INTO a(id,x) VALUES(1,'a'),(2,'bb')", "INSERT INTO b(id,x) VALUES(2,'bb'),(3,'ccc')",'CREATE TABLE c(w)', 'INSERT INTO c VALUES(1)',
            f'SELECT {projection} FROM c,(a FULL JOIN b USING(x)) g ORDER BY a.id,b.id',
            'CREATE VIEW v AS SELECT g.x,a.y,b.z FROM c,(a FULL JOIN b USING(x)) g','PRAGMA table_info(v)','SELECT * FROM v ORDER BY x'])
        cases+=1
    for schema in ['x INT,_rowid_ TEXT','x INT,_rowid_ TEXT,rowid TEXT','x INT,_rowid_ TEXT,rowid TEXT,oid TEXT']:
        for column in ['a.rowid','a._rowid_','a.oid','g._rowid_','g.rowid','g.oid']:
            compare(lib,[f'CREATE TABLE a({schema})','CREATE TABLE b(y)','CREATE TABLE c(w)','INSERT INTO a(x) VALUES(4)','INSERT INTO b VALUES(5)','INSERT INTO c VALUES(6)',f'SELECT {column} FROM c,(a JOIN b) g'])
            cases+=1
    for clause in ['(WITH q AS(SELECT * FROM a) SELECT * FROM q) q','((SELECT * FROM a) q JOIN b USING(x)) g', '(a JOIN (SELECT * FROM b) q USING(x)) g']:
        compare(lib,SEED+[f'SELECT * FROM c,{clause}',f'CREATE TABLE result AS SELECT * FROM c,{clause}','PRAGMA table_info(result)'])
        cases+=1
    with tempfile.TemporaryDirectory(prefix='safe-join-scope-') as folder:
        folder=Path(folder)
        for n,(size,encoding,mode) in enumerate(itertools.product([512,1024,2048,4096,8192,16384,32768,65536], ['UTF-8','UTF-16le','UTF-16be'],[0,1,2])):
            source=folder/f'{n}.db'; result=folder/f'{n}-rust.db'; final=folder/f'{n}-final.db'
            native(source,f"PRAGMA page_size={size};PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE a(x TEXT,y INT);CREATE TABLE b(x TEXT,z INT);CREATE TABLE c(w INT);CREATE VIEW v AS SELECT w,g.x,a.y,b.z FROM c,(a FULL JOIN b USING(x)) g;INSERT INTO a VALUES('é',1),('Rust',2);INSERT INTO b VALUES('Rust',3),('🦀',4);INSERT INTO c VALUES(1);")
            sql='CREATE TABLE copied AS SELECT * FROM v;INSERT INTO a SELECT x,10 FROM v WHERE y IS NULL;'
            run_engine(sql,'--load',source,'--save',result);native(source,sql)
            for q in ['SELECT * FROM v ORDER BY x','SELECT * FROM copied ORDER BY x','PRAGMA table_info(v)','PRAGMA table_info(copied)']:
                assert native(result,q)==native(source,q),(n,q)
            assert native(result,'PRAGMA integrity_check')==[{'integrity_check':'ok'}]
            native(result,"INSERT INTO b VALUES('native',5)")
            run_engine('UPDATE c SET w=2;','--load',result,'--save',final);native(result,'UPDATE c SET w=2;')
            assert native(final,'SELECT * FROM v ORDER BY x')==native(result,'SELECT * FROM v ORDER BY x')
            assert native(final,'PRAGMA integrity_check')==[{'integrity_check':'ok'}]
            cases+=1
    print(f'PASS: {cases} parenthesized join/namespace/ON dependency scenarios against SQLite 3.53.4')

if __name__=='__main__': main()
