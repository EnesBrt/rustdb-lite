#!/usr/bin/env python3
"""Compare scoped, derived, VALUES and compound queries with native SQLite."""
import subprocess
import tempfile
from pathlib import Path
from sql_differential import ROOT, check, run_engine
from constraint_differential import outcome
from index_differential import native
from encoding_differential import expected


def main():
    subprocess.run(['cargo','build','--bin','sqlite-safe-sql'],cwd=ROOT,check=True)
    setup="CREATE TABLE t(id INTEGER PRIMARY KEY,a TEXT COLLATE NOCASE,b NUMERIC);INSERT INTO t VALUES(1,'a',1),(2,'A',2),(3,'b',NULL),(4,NULL,'3');"
    queries=[
        ('SELECT s.a c0,s.b c1 FROM (SELECT a,b FROM t WHERE id<4) s ORDER BY s.b',2),
        ("SELECT x='A' c0 FROM (SELECT a x FROM t WHERE id=1)",1),
        ("SELECT x='1' c0 FROM (SELECT b x FROM t WHERE id=1)",1),
        ('SELECT s.a c0,t.id c1 FROM (SELECT id,a FROM t) s LEFT JOIN t ON s.id=t.id-1 ORDER BY s.id',2),
        ('SELECT a c0,count(*) c1 FROM (SELECT a FROM t) GROUP BY a ORDER BY a',2),
        ('SELECT a c0,"a:1" c1,"a:2" c2 FROM (SELECT 1 a,2 a,3 "a:1")',3),
        ('SELECT column1 c0,column2 c1 FROM (VALUES(1,2),(3,4)) ORDER BY column1 DESC',2),
        ('SELECT column1=1 c0 FROM (VALUES(CAST(1 AS TEXT)))',1),
        ('SELECT a c0 FROM (SELECT a FROM (SELECT a FROM t) ORDER BY a DESC LIMIT 2) ORDER BY a',1),
        ('WITH q(x,y) AS (SELECT a,b FROM t) SELECT x c0,y c1 FROM q ORDER BY y',2),
        ('WITH q AS (SELECT a,b FROM t) SELECT l.a c0,r.b c1 FROM q l JOIN q r ON l.b=r.b ORDER BY l.b',2),
        ('WITH q AS (SELECT * FROM later),later(x) AS (VALUES(2),(1)) SELECT x c0 FROM q ORDER BY x',1),
        ('WITH t(x) AS (VALUES(9)) SELECT x c0 FROM t',1),
        ('WITH t(x) AS (VALUES(9)) SELECT id c0 FROM main.t ORDER BY id',1),
        ('WITH q(x) AS (VALUES(1)) SELECT x c0 FROM (WITH q(x) AS (VALUES(2)) SELECT x FROM q)',1),
        ('WITH q(x) AS (VALUES(1)),r AS (SELECT x FROM q) SELECT r.x c0,s.x c1 FROM r,(WITH q(x) AS(VALUES(2)) SELECT x FROM q) s',2),
        ('WITH RECURSIVE q(x) AS NOT MATERIALIZED (VALUES(2),(1)) SELECT x c0 FROM q ORDER BY x',1),
        ('WITH q(x) AS MATERIALIZED (VALUES(2),(1)) SELECT x c0 FROM q ORDER BY x',1),
        ('WITH unused AS (SELECT missing FROM nowhere) SELECT 1 c0',1),
        ('WITH unused AS (SELECT abs(-9223372036854775808)) SELECT 1 c0',1),
        ('SELECT x c0 FROM (SELECT abs(-9223372036854775808) x) LIMIT 0',1),
        ('SELECT 1 c0 UNION SELECT 1.0',1),
        ("SELECT 'a' c0 UNION SELECT 'A' COLLATE NOCASE",1),
        ("SELECT c0='A' c0 FROM (SELECT 'A' c0 UNION SELECT 'a' COLLATE NOCASE)",1),
        ('SELECT 1 c0 UNION ALL SELECT 2 ORDER BY c0 DESC',1),
        ('SELECT b c0 FROM t UNION SELECT id FROM t ORDER BY c0',1),
        ('SELECT b c0 FROM t EXCEPT SELECT id FROM t ORDER BY c0',1),
        ('SELECT b c0 FROM t INTERSECT SELECT id FROM t ORDER BY c0 DESC',1),
        ("SELECT a c0 FROM t UNION SELECT 'B' ORDER BY c0",1),
        ('SELECT a c0,b c1 FROM t UNION SELECT a,b FROM t ORDER BY 2,1',2),
        ('SELECT b+1 c0 FROM t UNION ALL SELECT id+2 FROM t ORDER BY b+1 LIMIT 4 OFFSET 2',1),
        ('SELECT b+1 c0 FROM t UNION ALL SELECT id+2 later FROM t ORDER BY later DESC NULLS FIRST LIMIT 3',1),
        ('SELECT NULL c0 UNION SELECT NULL UNION SELECT 1 ORDER BY c0 DESC NULLS LAST',1),
        ('SELECT 1 c0 UNION SELECT 2 INTERSECT SELECT 2 EXCEPT SELECT 3 ORDER BY c0',1),
        ('SELECT column1 c0 FROM (VALUES(1),(2) UNION SELECT 3 ORDER BY 1 DESC)',1),
        ('SELECT 1 c0 UNION VALUES(2),(3)',1),
        ("SELECT 'a' c0 UNION SELECT 'A' COLLATE NOCASE ORDER BY c0 COLLATE BINARY",1),
        ("SELECT 'a' COLLATE NOCASE c0 UNION SELECT 'A' ORDER BY c0 COLLATE BINARY",1),
        ('WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM q WHERE x<20) SELECT x c0 FROM q ORDER BY x DESC',1),
        ('WITH q(x) AS(SELECT 1 UNION ALL SELECT x+1 FROM q LIMIT 10 OFFSET 3) SELECT x c0 FROM q',1),
        ('WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM q) SELECT x c0 FROM q LIMIT 10 OFFSET 2',1),
        ('WITH RECURSIVE q(x) AS(VALUES(1),(3) UNION ALL SELECT x+1 FROM q WHERE x<5) SELECT x c0 FROM q',1),
        ('WITH RECURSIVE q(x) AS(VALUES(1) UNION SELECT (x+1)%5 FROM q) SELECT x c0 FROM q ORDER BY x',1),
        ('WITH RECURSIVE q(x) AS(VALUES(NULL) UNION SELECT x FROM q) SELECT x c0 FROM q',1),
        ("WITH RECURSIVE q(x) AS(SELECT 'a' COLLATE NOCASE UNION SELECT upper(x) FROM q) SELECT x c0 FROM q",1),
        ('WITH RECURSIVE q(x) AS(SELECT 1 x UNION ALL SELECT x+1 FROM q WHERE x<5 ORDER BY x DESC) SELECT x c0 FROM q',1),
        ('WITH RECURSIVE q(x,d) AS(VALUES(1,0) UNION ALL SELECT x*2,d+1 FROM q WHERE d<3 UNION ALL SELECT x*2+1,d+1 FROM q WHERE d<3 ORDER BY 2 DESC,1) SELECT x c0,d c1 FROM q',2),
        ('WITH RECURSIVE q(x) AS(VALUES(1) UNION SELECT x+1 FROM q WHERE x<10 UNION SELECT x+2 FROM q WHERE x<9) SELECT x c0 FROM q ORDER BY x',1),
        ('WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM q LIMIT 0) SELECT count(*) c0 FROM q',1),
        ('WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM q WHERE x<4) SELECT a.x c0,b.x c1 FROM q a JOIN q b ON a.x=b.x ORDER BY a.x',2),
        ('WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT id FROM t JOIN q ON id=q.x+1) SELECT x c0 FROM q ORDER BY x',1),
        ('WITH q(x) AS(SELECT 1 a UNION ALL SELECT 2 b UNION ALL SELECT x+1 FROM q WHERE x<3 ORDER BY b DESC) SELECT x c0 FROM q',1),
        ('WITH q(x) AS(SELECT 1 UNION ALL SELECT 2-0 UNION ALL SELECT x+1 FROM q WHERE x<3 ORDER BY 2-0 DESC) SELECT x c0 FROM q',1),
        ('WITH a(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM a WHERE x<3),b(y) AS(VALUES(0) UNION ALL SELECT y+x FROM b,a WHERE y<2) SELECT y c0 FROM b ORDER BY y',1),
    ]
    cases=0
    for query,columns in queries:
        check(setup,query,columns)
        cases+=1
    for first in ['NULL','1','1.0',"'1'","'a' COLLATE NOCASE","'A'", "x'31'"]:
        for second in ['NULL','1','1.0',"'A'", "'a' COLLATE NOCASE"]:
            for operator in ['UNION','UNION ALL','INTERSECT','EXCEPT']:
                check('',f'SELECT {first} c0 {operator} SELECT {second} ORDER BY c0',1)
                cases+=1
    for query in [
        'SELECT * FROM (SELECT missing)',
        'SELECT rowid FROM (SELECT 1 a)',
        'SELECT * FROM (SELECT 1 a,2 a) WHERE missing',
        'WITH q AS(SELECT * FROM q) SELECT * FROM q',
        'WITH q AS(SELECT * FROM r),r AS(SELECT * FROM q) SELECT * FROM q',
        'WITH q AS(SELECT 1),Q AS(SELECT 2) SELECT 3',
        'WITH q(a,b) AS(SELECT 1) SELECT * FROM q',
        'SELECT 1 UNION SELECT 2,3',
        'VALUES(1),(2,3)',
        'SELECT 1 UNION SELECT 2 ORDER BY 3',
        'SELECT 1 x UNION SELECT 2 ORDER BY +x',
        'SELECT 1 x UNION SELECT 2 ORDER BY x+1',
        'SELECT 1 LIMIT 2 UNION SELECT 3',
        'VALUES(1) ORDER BY 1',
        'WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT sum(x) FROM q) SELECT * FROM q',
        'WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT a.x FROM q a,q b) SELECT * FROM q',
        'WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT x FROM (SELECT x FROM q)) SELECT * FROM q',
        'WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM q UNION SELECT x+2 FROM q) SELECT * FROM q',
        'WITH RECURSIVE q(x) AS(VALUES(1) INTERSECT SELECT x FROM q) SELECT * FROM q',
        'WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT x,x FROM q) SELECT * FROM q',
    ]:
        outcome(query+';')
        cases+=1
    statements=[
        'CREATE TABLE out(x,y);WITH q(x,y) AS (VALUES(1,2),(3,4)) INSERT INTO out SELECT * FROM q;',
        'CREATE TABLE out(x,y);INSERT INTO out WITH q(x,y) AS (VALUES(1,2),(3,4)) SELECT * FROM q;',
        'CREATE TABLE out(x,y);INSERT INTO out SELECT 1,2 UNION ALL SELECT 3,4;',
        'CREATE TABLE out(x,y);INSERT INTO out VALUES(1,2) UNION ALL SELECT 3,4;',
        'CREATE TABLE out(x,y);WITH unused AS(SELECT missing) INSERT INTO out VALUES(1,2);',
        'CREATE TABLE out(x,y);WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM q WHERE x<5) INSERT INTO out SELECT x,x*2 FROM q;',
        'CREATE TABLE out(x,y);INSERT INTO out VALUES(1,2);WITH unused AS(SELECT missing) UPDATE out SET y=3;',
    ]
    for script in statements:
        check(script,'SELECT x c0,y c1 FROM out ORDER BY x',2)
        cases+=1
    with tempfile.TemporaryDirectory(prefix='safe-queries-') as directory:
        for encoding in ['UTF-8','UTF-16le','UTF-16be']:
            path=Path(directory)/f'{encoding}.db'
            native(path,f"PRAGMA encoding='{encoding}';"+setup.replace("'A',2","'Ā',2").replace("'b',NULL","'🦀',NULL"))
            for query,columns in queries[:18]:
                actual=run_engine(query+';','--load',path)[-1]['rows']
                want=expected(path,query,columns,encoding)
                assert actual==want,(encoding,query,actual,want)
                cases+=1
    print(f'PASS: {cases} scoped/derived/compound query scenarios against SQLite 3.53.4')


if __name__=='__main__':main()
