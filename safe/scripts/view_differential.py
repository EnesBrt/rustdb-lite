#!/usr/bin/env python3
"""View/CTAS execution, metadata and database-image interchange with SQLite."""
import subprocess
import tempfile
from pathlib import Path
from sql_differential import ROOT, check, run_engine
from constraint_differential import outcome
from index_differential import native
from pragma_differential import compare

SETUP="CREATE TABLE t(id INTEGER PRIMARY KEY,a VARCHAR(10) COLLATE NOCASE,b NUMERIC,c BLOB);INSERT INTO t VALUES(1,'a',1,x'00'),(2,'B',2,NULL),(3,NULL,NULL,x'FF');"
VIEWS=[
    'CREATE VIEW v AS SELECT * FROM t',
    'CREATE VIEW v(p,q) AS SELECT id,a FROM t WHERE b<2',
    'CREATE VIEW v AS SELECT a,b,count(*) n FROM t GROUP BY a ORDER BY a',
    'CREATE VIEW v AS SELECT id+1 n,coalesce(a,\'none\') s FROM t',
    'CREATE VIEW v AS SELECT CAST(a AS INT) ci,CAST(a AS VARCHAR) cv,CAST(a AS blah) cb,+b plus,b COLLATE NOCASE coll FROM t',
    'CREATE VIEW v AS SELECT id,(SELECT b FROM t q WHERE q.id=t.id) s FROM t',
    'CREATE VIEW v AS SELECT a,1 a,2 "a:1",3 "A" FROM t',
    'CREATE VIEW v(x,x) AS SELECT a,b FROM t',
    'CREATE VIEW v AS WITH q AS(SELECT * FROM t) SELECT id,a FROM q',
    'CREATE VIEW v AS WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM q WHERE x<4) SELECT x FROM q',
    'CREATE VIEW v AS SELECT a FROM t UNION SELECT \'c\'',
    'CREATE VIEW v AS VALUES(1,\'a\'),(2,NULL)',
    'CREATE VIEW v AS SELECT (SELECT 8),(SELECT 9)',
    'CREATE VIEW v("", "a b", "quo""te") AS SELECT id,a,b FROM t',
]

def main():
    subprocess.run(['cargo','build','--bin','sqlite-safe-sql'],cwd=ROOT,check=True)
    cases=0
    with tempfile.TemporaryDirectory(prefix='safe-views-') as directory:
        directory=Path(directory)
        for encoding in ['UTF-8','UTF-16le','UTF-16be']:
            for i,definition in enumerate(VIEWS):
                path=directory/f'view-{encoding}-{i}.db'
                native(path,f"PRAGMA encoding='{encoding}';"+SETUP+definition+';')
                for pragma in ['PRAGMA table_info(v);','PRAGMA table_xinfo(v);','PRAGMA index_list(v);']:
                    compare(path,pragma);cases+=1
                saved=directory/f'saved-{encoding}-{i}.db'
                run_engine('CREATE TABLE result AS SELECT * FROM v;','--load',path,'--save',saved)
                native(path,'CREATE TABLE result AS SELECT * FROM v;')
                assert native(saved,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
                assert native(saved,'SELECT * FROM result ORDER BY rowid;')==native(path,'SELECT * FROM result ORDER BY rowid;')
                assert native(saved,'PRAGMA table_info(result);')==native(path,'PRAGMA table_info(result);')
                assert native(saved,"SELECT sql FROM sqlite_schema WHERE name IN('v','result') ORDER BY name;")==native(path,"SELECT sql FROM sqlite_schema WHERE name IN('v','result') ORDER BY name;")
                run_engine('SELECT * FROM v;SELECT * FROM result;','--load',saved)
                cases+=1
        for page_size in [512,1024,2048,4096,8192,16384,32768,65536]:
            for mode in [0,1,2]:
                path=directory/f'pages-{page_size}-{mode}.db'
                native(path,f'PRAGMA page_size={page_size};PRAGMA auto_vacuum={mode};'+SETUP)
                saved=directory/f'pages-out-{page_size}-{mode}.db'
                run_engine('CREATE VIEW v AS SELECT a,b FROM t;CREATE VIEW w AS SELECT * FROM v;CREATE TABLE z AS SELECT * FROM w;UPDATE t SET b=b+10;','--load',path,'--save',saved)
                assert native(saved,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
                assert native(saved,'SELECT b FROM w ORDER BY b;')==[{'b':None},{'b':11},{'b':12}]
                assert native(saved,'SELECT b FROM z ORDER BY b;')==[{'b':None},{'b':1},{'b':2}]
                run_engine('SELECT * FROM w;','--load',saved)
                cases+=1
        for i,ident in enumerate(['','select','within','abort','strict','1a','a b','a"b','a_b','a$b','🦀','long_name'*6]):
            quoted='"'+ident.replace('"','""')+'"'
            reference=directory/f'ident-{i}.db';saved=directory/f'ident-out-{i}.db'
            script=f'CREATE TABLE {quoted} AS SELECT 1 AS {quoted};'
            native(reference,script);run_engine(script,'--save',saved)
            assert native(saved,'SELECT sql FROM sqlite_schema;')==native(reference,'SELECT sql FROM sqlite_schema;'),(script,native(saved,'SELECT sql FROM sqlite_schema;'),native(reference,'SELECT sql FROM sqlite_schema;'))
            assert native(saved,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
            run_engine(f'SELECT * FROM {quoted};','--load',saved)
            cases+=1
        for i,script in enumerate([
            'CREATE VIEW v AS SELECT * FROM missing;',
            'CREATE VIEW v AS SELECT * FROM v;',
            'CREATE VIEW v AS SELECT * FROM w;CREATE VIEW w AS SELECT * FROM v;',
            'CREATE VIEW v(x,y) AS SELECT 1;',
        ]):
            path=directory/f'deferred-{i}.db'
            run_engine(script,'--save',path)
            assert native(path,"SELECT count(*) n FROM sqlite_schema WHERE type='view';")[0]['n']>=1
            run_engine('SELECT * FROM v;','--load',path,fail=True)
            cases+=1
        for i,definition in enumerate([
            'v(x,y) AS SELECT 1',
            'v(x) AS SELECT a,b FROM t',
            'v(x,y,z) AS SELECT a,b FROM t',
            'v(x,x) AS SELECT 1',
        ]):
            path=directory/f'width-{i}.db'
            native(path,SETUP+'CREATE VIEW '+definition+';')
            for pragma in ['PRAGMA table_info(v);','PRAGMA table_xinfo(v);']:
                compare(path,pragma);cases+=1
            run_engine('SELECT * FROM v;','--load',path,fail=True)
    queries=[
        (SETUP+'CREATE VIEW v AS SELECT id,a,b FROM t;',"SELECT id c0,a='A' c1 FROM v ORDER BY id",2),
        (SETUP+'CREATE VIEW v AS SELECT id,a FROM t;','WITH t(id,a) AS(VALUES(9,9)) SELECT id c0 FROM v ORDER BY id',1),
        (SETUP+'CREATE VIEW v AS SELECT id,a FROM t;',"WITH v(x) AS(VALUES(9)) SELECT x c0,(SELECT a FROM main.v WHERE id=1) c1 FROM v",2),
        (SETUP+'CREATE VIEW v AS SELECT (SELECT 8) a,(SELECT 9) b;','SELECT (SELECT 7) c0,a c1,b c2 FROM v',3),
        (SETUP+'CREATE VIEW v AS SELECT id,a FROM t;UPDATE t SET a=\'z\' WHERE id=1;','SELECT a c0 FROM v ORDER BY id',1),
        (SETUP+'CREATE VIEW v AS SELECT id,a FROM t;DROP TABLE t;CREATE TABLE t(id,a);INSERT INTO t VALUES(9,\'new\');','SELECT id c0,a c1 FROM v',2),
        (SETUP+'CREATE VIEW v AS SELECT * FROM t;CREATE TABLE z AS SELECT * FROM v ORDER BY id DESC;',"SELECT rowid c0,id c1,a='A' c2 FROM z ORDER BY rowid",3),
        (SETUP+'CREATE TABLE z AS SELECT * FROM t;','SELECT changes() c0,total_changes() c1,last_insert_rowid() c2',3),
        (SETUP+'CREATE VIEW v AS SELECT 1;CREATE TABLE IF NOT EXISTS v AS SELECT * FROM missing;','SELECT * FROM (SELECT 42 c0)',1),
        (SETUP+'BEGIN;CREATE VIEW v AS SELECT * FROM t;CREATE TABLE z AS SELECT * FROM v;ROLLBACK;','SELECT count(*) c0 FROM t',1),
    ]
    for setup,q,n in queries: check(setup,q,n);cases+=1
    for script in [
        'CREATE VIEW v AS SELECT ?1;',
        'CREATE VIEW v AS SELECT 1;INSERT INTO v VALUES(2);',
        'CREATE VIEW v AS SELECT 1;UPDATE v SET x=1;',
        'CREATE VIEW v AS SELECT 1;DELETE FROM v;',
        'CREATE VIEW v AS SELECT 1;CREATE INDEX i ON v(x);',
        'CREATE VIEW v AS SELECT 1;DROP TABLE IF EXISTS v;',
        'CREATE TABLE z(x);DROP VIEW IF EXISTS z;',
        'CREATE VIEW v AS SELECT t.x;SELECT (SELECT * FROM v) FROM t;',
        'CREATE VIEW v(x,y) AS SELECT 1;PRAGMA table_info(v);',
        'CREATE VIEW v(x,y) AS SELECT * FROM missing;PRAGMA table_info(v);',
        'CREATE VIEW v(x,y) AS SELECT * FROM v;PRAGMA table_xinfo(v);',
        'CREATE VIEW v(x,y) AS SELECT nonexistent(1);PRAGMA table_info(v);',
        'CREATE VIEW v AS SELECT * FROM v;SELECT * FROM v;',
        'CREATE VIEW v AS SELECT * FROM w;CREATE VIEW w AS SELECT * FROM v;SELECT * FROM v;',
        'CREATE VIEW v AS SELECT 1 a;SELECT rowid FROM v;',
        'CREATE TABLE z AS SELECT * FROM missing;',
        'CREATE TABLE z AS SELECT abs(-9223372036854775808);',
        'CREATE VIEW v AS SELECT 1;CREATE TABLE v(x);',
        'CREATE TABLE z(x);CREATE VIEW z AS SELECT 1;',
        'CREATE INDEX z ON t(a);CREATE VIEW IF NOT EXISTS z AS SELECT 1;',
    ]: outcome(SETUP+script);cases+=1
    print(f'PASS: {cases} view/CTAS/schema/image scenarios against SQLite 3.53.4')

if __name__=='__main__': main()
