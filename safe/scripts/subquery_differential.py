#!/usr/bin/env python3
"""Check expression subqueries against the pinned native test oracle."""
import subprocess
import tempfile
from pathlib import Path
from sql_differential import ROOT, check, run_engine
from constraint_differential import outcome
from encoding_differential import expected
from index_differential import native

SETUP = "CREATE TABLE t(id INTEGER PRIMARY KEY,x NUMERIC,y TEXT COLLATE NOCASE);INSERT INTO t VALUES(1,1,'a'),(2,2,'B'),(3,NULL,NULL),(4,'3','c');CREATE TABLE u(k INTEGER,v TEXT);INSERT INTO u VALUES(1,'one'),(1,'again'),(2,'two'),(NULL,NULL);"

QUERIES = [
    ('SELECT (SELECT 1) c0,(SELECT NULL) c1,(SELECT x FROM t WHERE 0) c2',3),
    ('SELECT (SELECT x FROM t ORDER BY id DESC) c0',1),
    ('SELECT (SELECT x FROM t ORDER BY id LIMIT 20 OFFSET 1) c0',1),
    ('SELECT (SELECT x FROM t LIMIT 0) c0,(SELECT x FROM t LIMIT -1 OFFSET 20) c1',2),
    ('SELECT (VALUES(1),(abs(-9223372036854775808))) c0',1),
    ('SELECT (SELECT 1 UNION ALL SELECT abs(-9223372036854775808)) c0',1),
    ('SELECT (SELECT 3 UNION ALL SELECT 2 UNION ALL SELECT 1 ORDER BY 1) c0',1),
    ('SELECT (SELECT 1 UNION SELECT 2 EXCEPT SELECT 1) c0',1),
    ('SELECT id c0,(SELECT v FROM u WHERE k=t.id ORDER BY v) c1 FROM t ORDER BY id',2),
    ('SELECT id c0,(SELECT count(*) FROM u WHERE k=t.id) c1 FROM t ORDER BY id',2),
    ('SELECT id c0,(SELECT (SELECT t.y)) c1 FROM t ORDER BY id',2),
    ('SELECT id c0,(SELECT (SELECT u.v||t.y) FROM u WHERE k=t.id ORDER BY v) c1 FROM t ORDER BY id',2),
    ('SELECT id c0 FROM t WHERE EXISTS(SELECT 1 FROM u WHERE k=t.id) ORDER BY id',1),
    ('SELECT id c0 FROM t WHERE NOT EXISTS(SELECT NULL FROM u WHERE k=t.id) ORDER BY id',1),
    ('SELECT EXISTS(SELECT abs(-9223372036854775808) FROM t) c0',1),
    ('SELECT EXISTS(SELECT DISTINCT abs(-9223372036854775808) FROM t LIMIT 1 OFFSET 1) c0',1),
    ('SELECT EXISTS(SELECT abs(-9223372036854775808) UNION ALL SELECT 2) c0',1),
    ('SELECT EXISTS(VALUES(abs(-9223372036854775808))) c0',1),
    ('SELECT EXISTS(SELECT sum(x) FROM t) c0',1),
    ('SELECT EXISTS(SELECT 1 FROM t WHERE 0) c0,EXISTS(SELECT count(*) FROM t WHERE 0) c1',2),
    ('SELECT EXISTS(SELECT count(*) FROM t HAVING count(*)>10) c0',1),
    ('SELECT EXISTS(SELECT x FROM t GROUP BY x HAVING count(*)>0) c0',1),
    ('SELECT EXISTS(SELECT 1 INTERSECT SELECT 2) c0,EXISTS(SELECT 1 EXCEPT SELECT 2) c1',2),
    ('SELECT id c0, x IN(SELECT k FROM u) c1,x NOT IN(SELECT k FROM u) c2 FROM t ORDER BY id',3),
    ('SELECT id c0, x IN(SELECT k FROM u WHERE u.k<t.id) c1 FROM t ORDER BY id',2),
    ('SELECT id c0 FROM t WHERE id IN(SELECT k FROM u WHERE v IS NOT NULL) ORDER BY id',1),
    ("SELECT 'A'=(SELECT y FROM t WHERE id=1) c0,(SELECT y FROM t WHERE id=1)='A' c1",2),
    ("SELECT '1'=(SELECT x FROM t WHERE id=1) c0,(SELECT x FROM t WHERE id=1)='1' c1",2),
    ('SELECT sum((SELECT k FROM u WHERE k=t.id)) c0 FROM t',1),
    ('SELECT (SELECT count(*) FROM u WHERE k=t.id) c0,count(*) c1 FROM t GROUP BY c0 ORDER BY c0',2),
    ('SELECT id c0 FROM t ORDER BY (SELECT v FROM u WHERE k=t.id),id',1),
    ('SELECT t.id c0,u.v c1 FROM t LEFT JOIN u ON u.k=(SELECT t.x) ORDER BY t.id,u.v',2),
    ('SELECT CASE WHEN 0 THEN (SELECT abs(-9223372036854775808)) ELSE 7 END c0',1),
    ('SELECT coalesce(1,(SELECT abs(-9223372036854775808))) c0',1),
    ('SELECT iif(0,(SELECT abs(-9223372036854775808)),3) c0',1),
    ('SELECT CASE WHEN 0 THEN (SELECT 1 LIMIT abs(-9223372036854775808)) ELSE 7 END c0',1),
    ('SELECT (SELECT 1 LIMIT (SELECT 1)) c0',1),
    ('SELECT id c0 FROM t LIMIT (SELECT 2) OFFSET (SELECT 1)',1),
    ('SELECT id c0,(WITH q AS(SELECT t.y v) SELECT v FROM q) c1 FROM t ORDER BY id',2),
    ('WITH q AS(SELECT y FROM t) SELECT (SELECT count(*) FROM q) c0',1),
    ('WITH q AS(SELECT t.x) SELECT (SELECT * FROM q) c0 FROM t ORDER BY id',1),
    ('WITH q(v) AS(VALUES(8)) SELECT id c0,(WITH q AS(SELECT t.id v) SELECT v FROM q) c1 FROM t ORDER BY id',2),
    ('SELECT (SELECT rowid FROM u WHERE k=t.id) c0 FROM t ORDER BY id',1),
    ('SELECT id c0,(SELECT x FROM (SELECT t.x x)) c1 FROM t ORDER BY id',2),
    ('SELECT id c0,(SELECT id FROM t WHERE id=2) c1 FROM t ORDER BY id',2),
    ('SELECT id c0,(SELECT t.id FROM u t WHERE k=1) c1 FROM t ORDER BY id',2),
    ('SELECT (WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM q WHERE x<5) SELECT sum(x) FROM q) c0',1),
    ('SELECT (WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM q) SELECT x FROM q) c0',1),
    ('WITH q(x) AS(VALUES(1) UNION ALL SELECT x+(SELECT 1) FROM q WHERE x<4) SELECT x c0 FROM q',1),
    ('WITH q(x) AS(VALUES(1) UNION ALL SELECT (SELECT x+1) FROM q WHERE x<3) SELECT x c0 FROM q',1),
    ('WITH q(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM q WHERE x<(SELECT 3)) SELECT x c0 FROM q',1),
]

def main():
    subprocess.run(['cargo','build','--bin','sqlite-safe-sql'],cwd=ROOT,check=True)
    count=0
    for query,columns in QUERIES:
        check(SETUP,query,columns);count+=1
    values=['NULL','1','1.0',"'1'","'a' COLLATE NOCASE","'A'","x'31'"]
    sources=['SELECT NULL WHERE 0','SELECT NULL','VALUES(1),(NULL)',"VALUES('1'),('A')",'SELECT 1 UNION SELECT 1.0',"SELECT 'a' COLLATE NOCASE",'SELECT x FROM t','SELECT y FROM t']
    for value in values:
        for query in sources:
            for negation in ['', 'NOT ']:
                check(SETUP,f'SELECT {value} {negation}IN({query}) c0',1);count+=1
    for affinity in ['', 'TEXT', 'NUMERIC', 'BLOB']:
        setup=f'CREATE TABLE a(x {affinity} COLLATE NOCASE);INSERT INTO a VALUES(1),(\'A\'),(NULL);'
        for value in values:
            check(setup,f'SELECT {value} IN(SELECT x FROM a) c0,{value}=(SELECT x FROM a LIMIT 1) c1',2);count+=1
    for script in [
        'CREATE TABLE out(x);INSERT INTO out VALUES((SELECT 1)),((SELECT count(*) FROM out));',
        'CREATE TABLE out(x);INSERT INTO out VALUES((SELECT last_insert_rowid())),((SELECT last_insert_rowid()));',
        'CREATE TABLE out(x);INSERT INTO out SELECT (SELECT count(*) FROM u WHERE u.k=t.id) FROM t;',
        'CREATE TABLE out(x);WITH q(v) AS(VALUES(3)) INSERT INTO out VALUES((SELECT v FROM q));',
        'CREATE TABLE out(x);INSERT INTO out SELECT id FROM t;UPDATE out SET x=(SELECT max(k) FROM u WHERE k<out.x);',
        'CREATE TABLE out(x);INSERT INTO out VALUES(1),(2),(3);UPDATE out SET x=x+(SELECT count(*) FROM out);',
        'CREATE TABLE out(x);INSERT INTO out VALUES(1),(2),(3);UPDATE out SET x=(SELECT sum(b.x) FROM out b WHERE b.x<=out.x);',
        'CREATE TABLE out(x);INSERT INTO out SELECT id FROM t;DELETE FROM out WHERE EXISTS(SELECT 1 FROM u WHERE k=out.x);',
        'CREATE TABLE out(x);INSERT INTO out SELECT id FROM t;WITH q(v) AS(VALUES(2),(3)) DELETE FROM out WHERE x IN(SELECT v FROM q);',
    ]:
        check(SETUP+script,'SELECT rowid c0,x c1 FROM out ORDER BY rowid',2);count+=1
    for query in [
        'SELECT (SELECT 1,2)', 'SELECT 1 IN(SELECT 1,2)',
        'SELECT CASE WHEN 0 THEN (SELECT missing) END',
        'SELECT coalesce(1,(SELECT missing))', 'SELECT EXISTS(SELECT no_such_function(1))',
        'SELECT EXISTS(SELECT 1 ORDER BY missing)',
        'SELECT EXISTS(SELECT sum(abs(-9223372036854775808)) FROM t)',
        'SELECT x AS alias,(SELECT alias) FROM t',
        'SELECT (SELECT 1 LIMIT t.id) FROM t',
        'SELECT (SELECT id FROM t a,t b) FROM t',
        'SELECT * FROM t,(SELECT t.x)',
        'SELECT (SELECT 1 LIMIT (SELECT t.id)) FROM t',
        'SELECT (WITH q AS(SELECT t.id n) SELECT 1 LIMIT (SELECT n FROM q)) FROM t',
        'WITH q(x) AS(VALUES(1) UNION ALL SELECT (SELECT x FROM q) FROM q WHERE x<3) SELECT * FROM q',
        'CREATE TABLE bad(x CHECK(x IN(SELECT 1)))',
        'CREATE TABLE bad(x DEFAULT (SELECT 1))',
    ]:
        outcome(SETUP+query+';');count+=1
    with tempfile.TemporaryDirectory(prefix='safe-subqueries-') as directory:
        for encoding in ['UTF-8','UTF-16le','UTF-16be']:
            path=Path(directory)/f'{encoding}.db'
            native(path,f"PRAGMA encoding='{encoding}';"+SETUP)
            for query,columns in QUERIES[8:14]+QUERIES[23:28]+QUERIES[38:41]:
                actual=run_engine(query+';','--load',path)[-1]['rows']
                assert actual==expected(path,query,columns,encoding),(encoding,query,actual)
                count+=1
    print(f'PASS: {count} scalar/EXISTS/IN/correlated subquery scenarios against SQLite 3.53.4')

if __name__=='__main__': main()
