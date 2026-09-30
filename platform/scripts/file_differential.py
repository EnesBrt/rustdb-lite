#!/usr/bin/env python3
"""Real-file interchange, POSIX lock contention, and process-interruption recovery.

This tests process exit and explicit partial writes, not physical power loss.
Native SQLite is a separate test oracle, never an engine fallback.
"""
from pathlib import Path
import json
import selectors
import shutil
import sqlite3
import subprocess
import sys
import tempfile

ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT/'safe/scripts'))
from index_differential import native
from sql_differential import ORACLE
PROBE=ROOT/'target/debug/examples/storage_probe'
SQL=ROOT/'target/debug/examples/file_sql'

def run(program,*args,expect=0):
    result=subprocess.run([str(program),*map(str,args)],capture_output=True,text=True,timeout=20)
    assert result.returncode==expect,(args,result.returncode,result.stdout,result.stderr)
    return result

def blocked_native(path,sql):
    result=subprocess.run([str(ORACLE),str(path),sql],capture_output=True,text=True,timeout=5)
    assert result.returncode!=0 and 'locked' in result.stderr,(sql,result.stdout,result.stderr)

def logical(path):
    schema=native(path,"SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name;")
    return native(path,'SELECT * FROM t ORDER BY id;') if schema else []

def copy_pair(source,destination):
    shutil.copyfile(source,destination)
    journal=Path(str(source)+'-journal')
    if journal.exists():shutil.copyfile(journal,str(destination)+'-journal')

def make(path,count,mode=0):
    if count==0:path.write_bytes(b'');return
    native(path,f'PRAGMA page_size=512;PRAGMA auto_vacuum={mode};CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT UNIQUE);'+
           ''.join(f"INSERT INTO t VALUES({i},'{i:04d}{'x'*240}');" for i in range(count)))

def main():
    subprocess.run(['cargo','build','-p','sqlite-safe-platform','--examples'],cwd=ROOT,check=True)
    cases=0
    with tempfile.TemporaryDirectory(prefix='safe-platform-') as temp:
        temp=Path(temp)
        for encoding in ['UTF-8','UTF-16le','UTF-16be']:
            path=temp/f'{encoding}.db'
            native(path,f"PRAGMA encoding='{encoding}';PRAGMA application_id=1196444487;PRAGMA user_version=42;CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT UNIQUE);INSERT INTO t VALUES(1,'🦀');")
            connection=sqlite3.connect(path,isolation_level=None,timeout=0)
            assert connection.execute('SELECT * FROM t').fetchall()==[(1,'🦀')]
            run(SQL,path,'BEGIN',"UPDATE t SET v='é🦀' WHERE id=1","INSERT INTO t VALUES(2,'Rust')",'COMMIT')
            assert connection.execute('SELECT * FROM t ORDER BY id').fetchall()==[(1,'é🦀'),(2,'Rust')]
            connection.close()
            assert native(path,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
            assert native(path,'PRAGMA application_id;')==[{'application_id':1196444487}]
            assert native(path,'PRAGMA user_version;')==[{'user_version':42}]
            cases+=1
        path=temp/'locks.db';make(path,2)
        connection=sqlite3.connect(path,isolation_level=None,timeout=0)
        for begin in ['BEGIN','BEGIN IMMEDIATE','BEGIN EXCLUSIVE']:
            connection.execute(begin);connection.execute('SELECT * FROM t').fetchall()
            assert 'Busy' in run(PROBE,'recover',path,expect=1).stderr
            connection.execute('ROLLBACK')
            run(PROBE,'recover',path)
            cases+=1
        connection.close()
        for operation in ['hold','double-hold']:
            child=subprocess.Popen([str(PROBE),operation,str(path)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
            try:
                with selectors.DefaultSelector() as poll:
                    poll.register(child.stdout,selectors.EVENT_READ)
                    assert poll.select(10),'lock holder did not become ready'
                    assert child.stdout.readline().strip()=='LOCKED'
                blocked_native(path,'SELECT * FROM t;')
                blocked_native(path,"UPDATE t SET v='changed' WHERE id=0;")
                assert 'Busy' in run(PROBE,'recover',path,expect=1).stderr
                child.stdin.write('release\n');child.stdin.flush()
                assert child.wait(timeout=10)==0,child.stderr.read()
            finally:
                if child.poll() is None:child.kill();child.wait(timeout=5)
            assert native(path,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
            cases+=1
        for mode in [1,2]:
            path=temp/f'autovacuum-{mode}.db'
            native(path,f'PRAGMA auto_vacuum={mode};CREATE TABLE t(x);INSERT INTO t VALUES(1);')
            connection=sqlite3.connect(path,isolation_level=None,timeout=0)
            assert connection.execute('SELECT * FROM t').fetchall()==[(1,)]
            run(SQL,path,'UPDATE t SET x=2','CREATE INDEX idx ON t(x)')
            assert connection.execute('SELECT * FROM t INDEXED BY idx').fetchall()==[(2,)]
            assert connection.execute('PRAGMA auto_vacuum').fetchone()==(mode,)
            connection.close()
            assert native(path,'PRAGMA auto_vacuum;')==[{'auto_vacuum':mode}]
            assert native(path,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
            cases+=1
        path=temp/'cte.db';make(path,2)
        run(SQL,path,'WITH RECURSIVE q(n) AS(VALUES(3) UNION ALL SELECT n+1 FROM q WHERE n<7) INSERT INTO t SELECT n,\'value-\'||n FROM q')
        assert native(path,'SELECT count(*) AS n FROM t;')==[{'n':7}]
        assert native(path,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
        cases+=1
        path=temp/'views.db';make(path,3)
        native(path,'CREATE VIEW v AS SELECT id,v FROM t WHERE id>0;')
        connection=sqlite3.connect(path,isolation_level=None,timeout=0)
        assert connection.execute('SELECT count(*) FROM v').fetchone()==(2,)
        run(SQL,path,'CREATE TABLE snapshot AS SELECT * FROM v','CREATE VIEW w AS SELECT * FROM snapshot','UPDATE t SET id=id+10')
        assert connection.execute('SELECT id FROM v ORDER BY id').fetchall()==[(10,),(11,),(12,)]
        assert connection.execute('SELECT id FROM w ORDER BY id').fetchall()==[(1,),(2,)]
        run(SQL,path,'DROP VIEW v','CREATE VIEW v AS SELECT * FROM snapshot')
        assert connection.execute('SELECT id FROM v ORDER BY id').fetchall()==[(1,),(2,)]
        connection.close()
        assert native(path,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
        run(SQL,path,'SELECT * FROM v','SELECT * FROM w')
        cases+=1
        crashes=0
        path=temp/'subqueries.db';make(path,3)
        run(SQL,path,"UPDATE t SET v=(SELECT 'changed-'||t.id) WHERE id IN(SELECT id FROM t WHERE id<2)")
        assert native(path,"SELECT id,v FROM t WHERE id<2 ORDER BY id;")==[{'id':0,'v':'changed-0'},{'id':1,'v':'changed-1'}]
        assert native(path,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
        cases+=1
        for transaction in [False,True]:
            for policy in ['ABORT','FAIL','ROLLBACK','IGNORE','REPLACE']:
                path=temp/f'conflict-{policy}-{transaction}.db'
                native(path,"CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT UNIQUE);INSERT INTO t VALUES(1,'one');")
                run(SQL,path,*(['BEGIN',"INSERT INTO t VALUES(10,'prior')"] if transaction else []),
                    f"INSERT OR {policy} INTO t VALUES(2,'two'),(3,'one'),(4,'four')",*(['COMMIT'] if transaction else []),
                    expect=0 if policy in ['IGNORE','REPLACE'] else 1)
                # This process exits on SQL errors, dropping any open transaction.
                ids=([1] if transaction and policy in ['ABORT','FAIL','ROLLBACK'] else
                     {'ABORT':[1],'ROLLBACK':[1],'FAIL':[1,2],'IGNORE':[1,2,4],'REPLACE':[2,3,4]}[policy])
                if transaction and policy in ['IGNORE','REPLACE']: ids.append(10)
                assert native(path,'SELECT id FROM t ORDER BY id;')==[{'id':i} for i in ids]
                assert native(path,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
                cases+=1
        for encoding in ['UTF-8','UTF-16le','UTF-16be']:
            for mode in [0,1,2]:
                path=temp/f'upsert-{encoding}-{mode}.db'
                native(path,f"PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT UNIQUE);INSERT INTO t VALUES(1,'é');")
                connection=sqlite3.connect(path,isolation_level=None,timeout=0)
                assert connection.execute('SELECT * FROM t').fetchall()==[(1,'é')]
                run(SQL,path,"INSERT INTO t VALUES(2,'é'),(3,'🦀') ON CONFLICT(v) DO UPDATE SET id=excluded.id")
                assert connection.execute('SELECT * FROM t ORDER BY id').fetchall()==[(2,'é'),(3,'🦀')]
                run(SQL,path,"INSERT OR FAIL INTO t VALUES(4,'new'),(5,'é') ON CONFLICT(v) DO UPDATE SET id=3",expect=1)
                assert connection.execute('SELECT * FROM t ORDER BY id').fetchall()==[(2,'é'),(3,'🦀')]
                run(SQL,path,'BEGIN',"INSERT INTO t VALUES(2,'changed') ON CONFLICT(id) DO UPDATE SET v=excluded.v",'COMMIT')
                assert connection.execute('SELECT * FROM t ORDER BY id').fetchall()==[(2,'changed'),(3,'🦀')]
                connection.close()
                assert native(path,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
                assert native(path,'PRAGMA auto_vacuum;')==[{'auto_vacuum':mode}]
                cases+=1
        for encoding in ['UTF-8','UTF-16le','UTF-16be']:
            for mode in [0,1,2]:
                path=temp/f'returning-{encoding}-{mode}.db'
                native(path,f"PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT UNIQUE);INSERT INTO t VALUES(1,'é');")
                connection=sqlite3.connect(path,isolation_level=None,timeout=0)
                assert connection.execute('SELECT * FROM t').fetchall()==[(1,'é')]
                run(SQL,path,"INSERT INTO t VALUES(2,'é'),(3,'🦀') ON CONFLICT(v) DO UPDATE SET id=excluded.id RETURNING *")
                assert connection.execute('SELECT * FROM t ORDER BY id').fetchall()==[(2,'é'),(3,'🦀')]
                before=path.read_bytes()
                run(SQL,path,'DELETE FROM t RETURNING abs(CASE id WHEN 3 THEN -9223372036854775808 ELSE 1 END)',expect=1)
                assert path.read_bytes()==before
                run(SQL,path,"INSERT OR FAIL INTO t VALUES(4,'four'),(5,'é') RETURNING id",expect=1)
                assert connection.execute('SELECT id FROM t ORDER BY id').fetchall()==[(2,),(3,),(4,)]
                run(SQL,path,'BEGIN',"UPDATE t SET v=v||'!' RETURNING *",'COMMIT','DELETE FROM t WHERE id=4 RETURNING *')
                assert connection.execute('SELECT * FROM t ORDER BY id').fetchall()==[(2,'é!'),(3,'🦀!')]
                connection.close()
                assert native(path,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
                assert native(path,'PRAGMA auto_vacuum;')==[{'auto_vacuum':mode}]
                cases+=1
        for encoding in ['UTF-8','UTF-16le','UTF-16be']:
            for mode in [0,1,2]:
                path=temp/f'sequence-{encoding}-{mode}.db'
                native(path,f"PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,v TEXT UNIQUE);INSERT INTO t VALUES(100,'é');DELETE FROM t;")
                connection=sqlite3.connect(path,isolation_level=None,timeout=0)
                assert connection.execute('SELECT * FROM t').fetchall()==[]
                run(SQL,path,"INSERT INTO t(v) VALUES('é')")
                assert connection.execute('SELECT id FROM t').fetchall()==[(101,)]
                run(SQL,path,"INSERT OR FAIL INTO t(v) VALUES('🦀'),('é')",expect=1)
                assert connection.execute('SELECT id FROM t ORDER BY id').fetchall()==[(101,),(102,)]
                assert connection.execute('SELECT seq FROM sqlite_sequence').fetchall()==[(101,)]
                run(SQL,path,"INSERT INTO t(v) VALUES('new') RETURNING id")
                assert connection.execute('SELECT seq FROM sqlite_sequence').fetchall()==[(103,)]
                before=path.read_bytes()
                run(SQL,path,'BEGIN',"INSERT INTO t VALUES(9223372036854775807,'max')","INSERT INTO t(v) VALUES('full')",expect=1)
                assert path.read_bytes()==before
                assert connection.execute('SELECT seq FROM sqlite_sequence').fetchall()==[(103,)]
                run(SQL,path,'DROP TABLE t')
                assert connection.execute('SELECT * FROM sqlite_sequence').fetchall()==[]
                connection.close()
                assert native(path,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
                cases+=1
        for encoding in ['UTF-8','UTF-16le','UTF-16be']:
            for mode in [0,1,2]:
                path=temp/f'strict-{encoding}-{mode}.db'
                native(path,f"PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(x INT,a ANY,r REAL) STRICT;")
                connection=sqlite3.connect(path,isolation_level=None,timeout=0)
                assert connection.execute('SELECT * FROM t').fetchall()==[]
                run(SQL,path,"INSERT INTO t VALUES(1,'001',1),(2,2,2.5)")
                assert connection.execute('SELECT x,a,r FROM t ORDER BY x').fetchall()==[(1,'001',1.0),(2,2,2.5)]
                before=path.read_bytes()
                run(SQL,path,"INSERT INTO t VALUES(3,'03',3),('bad',4,4)",expect=1)
                assert path.read_bytes()==before
                run(SQL,path,'--continue','BEGIN',"INSERT INTO t VALUES(3,'03',3),('bad',4,4)",'COMMIT',expect=1)
                assert connection.execute('SELECT x,a,r FROM t ORDER BY x').fetchall()==[(1,'001',1.0),(2,2,2.5),(3,'03',3.0)]
                # Preserve a valid index after a type error on a colliding rowid.
                # The pinned native engine can lose index entries in this case;
                # that physical-error divergence is documented in STRICT.md.
                run(SQL,path,'CREATE INDEX strict_x ON t(x)')
                run(SQL,path,'--continue','BEGIN',"INSERT OR REPLACE INTO t(rowid,x) VALUES(1,'bad')",'COMMIT',expect=1)
                assert connection.execute('SELECT a FROM t INDEXED BY strict_x WHERE x=1').fetchall()==[('001',)]
                connection.close()
                assert native(path,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
                cases+=1
        for encoding in ['UTF-8','UTF-16le','UTF-16be']:
            for mode in [0,1,2]:
                path=temp/f'without-rowid-{encoding}-{mode}.db'
                native(path,f"PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(id INT,v TEXT,n INT,PRIMARY KEY(v DESC,id),UNIQUE(id)) WITHOUT ROWID,STRICT;CREATE INDEX ix ON t(n DESC);INSERT INTO t VALUES(1,'é',1),(2,'🦀',2);")
                connection=sqlite3.connect(path,isolation_level=None,timeout=0)
                assert connection.execute('SELECT count(*) FROM t').fetchone()==(2,)
                run(SQL,path,'BEGIN','UPDATE t SET id=id+10','COMMIT',"INSERT INTO t VALUES(11,'é',3) ON CONFLICT(id) DO UPDATE SET v='updated',n=excluded.n RETURNING *")
                assert connection.execute('SELECT * FROM t ORDER BY id').fetchall()==[(11,'updated',3),(12,'🦀',2)]
                run(SQL,path,"INSERT OR FAIL INTO t VALUES(13,'new',3),(11,'updated',4)",expect=1)
                expected=[(11,'updated',3),(12,'🦀',2),(13,'new',3)]
                assert connection.execute('SELECT * FROM t INDEXED BY ix ORDER BY id').fetchall()==expected
                before=path.read_bytes()
                run(SQL,path,'BEGIN','UPDATE t SET n=n+10','ROLLBACK')
                run(SQL,path,"INSERT OR IGNORE INTO t VALUES(14,'bad','bad')",expect=1)
                assert path.read_bytes()==before
                run(SQL,path,'CREATE TABLE copied AS SELECT * FROM t','CREATE TABLE u(k TEXT PRIMARY KEY) WITHOUT ROWID',"INSERT INTO u VALUES('Rust-created')")
                assert connection.execute('SELECT * FROM copied ORDER BY id').fetchall()==expected
                connection.close()
                assert native(path,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
                cases+=1
        for encoding in ['UTF-8','UTF-16le','UTF-16be']:
            for mode in [0,1,2]:
                path=temp/f'generated-{encoding}-{mode}.db'
                native(path,f"PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(id INTEGER PRIMARY KEY,x TEXT,g TEXT AS(upper(x)) UNIQUE,s INT AS(length(g)) STORED);INSERT INTO t(x) VALUES('é'),('rust');")
                connection=sqlite3.connect(path,isolation_level=None,timeout=0)
                run(SQL,path,'BEGIN',"UPDATE t SET x=x||id",'COMMIT',"INSERT INTO t(x) VALUES('RUST2') ON CONFLICT(g) DO UPDATE SET x=excluded.g||'new' RETURNING *")
                expected=[(1,'é1','é1',2),(2,'RUST2new','RUST2NEW',8)]
                assert connection.execute('SELECT * FROM t ORDER BY id').fetchall()==expected
                before=path.read_bytes()
                run(SQL,path,"INSERT INTO t(x) VALUES('fresh'),('é1')",expect=1)
                assert path.read_bytes()==before
                run(SQL,path,'BEGIN',"UPDATE t SET x='rollback' WHERE id=1",'ROLLBACK')
                assert path.read_bytes()==before
                run(SQL,path,"INSERT OR FAIL INTO t(x) VALUES('fresh'),('é1')",expect=1)
                expected.append((3,'fresh','FRESH',5))
                assert connection.execute('SELECT * FROM t INDEXED BY sqlite_autoindex_t_1 ORDER BY id').fetchall()==expected
                run(SQL,path,'CREATE TABLE copied AS SELECT * FROM t')
                assert connection.execute('SELECT * FROM copied ORDER BY id').fetchall()==expected
                connection.close()
                assert native(path,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
                cases+=1
        for encoding in ['UTF-8','UTF-16le','UTF-16be']:
            for mode in [0,1,2]:
                path=temp/f'expression-index-{encoding}-{mode}.db'
                native(path,f"PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(id INTEGER PRIMARY KEY,x TEXT,active INT);INSERT INTO t VALUES(1,'Rust',1),(2,'RUST',0);")
                connection=sqlite3.connect(path,isolation_level=None,timeout=0)
                run(SQL,path,'CREATE UNIQUE INDEX ix ON t(lower(x)) WHERE active=1','CREATE INDEX iy ON t(length(x) DESC) WHERE active IS NOT NULL')
                before=path.read_bytes()
                run(SQL,path,'UPDATE t SET active=1 WHERE id=2',expect=1)
                assert path.read_bytes()==before
                run(SQL,path,"INSERT INTO t VALUES(3,'RUST',1) ON CONFLICT(lower(x)) WHERE active=1 DO UPDATE SET x='changed'")
                assert connection.execute('SELECT id FROM t INDEXED BY ix WHERE active=1 ORDER BY id').fetchall()==[(1,)]
                before=path.read_bytes()
                run(SQL,path,'BEGIN','UPDATE t SET active=0','ROLLBACK')
                assert path.read_bytes()==before
                run(SQL,path,"INSERT OR FAIL INTO t VALUES(3,'fresh',1),(4,'CHANGED',1)",expect=1)
                expected=[(1,'changed',1),(2,'RUST',0),(3,'fresh',1)]
                assert connection.execute('SELECT * FROM t ORDER BY id').fetchall()==expected
                assert connection.execute('SELECT * FROM t INDEXED BY iy WHERE active IS NOT NULL ORDER BY id').fetchall()==expected
                run(SQL,path,'UPDATE t SET active=1 WHERE id=2','CREATE TABLE copied AS SELECT * FROM t')
                assert connection.execute('SELECT id FROM t INDEXED BY ix WHERE active=1 ORDER BY id').fetchall()==[(1,),(2,),(3,)]
                connection.close()
                assert native(path,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
                cases+=1
        scenarios=[(old,new,mode) for mode in [0,1,2] for old,new in [(2,8),(8,1),(0,3)]]
        for scenario,(old_count,new_count,mode) in enumerate(scenarios):
            old=temp/f'original-{scenario}.db';new=temp/f'next-{scenario}.db'
            make(old,old_count,mode);make(new,new_count,mode)
            old_rows=logical(old);new_rows=logical(new)
            pages=len(new.read_bytes())//512
            stops=[(stop,False) for stop in range(1,pages+10)]
            stops += [(stop,True) for stop in sorted({6,6+pages//2,5+pages})]
            for stop,partial in stops:
                name=f'{scenario}-{stop}-{partial}'
                path=temp/f'crash-{name}.db';shutil.copyfile(old,path)
                run(PROBE,'commit',path,new,stop,*(['partial'] if partial else []),expect=99)
                rust=temp/f'rust-recovery-{name}.db'
                reference=temp/f'native-recovery-{name}.db'
                copy_pair(path,rust);copy_pair(path,reference)
                run(PROBE,'recover',rust)
                assert native(rust,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
                assert native(reference,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
                expected=old_rows if stop<pages+8 else new_rows
                assert logical(rust)==expected,(scenario,stop,'Rust')
                assert logical(reference)==expected,(scenario,stop,'native')
                assert rust.read_bytes()==reference.read_bytes(),(scenario,stop,'byte mismatch')
                crashes+=1
    print(f'PASS: {cases} real-file interchange/lock scenarios and {crashes} process-interruption/partial-write points; SQLite 3.53.4 oracle, Python SQLite {sqlite3.sqlite_version} persistent-connection peer')

if __name__=='__main__':main()
