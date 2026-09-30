#!/usr/bin/env python3
"""Row comparisons, membership and assignments against SQLite 3.53.4."""
import itertools
import subprocess
import tempfile
from pathlib import Path
from conflict_differential import reference_library, compare
from sql_differential import ROOT, run_engine
from index_differential import native

SEED=['CREATE TABLE t(id INTEGER PRIMARY KEY,a TEXT COLLATE NOCASE,b NUMERIC,c BLOB)',
      "INSERT INTO t VALUES(1,'A',1,x'31'),(2,'a',2,x'00'),(3,'B',NULL,NULL),(4,NULL,4,x'FF')"]

def main():
    subprocess.run(['cargo','build','--example','statement_probe','--bin','sqlite-safe-sql'],cwd=ROOT,check=True)
    lib=reference_library();cases=0
    vectors=['(1,2)','(1,NULL)','(NULL,2)','(NULL,NULL)',"('1',2)","(1,'2')",'(1,2.0)','(0,3)','(2,1)',"(x'31',2)"]
    for lhs,rhs,op in itertools.product(vectors,vectors,['=','!=','<','<=','>','>=','IS','IS NOT']):
        compare(lib,[f'SELECT {lhs} {op} {rhs}'])
        cases+=1
    for lhs,rhs,op in itertools.product(['(a,b)','(b,a)','(SELECT a,b)','(SELECT b,a)'],
        ["('a',1)","('A','2')",'(NULL,4)',"(SELECT 'a',1)","(SELECT 'a' COLLATE BINARY,1)","(SELECT 'a' COLLATE NOCASE,1)"],
        ['=','<','>','IS']):
        compare(lib,SEED+[f'SELECT id,{lhs} {op} {rhs},{rhs} {op} {lhs} FROM t ORDER BY id'])
        cases+=1
    for lhs,rhs,op in itertools.product(vectors,
        ['VALUES(1,2),(3,4)', 'VALUES(1,NULL),(3,4)', 'VALUES(NULL,2)', 'SELECT 1,2 WHERE 0', "SELECT a,b FROM t", "SELECT a,b FROM t UNION ALL SELECT 'a',2"],['IN','NOT IN']):
        compare(lib,SEED+[f'SELECT {lhs} {op} ({rhs})'])
        cases+=1
    for lhs,rhs in itertools.product(vectors,['((1,2),(3,4))','((1,NULL),(3,4))','((SELECT 1,2 UNION ALL SELECT 3,4))','()']):
        compare(lib,[f'SELECT {lhs} IN {rhs},{lhs} NOT IN {rhs}'])
        cases+=1
    for x,a,b in itertools.product(vectors[:4],vectors[:4],vectors[:4]):
        compare(lib,[f'SELECT {x} BETWEEN {a} AND {b},{x} NOT BETWEEN {a} AND {b}',f'SELECT CASE {x} WHEN {a} THEN 10 WHEN {b} THEN 20 ELSE 30 END'])
        cases+=1
    for q in [
        'SELECT (1,abs(-9223372036854775808))<(2,0)',
        'SELECT (1,abs(-9223372036854775808))=(2,0)',
        'SELECT (1,abs(-9223372036854775808)) BETWEEN (2,0) AND (3,0)',
        'SELECT CASE (1,abs(-9223372036854775808)) WHEN (2,0) THEN 1 ELSE 0 END',
        'SELECT (1,2) IN ((1,2),(abs(-9223372036854775808),3))',
        'SELECT (SELECT 1,2)=(1,2)', 'SELECT (SELECT 1,2 WHERE 0) IS (NULL,NULL)',
        'SELECT (SELECT 1,2 UNION ALL SELECT 2,3)=(1,2)',
        'SELECT (SELECT 1,2) IN ((1,2),(3,4))',
        'SELECT (EXISTS(SELECT 1)) IN (VALUES(1))',
        'SELECT 2 IN ((SELECT 1 UNION ALL SELECT 2))',
        'SELECT (1,2)', 'SELECT abs((1,2))', 'SELECT (1,2)=(1)',
        'SELECT (1,2) IS NULL', 'SELECT ((1,2),3)=((1,2),3)',
        'SELECT +(1,2)=(1,2)', 'SELECT (1,2) COLLATE BINARY=(1,2)',
        'SELECT CASE WHEN 1 THEN 1 ELSE (2,3) END',
        'SELECT (1,2) IN (1,2)', 'SELECT (1,2) IN ((1,2),(3,4,5))',
        'SELECT (1,2) IN (SELECT 1)', 'SELECT 1 IN (SELECT 1,2)',
        'SELECT * FROM t WHERE (a,b)', 'SELECT (SELECT a,b FROM t)',
        'SELECT (a,b) IN ((a,b),(b,a)) FROM t ORDER BY id',
        "SELECT (a,b) IN ((SELECT a,b)) FROM t ORDER BY id",
        "SELECT (a,b) BETWEEN ('a',1) AND ('b',3) FROM t ORDER BY id",
    ]:
        compare(lib,SEED+[q]);cases+=1
    for source in ['q','main.q','q()']:
        compare(lib,['CREATE TABLE q(a TEXT COLLATE NOCASE,b INT)',"INSERT INTO q VALUES('A',1),('b',NULL)",f"SELECT ('a',1) IN {source},('c',2) NOT IN {source}"])
        cases+=1
    compare(lib,['WITH q(a,b) AS(VALUES(1,2),(3,4)) SELECT (3,4) IN q']);cases+=1
    for assignment in ['(a,b)=(b,a)','(a,b)=(SELECT b,a)','(a,b)=(SELECT b,a WHERE id<3)',
                       '(a,b)=(SELECT b,a UNION ALL SELECT 8,9)',
                       '(a,b)=(SELECT max(b),min(a) FROM t)',
                       "(a,b)=('z',9),b=10", "a=abs(-9223372036854775808),(a,b)=('z',9)",
                       '(a,a)=(1,2)', '(a)=(SELECT b)', '(a,b)=(1,2,3)',
                       '(a,b)=(SELECT 1)', 'a=(SELECT 1,2)']:
        compare(lib,SEED+['BEGIN',f'UPDATE t SET {assignment} WHERE id IN (1,2) RETURNING id,a,b','SELECT * FROM t ORDER BY id','ROLLBACK'])
        cases+=1
    for lhs,rhs,op in itertools.product(
        list(itertools.product(['0','1','NULL'],repeat=3)),list(itertools.product(['0','1','NULL'],repeat=3)),['=','<','IS'],
    ):
        compare(lib,[f'SELECT ({",".join(lhs)}) {op} ({",".join(rhs)})'])
        cases+=1
    for q in ['SELECT (missing,2) IN ()','SELECT (abs(-9223372036854775808),2) IN ()',
        'SELECT (abs(-9223372036854775808),2) IN (SELECT 1,2 WHERE 0)',
        'SELECT (SELECT 1,2) IN ()','SELECT (EXISTS(SELECT 1),2) IN (VALUES(1,2))',
        'SELECT (1,2) IN (VALUES(1,2),(3,4))',
        'SELECT (a,b)=(SELECT a,b) FROM t ORDER BY id',
        'SELECT (a,b) IN (SELECT a,b FROM t b WHERE b.id=t.id) FROM t ORDER BY id',
        'SELECT id FROM t WHERE (a,b)>(SELECT a,b FROM t WHERE id=1) ORDER BY id',
    ]:
        compare(lib,SEED+[q]);cases+=1
    for suffix,policy in itertools.product(['',' WITHOUT ROWID'],['ABORT','FAIL','IGNORE','REPLACE','ROLLBACK']):
        statements=[f'CREATE TABLE t(id INT PRIMARY KEY,a TEXT UNIQUE,b INT,c INT AS(b+1) STORED){suffix}',
            "INSERT INTO t(id,a,b) VALUES(1,'a',1),(2,'b',2),(3,'c',3)", 'BEGIN',
            f"UPDATE OR {policy} t SET (a,b)=(SELECT 'new',b+10) RETURNING id,a,b,c", 'SELECT * FROM t ORDER BY id', 'ROLLBACK']
        compare(lib,statements);cases+=1
    for suffix in ['', ' WITHOUT ROWID', ' STRICT']:
        compare(lib,[f'CREATE TABLE t(id INT PRIMARY KEY,a TEXT,b INT){suffix}',"INSERT INTO t VALUES(1,'a',1),(2,'b',2)",
            "INSERT INTO t VALUES(1,'new',9) ON CONFLICT(id) DO UPDATE SET (a,b)=(SELECT excluded.a,excluded.b) RETURNING *",
            "UPDATE t SET (id,a,b)=(SELECT id+10,a,b*10) WHERE (id,b) IN (VALUES(2,2)) RETURNING *", 'SELECT * FROM t ORDER BY id'])
        cases+=1
    compare(lib,['CREATE TABLE t(a INT,b INT CHECK((a,b)>(0,0)),c INT AS((a,b)<(5,5)))',
        'CREATE UNIQUE INDEX i ON t((a,b)=(1,2)) WHERE (a,b)<(4,4)',
        'INSERT INTO t(a,b) VALUES(1,2),(2,3)',
        'INSERT INTO t(a,b) VALUES(3,3) ON CONFLICT((a,b)=(1,2)) WHERE (a,b)<(4,4) DO UPDATE SET (a,b)=(excluded.a,excluded.b)',
        'SELECT * FROM t ORDER BY a','INSERT INTO t(a,b) VALUES(-1,0)']);cases+=1
    with tempfile.TemporaryDirectory(prefix='safe-row-value-') as folder:
        folder=Path(folder)
        for n,(page_size,encoding,mode) in enumerate(itertools.product([512,1024,2048,4096,8192,16384,32768,65536],['UTF-8','UTF-16le','UTF-16be'],[0,1,2])):
            source=folder/f'{n}.db';result=folder/f'{n}-rust.db';final=folder/f'{n}-final.db'
            native(source,f"PRAGMA page_size={page_size};PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(id INT PRIMARY KEY,a TEXT,b INT,c INT AS(b+1) STORED);CREATE TABLE q(a TEXT,b INT);INSERT INTO t(id,a,b) VALUES(1,'é',1),(2,'Rust',2),(3,'🦀',3);INSERT INTO q VALUES('é',1),('🦀',3);CREATE VIEW v AS SELECT id,a,b,c FROM t WHERE (a,b) IN q;CREATE INDEX idx ON t((a,b)>('',0)) WHERE (id,b)>(0,0);")
            sql='CREATE TABLE copied AS SELECT * FROM v;UPDATE t SET (a,b)=(SELECT a,b+10) WHERE (a,b) IN q;'
            run_engine(sql,'--load',source,'--save',result);native(source,sql)
            for q in ['SELECT * FROM t ORDER BY id','SELECT * FROM copied ORDER BY id','SELECT * FROM v ORDER BY id','PRAGMA table_info(copied)']:
                assert native(result,q)==native(source,q),(n,q)
            assert native(result,'PRAGMA integrity_check')==[{'integrity_check':'ok'}]
            native(result,"INSERT INTO q VALUES('Rust',2)")
            run_engine('UPDATE t SET (a,b)=(SELECT a,b+1) WHERE (a,b) IN q;','--load',result,'--save',final)
            native(result,'UPDATE t SET (a,b)=(SELECT a,b+1) WHERE (a,b) IN q;')
            assert native(final,'SELECT * FROM t ORDER BY id')==native(result,'SELECT * FROM t ORDER BY id')
            assert native(final,'PRAGMA integrity_check')==[{'integrity_check':'ok'}]
            cases+=1
    print(f'PASS: {cases} row-value comparison/membership/assignment scenarios against SQLite 3.53.4')

if __name__=='__main__': main()
