#!/usr/bin/env python3
"""Compare WITHOUT ROWID SQL, primary storage and secondary index layouts."""
import itertools
from pathlib import Path
import subprocess
import tempfile

from conflict_differential import reference_library, compare, POLICIES
from sql_differential import ROOT, run_engine
from index_differential import native

ROWS = 'SELECT * FROM t ORDER BY id,x,y'
SCHEMAS = [
    'id INTEGER PRIMARY KEY,x TEXT,y INT',
    'id INTEGER PRIMARY KEY DESC,x TEXT,y INT',
    'id "INTEGER"(3) PRIMARY KEY,x TEXT UNIQUE,y INT',
    'id "INTEGER" extra,x TEXT UNIQUE,y INT,PRIMARY KEY(id)',
    'id INT PRIMARY KEY,x TEXT,y INT',
    'id TEXT PRIMARY KEY COLLATE NOCASE,x TEXT,y INT',
    'id TEXT,x TEXT,y INT,PRIMARY KEY(x,id DESC)',
    'id INT,x TEXT,y INT,PRIMARY KEY(id DESC,x)',
    'id INT,x TEXT,y INT,PRIMARY KEY(id,id,x,x)',
    'id TEXT,x TEXT,y INT,PRIMARY KEY(id COLLATE NOCASE,id COLLATE BINARY,x)',
    'id INTEGER UNIQUE PRIMARY KEY,x TEXT UNIQUE,y INT',
    'id INTEGER,x TEXT UNIQUE,y INT,PRIMARY KEY(id DESC)',
    'id INTEGER,x TEXT,y INT,UNIQUE(x),PRIMARY KEY(id DESC)',
    'id INT PRIMARY KEY ON CONFLICT REPLACE,x TEXT UNIQUE ON CONFLICT IGNORE,y INT',
    'id INT PRIMARY KEY,x TEXT NOT NULL ON CONFLICT REPLACE DEFAULT \'default\',y INT CHECK(y>0)',
]


def main():
    subprocess.run(['cargo','build','--example','statement_probe','--bin','sqlite-safe-sql'],cwd=ROOT,check=True)
    lib=reference_library();cases=0
    for schema, suffix in itertools.product(SCHEMAS, ['WITHOUT ROWID','STRICT, WITHOUT ROWID','WITHOUT ROWID,STRICT']):
        compare(lib,[f'CREATE TABLE t({schema}) {suffix}',
            'PRAGMA table_list(t)','PRAGMA table_info(t)','PRAGMA index_list(t)',
            *[f'PRAGMA {p}({n})' for p in ['index_info','index_xinfo'] for n in ['t',*[f'sqlite_autoindex_t_{i}' for i in range(1,4)]]],
            'CREATE INDEX ix ON t(x DESC,id COLLATE NOCASE)','PRAGMA index_xinfo(ix)',
            "INSERT INTO t VALUES(3,'c',3),(1,'a',1),(2,'b',2)",ROWS,
            'SELECT rowid FROM t','INSERT INTO t(rowid) VALUES(10)',
            'UPDATE t SET y=y+10 RETURNING *',ROWS,'DELETE FROM t WHERE y>12 RETURNING *',ROWS])
        cases+=1
    for schema, policy, begin, dml in itertools.product(SCHEMAS, ['',*[' OR '+p for p in POLICIES]], [[],['BEGIN'],['SAVEPOINT s']], [
        "INSERT{p} INTO t VALUES(4,'d',4),(1,'a',9),(5,'e',5)",
        "INSERT{p} INTO t VALUES(4,'d',4),(NULL,'z',9),(5,'e',5)",
        "UPDATE{p} t SET id=id+1 RETURNING *",
        "UPDATE{p} t SET id=3,x='same' RETURNING *",
        "UPDATE{p} t SET y=CASE id WHEN 1 THEN 10 ELSE -1 END RETURNING *",
    ]):
        compare(lib,[f'CREATE TABLE t({schema}) WITHOUT ROWID',
            "INSERT INTO t VALUES(3,'c',3),(1,'a',1),(2,'b',2)",*begin,dml.format(p=policy),ROWS,
            'ROLLBACK TO s',ROWS,'COMMIT',ROWS])
        cases+=1
    for statements in [
        ['CREATE TABLE t(x) WITHOUT ROWID'],
        ['CREATE TABLE t(x INTEGER PRIMARY KEY AUTOINCREMENT) WITHOUT ROWID'],
        ['CREATE TABLE t(x PRIMARY KEY) WITHOUT ROWID, WITHOUT ROWID','INSERT INTO t VALUES(1)','SELECT * FROM t'],
        ['CREATE TABLE t(x PRIMARY KEY) WITHOUT ROWID STRICT'],
        ['CREATE TABLE t(id INTEGER PRIMARY KEY,x TEXT,y INT) WITHOUT ROWID','INSERT INTO t DEFAULT VALUES',ROWS],
        ['CREATE TABLE t(id INTEGER PRIMARY KEY DEFAULT 5,x TEXT,y INT) WITHOUT ROWID','INSERT INTO t DEFAULT VALUES',ROWS],
        ['CREATE TABLE t(rowid PRIMARY KEY,_rowid_,oid) WITHOUT ROWID',"INSERT INTO t VALUES('abc',2,3) RETURNING *",'SELECT rowid,_rowid_,oid FROM t'],
        ['CREATE TABLE t(id INTEGER PRIMARY KEY,x TEXT,y INT) WITHOUT ROWID',"INSERT INTO t VALUES(1,'a',1)","INSERT INTO t VALUES(1,'b',2) ON CONFLICT(id) DO UPDATE SET id=3,x=excluded.x,y=excluded.y RETURNING *",ROWS],
        ['CREATE TABLE normal(x)','INSERT INTO normal(rowid,x) VALUES(42,1)','CREATE TABLE t(id INT PRIMARY KEY,x ANY,y INT) WITHOUT ROWID,STRICT',"INSERT INTO t VALUES(1,'001',1),(2,2,2) RETURNING *,last_insert_rowid(),changes()",'CREATE VIEW v AS SELECT * FROM t','CREATE TABLE copied AS SELECT * FROM v','SELECT * FROM copied ORDER BY id','PRAGMA table_info(v)',"UPDATE t SET y=(SELECT sum(c.y) FROM t c WHERE c.id<=t.id) RETURNING *",ROWS],
        ['CREATE TABLE t(id TEXT,x TEXT,y INT,PRIMARY KEY(id COLLATE NOCASE,id COLLATE BINARY,x)) WITHOUT ROWID',"INSERT INTO t VALUES('A','b',1),('a','b',2)",ROWS,'PRAGMA table_info(t)','PRAGMA index_xinfo(t)',"UPDATE t SET y=y+1 RETURNING *",ROWS],
    ]:
        compare(lib,statements);cases+=1
    for policy,begin,dml in itertools.product(['',*[' OR '+p for p in POLICIES]],[[],['BEGIN'],['SAVEPOINT s']],[
        "INSERT{p} INTO t VALUES(4,'d',4),(5,'e','bad')",
        "UPDATE{p} t SET y=CASE id WHEN 1 THEN 10 ELSE 'bad' END",
        "INSERT{p} INTO t VALUES(4,'d',4),(1,'a',2) ON CONFLICT(id) DO UPDATE SET y='bad'",
        "INSERT{p} INTO t VALUES(4,'d',4),(1,'a',2) ON CONFLICT(id) DO UPDATE SET id=NULL",
        "INSERT{p} INTO t VALUES(4,'d',4),(1,'a',2) ON CONFLICT(id) DO UPDATE SET x=excluded.x||'new' RETURNING *",
    ]):
        compare(lib,['CREATE TABLE t(id INT PRIMARY KEY,x TEXT UNIQUE,y INT) WITHOUT ROWID,STRICT',
            "INSERT INTO t VALUES(3,'c',3),(1,'a',1),(2,'b',2)",*begin,dml.format(p=policy),ROWS,'ROLLBACK TO s',ROWS,'COMMIT',ROWS])
        cases+=1
    with tempfile.TemporaryDirectory(prefix='safe-without-rowid-') as folder:
        folder=Path(folder)
        for n,(schema,encoding,mode) in enumerate(itertools.product(SCHEMAS,['UTF-8','UTF-16le','UTF-16be'],[0,1,2])):
            original=folder/f'{n}-in.db';saved=folder/f'{n}-out.db'
            native(original,f"PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t({schema}) WITHOUT ROWID;CREATE INDEX ix ON t(x DESC,id COLLATE NOCASE);INSERT INTO t VALUES(3,'🦀',3),(1,'é',1),(2,'b',2);")
            sql="UPDATE t SET y=y+10;INSERT INTO t VALUES(4,'new',4);"
            run_engine(sql,'--load',original,'--save',saved)
            native(original,sql)
            for query in [ROWS,'PRAGMA index_list(t)','PRAGMA index_xinfo(t)','PRAGMA index_xinfo(ix)',"SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name"]:
                assert native(saved,query)==native(original,query),(schema,encoding,mode,query)
            assert native(saved,'PRAGMA integrity_check')==[{'integrity_check':'ok'}],(schema,encoding,mode,native(saved,'PRAGMA integrity_check'))
            assert native(saved,'SELECT * FROM t INDEXED BY ix ORDER BY id,x,y')==native(saved,ROWS)
            cases+=1
        for n,(page_size,encoding) in enumerate(itertools.product([512,1024,2048,4096,8192,16384,32768,65536],['UTF-8','UTF-16le','UTF-16be'])):
            empty=folder/f'packed-{n}-empty.db';saved=folder/f'packed-{n}.db';final=folder/f'packed-{n}-final.db'
            native(empty,f"PRAGMA page_size={page_size};PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={n%3};VACUUM;")
            schema='id INT,x TEXT COLLATE NOCASE,y BLOB,PRIMARY KEY(x DESC,id),UNIQUE(id)'
            values=','.join(f"({i},'é-{i:04d}-{'a'*700}',x'{('00ff'*300)}')" for i in range(80,-1,-1))
            run_engine(f'CREATE TABLE t({schema}) WITHOUT ROWID,STRICT;CREATE INDEX ix ON t(id DESC,x COLLATE BINARY);INSERT INTO t VALUES{values};','--load',empty,'--save',saved)
            assert native(saved,'PRAGMA integrity_check')==[{'integrity_check':'ok'}]
            assert native(saved,'SELECT count(*) AS n FROM t INDEXED BY ix')==[{'n':81}]
            native(saved,"UPDATE t SET id=id+1000 WHERE id<10;DELETE FROM t WHERE id BETWEEN 20 AND 40;INSERT INTO t VALUES(-100,'native',x'AA');")
            expected=native(saved,ROWS)
            run_engine('CREATE VIEW v AS SELECT * FROM t;','--load',saved,'--save',final)
            assert native(final,ROWS)==expected
            assert native(final,'PRAGMA integrity_check')==[{'integrity_check':'ok'}]
            assert native(final,'SELECT * FROM t INDEXED BY ix ORDER BY id,x,y')==expected
            cases+=1
    print(f'PASS: {cases} WITHOUT ROWID SQL/primary-key/counter/metadata/image scenarios against SQLite 3.53.4')


if __name__=='__main__':main()
