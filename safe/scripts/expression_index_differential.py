#!/usr/bin/env python3
"""Expression and partial index metadata, conflicts, UPSERT matching and files."""
import itertools
from pathlib import Path
import subprocess
import tempfile
from conflict_differential import reference_library, compare, POLICIES
from sql_differential import ROOT, run_engine
from index_differential import native

INDEXES = [
    ('x+1',''),('abs(x)',''),('lower(x)',''),('length(x),y DESC',''),
    ('(x||y) COLLATE NOCASE',''),('x COLLATE nocase',' WHERE y>0'),
    ('lower(x)',' WHERE y IS NOT NULL'),('x+1',' WHERE y>0'),
    ('x',' WHERE y>0'),('x',' WHERE id>1'),('x',' WHERE rowid>1'),
    ('x',' WHERE y'),('x',' WHERE y IS NULL'),('x',' WHERE 0'),
    ('x',' WHERE NULL'),('x',' WHERE 1'),('1',''),('g',' WHERE y>0'),
    ('g+1',' WHERE g>0'),('abs(x)',' WHERE x != -9223372036854775808'),
]
SCHEMA = 'id INTEGER PRIMARY KEY,x ANY,y INT,g INT AS(length(x))'


def main():
    subprocess.run(['cargo','build','--example','statement_probe','--bin','sqlite-safe-sql'],cwd=ROOT,check=True)
    lib = reference_library(); cases = 0
    for (key,predicate),unique,suffix in itertools.product(INDEXES,['','UNIQUE '],['',' STRICT',' WITHOUT ROWID']):
        compare(lib,[f'CREATE TABLE t({SCHEMA}){suffix}', f'CREATE {unique}INDEX ix ON t({key}){predicate}',
            'PRAGMA index_list(t)','PRAGMA index_info(ix)','PRAGMA index_xinfo(ix)',
            "INSERT INTO t(id,x,y) VALUES(1,'a',0),(2,'A',1),(3,'bbb',NULL)",
            'SELECT * FROM t ORDER BY id','UPDATE OR IGNORE t SET y=1','SELECT * FROM t ORDER BY id',
            "INSERT INTO t(id,x,y) VALUES(4,'a',0),(5,'A',1) ON CONFLICT DO NOTHING RETURNING *",'SELECT * FROM t ORDER BY id',
            "UPDATE t SET x=x||id WHERE id=1 RETURNING *",'SELECT * FROM t ORDER BY id','DROP INDEX ix','PRAGMA index_list(t)'])
        cases += 1
    for (key,predicate),policy,begin,dml in itertools.product(INDEXES, ['',*[' OR '+p for p in POLICIES]], [[],['BEGIN'],['SAVEPOINT s']], [
        'INSERT{p} INTO t(id,x,y) VALUES(3,3,1),(4,2,1),(5,NULL,0)',
        'UPDATE{p} t SET y=1', 'UPDATE{p} t SET x=1,y=1',
        'INSERT{p} INTO t(id,x,y) VALUES(3,3,1),(4,-9223372036854775808,1)',
    ]):
        compare(lib,[f'CREATE TABLE t({SCHEMA})',f'CREATE UNIQUE INDEX ix ON t({key}){predicate}',
            'INSERT INTO t(id,x,y) VALUES(1,1,0),(2,2,1)',*begin,dml.format(p=policy),'SELECT * FROM t ORDER BY id',
            'ROLLBACK TO s','SELECT * FROM t ORDER BY id','COMMIT'])
        cases += 1
    for key,target in itertools.product(['x+1','lower(x)','CAST(x AS INT)','x+1.0',"x||x'AB'",'1','1.0','id+1','+x','(x+1) COLLATE nocase'], [
        'x+1','1+x','x+01','x+0x1','x+1.0','x+1.00','LOWER(x)','CAST(x AS INT)','CAST(x AS INTEGER)',
        'CAST(x AS int)',"x||x'AB'","x||x'ab'",'1','1 COLLATE BINARY','1.0','1.00','id+1','rowid+1','+x','x','(x+1) COLLATE nocase',
    ]):
        compare(lib,['CREATE TABLE t(id INTEGER PRIMARY KEY,x)',f'CREATE UNIQUE INDEX ix ON t({key})',
            'INSERT INTO t VALUES(1,1)',f'INSERT INTO t VALUES(2,1) ON CONFLICT({target}) DO UPDATE SET x=excluded.x+10 RETURNING *','SELECT * FROM t ORDER BY id'])
        cases += 1
    for predicate,target in itertools.product(['x>0','x IS NOT NULL','y=1 AND x>0','y=1 OR x>0','y>0 COLLATE BINARY'],[
        '', ' WHERE x>0',' WHERE (x>0)',' WHERE 0<x',' WHERE x>00',' WHERE x>0.0',' WHERE x IS NOT NULL',
        ' WHERE y=1 AND x>0',' WHERE x>0 AND y=1',' WHERE y=1 OR x>0',' WHERE y>0 COLLATE BINARY',
    ]):
        compare(lib,['CREATE TABLE t(x,y)',f'CREATE UNIQUE INDEX ix ON t(lower(x)) WHERE {predicate}',
            'INSERT INTO t VALUES(1,1)',f'INSERT INTO t VALUES(1,1) ON CONFLICT(lower(x)){target} DO UPDATE SET y=y+1 RETURNING *','SELECT * FROM t'])
        cases += 1
    for expr in ['t.x','rowid','oid','?1','changes()','last_insert_rowid()','sum(x)','(SELECT 1)',"'x'","'missing'",'"missing"','(x COLLATE NoCaSe) COLLATE nocase', 'x||y COLLATE NOCASE']:
        compare(lib,['CREATE TABLE t(x TEXT COLLATE RTRIM,y)',f'CREATE INDEX ix ON t({expr})','PRAGMA index_xinfo(ix)',"INSERT INTO t VALUES('a',1)"])
        cases += 1
    for predicate in ['t.x>0','rowid>0','?1','changes()','sum(x)','(SELECT 1)','unknown>1','"missing"']:
        compare(lib,['CREATE TABLE t(x,y)',f'CREATE INDEX ix ON t(x) WHERE {predicate}','PRAGMA index_list(t)','INSERT INTO t VALUES(1,1)'])
        cases += 1
    for unique, predicate in itertools.product(['','UNIQUE '],['',' WHERE x>0',' WHERE 0']):
        compare(lib,['CREATE TABLE t(x)','INSERT INTO t VALUES(1),(-9223372036854775808)',f'CREATE {unique}INDEX ix ON t(abs(x)){predicate}','PRAGMA index_list(t)','SELECT * FROM t ORDER BY x'])
        cases += 1
    for key,predicate,unique in itertools.product(['x','x+1','abs(x)','coalesce(x,1)','iif(x,1,2)'],['',' WHERE y>0',' WHERE abs(y)>0',' WHERE 0',' WHERE coalesce(y,1)>0'],['','UNIQUE ']):
        compare(lib,['CREATE TABLE t(x INT,y INT) STRICT',f'CREATE {unique}INDEX ix ON t({key}){predicate}','BEGIN',"INSERT INTO t VALUES(1,1),('bad',1)",'SELECT * FROM t','COMMIT'])
        cases+=1
    for key,storage in itertools.product(['abs(x)','length(x)','coalesce(x,1)','iif(x,1,2)'],['VIRTUAL','STORED']):
        compare(lib,[f'CREATE TABLE t(x INT,g INT AS({key}) {storage}) STRICT','BEGIN',"INSERT INTO t VALUES(1),('bad')",'SELECT * FROM t','COMMIT'])
        cases+=1
    with tempfile.TemporaryDirectory(prefix='safe-expression-index-') as folder:
        folder=Path(folder)
        for n,(size,encoding,mode,suffix) in enumerate(itertools.product([512,1024,2048,4096,8192,16384,32768,65536],['UTF-8','UTF-16le','UTF-16be'],[0,1,2],['',' WITHOUT ROWID'])):
            source=folder/f'{n}-native.db'; result=folder/f'{n}-rust.db'; final=folder/f'{n}-final.db'
            native(source,f"PRAGMA page_size={size};PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(id INTEGER PRIMARY KEY,x TEXT,y INT,g TEXT AS(upper(x))){suffix};CREATE UNIQUE INDEX ix ON t(lower(x) COLLATE NOCASE) WHERE y>0;CREATE INDEX iy ON t(length(x) DESC,g||id) WHERE y IS NOT NULL;INSERT INTO t(id,x,y) VALUES(1,'é',0),(2,'🦀',1),(3,'RUST',NULL);")
            sql="UPDATE t SET x=x||id;INSERT INTO t(id,x,y) VALUES(4,'new',1);CREATE INDEX iz ON t((id+y) DESC) WHERE x!='';"
            run_engine(sql,'--load',source,'--save',result);native(source,sql)
            for q in ['SELECT * FROM t ORDER BY id','PRAGMA index_list(t)','PRAGMA index_xinfo(ix)','PRAGMA index_xinfo(iy)','PRAGMA index_xinfo(iz)']:
                assert native(result,q)==native(source,q),(n,q)
            assert native(result,'PRAGMA integrity_check')==[{'integrity_check':'ok'}],n
            for ix,pred in [('ix','y>0'),('iy','y IS NOT NULL'),('iz',"x!=''")]:
                assert native(result,f'SELECT * FROM t INDEXED BY {ix} WHERE {pred} ORDER BY id')==native(source,f'SELECT * FROM t WHERE {pred} ORDER BY id'),(n,ix)
            native(result,"UPDATE t SET y=1 WHERE id=3;DELETE FROM t WHERE id=2;INSERT INTO t(id,x,y) VALUES(5,'native',0);")
            run_engine("UPDATE t SET y=1 WHERE id=5;",'--load',result,'--save',final);native(result,'UPDATE t SET y=1 WHERE id=5;')
            assert native(final,'SELECT * FROM t ORDER BY id')==native(result,'SELECT * FROM t ORDER BY id')
            assert native(final,'PRAGMA integrity_check')==[{'integrity_check':'ok'}],n
            cases+=1
        for n,(size,encoding) in enumerate(itertools.product([512,1024,2048,4096,8192,16384,32768,65536],['UTF-8','UTF-16le','UTF-16be'])):
            source=folder/f'large-{n}-empty.db';result=folder/f'large-{n}.db';final=folder/f'large-{n}-final.db'
            native(source,f"PRAGMA page_size={size};PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={n%3};VACUUM;")
            suffix=' WITHOUT ROWID' if n%2 else ''
            values=','.join(f"({i},'é-{i:04d}-{'a'*750}',{i%3})" for i in range(90))
            run_engine(f"CREATE TABLE t(id INT PRIMARY KEY,x TEXT,y INT){suffix};CREATE UNIQUE INDEX ix ON t(lower(x)||x COLLATE BINARY) WHERE y>0;INSERT INTO t VALUES{values};",'--load',source,'--save',result)
            assert native(result,'PRAGMA integrity_check')==[{'integrity_check':'ok'}]
            assert native(result,'SELECT count(*) AS n FROM t INDEXED BY ix WHERE y>0')==[{'n':60}]
            native(result,"UPDATE t SET y=1 WHERE id<10;DELETE FROM t WHERE id BETWEEN 20 AND 40;")
            run_engine("UPDATE t SET x=x||id WHERE id>70;",'--load',result,'--save',final)
            native(result,"UPDATE t SET x=x||id WHERE id>70;")
            assert native(final,'SELECT * FROM t ORDER BY id')==native(result,'SELECT * FROM t ORDER BY id')
            assert native(final,'PRAGMA integrity_check')==[{'integrity_check':'ok'}]
            assert native(final,'SELECT id FROM t INDEXED BY ix WHERE y>0 ORDER BY id')==native(result,'SELECT id FROM t WHERE y>0 ORDER BY id')
            cases+=1
    print(f'PASS: {cases} expression/partial-index declaration/constraint/UPSERT/metadata/image scenarios against SQLite 3.53.4')


if __name__=='__main__':main()
