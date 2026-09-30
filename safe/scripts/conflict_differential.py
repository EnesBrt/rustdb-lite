#!/usr/bin/env python3
"""Compare statement errors, retained rows, transactions and counters to SQLite.

Only this Python test reference loads native SQLite. The Rust driver executes
the safe core; it never links to this library or uses an engine fallback.
"""
import ctypes as c
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
from sql_differential import ROOT, run_engine
from index_differential import native

POLICIES = ['ROLLBACK', 'ABORT', 'FAIL', 'IGNORE', 'REPLACE']

def reference_library():
    sys.path.insert(0, str(ROOT / 'legacy/scripts'))
    from config import DEFINES
    path = ROOT / ('legacy/build/libsqlite3_safe_reference.' + ('dylib' if sys.platform == 'darwin' else 'so'))
    source = ROOT / 'legacy/upstream/sqlite3.c'
    if not path.exists() or path.stat().st_mtime < source.stat().st_mtime:
        subprocess.run([os.environ.get('CC', 'cc'), '-O2', '-dynamiclib' if sys.platform == 'darwin' else '-shared', '-fPIC',
                        *['-D' + d for d in DEFINES], str(source), '-lm', '-lpthread', '-o', str(path)], check=True)
    lib = c.CDLL(str(path))
    signatures = {
        'open': ([c.c_char_p,c.POINTER(c.c_void_p)],c.c_int),
        'close': ([c.c_void_p],c.c_int),
        'prepare_v2': ([c.c_void_p,c.c_char_p,c.c_int,c.POINTER(c.c_void_p),c.POINTER(c.c_char_p)],c.c_int),
        'step': ([c.c_void_p],c.c_int), 'finalize': ([c.c_void_p],c.c_int),
        'column_count': ([c.c_void_p],c.c_int),
        'column_name': ([c.c_void_p,c.c_int],c.c_char_p),
        'column_type': ([c.c_void_p,c.c_int],c.c_int),
        'column_int64': ([c.c_void_p,c.c_int],c.c_int64),
        'column_double': ([c.c_void_p,c.c_int],c.c_double),
        'column_text': ([c.c_void_p,c.c_int],c.c_void_p),
        'column_blob': ([c.c_void_p,c.c_int],c.c_void_p),
        'column_bytes': ([c.c_void_p,c.c_int],c.c_int),
        'get_autocommit': ([c.c_void_p],c.c_int),
        'changes64': ([c.c_void_p],c.c_int64), 'total_changes64': ([c.c_void_p],c.c_int64),
        'last_insert_rowid': ([c.c_void_p],c.c_int64), 'libversion': ([],c.c_char_p),
    }
    for name,(args,result) in signatures.items():
        f=getattr(lib,'sqlite3_'+name);f.argtypes=args;f.restype=result
    assert lib.sqlite3_libversion() == b'3.53.4'
    return lib

def oracle(lib, statements):
    db=c.c_void_p()
    assert lib.sqlite3_open(b':memory:',c.byref(db)) == 0
    events=[]
    try:
        for sql in statements:
            stmt=c.c_void_p();tail=c.c_char_p()
            code=lib.sqlite3_prepare_v2(db,sql.encode(),-1,c.byref(stmt),c.byref(tail))
            rows=[];columns=[]
            if code == 0:
                first_step = True
                while True:
                    code=lib.sqlite3_step(stmt)
                    # sqlite3_step may reprepare after a schema change. Observe
                    # execution metadata, as the Rust QueryResult API does; a
                    # stale pre-step width would read nonexistent columns as NULL.
                    if first_step:
                        columns=[lib.sqlite3_column_name(stmt,i).decode() for i in range(lib.sqlite3_column_count(stmt))]
                        first_step = False
                    if code != 100: break
                    row=[]
                    for i in range(len(columns)):
                        typ=lib.sqlite3_column_type(stmt,i)
                        if typ == 1: value={'type':'integer','value':lib.sqlite3_column_int64(stmt,i)}
                        elif typ == 2: value={'type':'real','bits':struct.pack('>d',lib.sqlite3_column_double(stmt,i)).hex().upper()}
                        elif typ in [3,4]:
                            p=(lib.sqlite3_column_text if typ==3 else lib.sqlite3_column_blob)(stmt,i)
                            n=lib.sqlite3_column_bytes(stmt,i)
                            value={'type':'text' if typ==3 else 'blob','hex':c.string_at(p,n).hex().upper()}
                        else: value={'type':'null'}
                        row.append(value)
                    rows.append(row)
            lib.sqlite3_finalize(stmt)
            ok=code==101
            events.append({'ok':ok,'autocommit':bool(lib.sqlite3_get_autocommit(db)),
                           'changes':lib.sqlite3_changes64(db),'total':lib.sqlite3_total_changes64(db),
                           'last_rowid':lib.sqlite3_last_insert_rowid(db),'columns':columns if ok else [],'rows':rows if ok else []})
    finally: lib.sqlite3_close(db)
    return events

def compare(lib, statements):
    result=subprocess.run([str(ROOT/'target/debug/examples/statement_probe')],input='\n'.join(statements)+'\n',capture_output=True,text=True,timeout=30)
    assert result.returncode==0,(statements,result.returncode,result.stderr)
    actual=[json.loads(line) for line in result.stdout.splitlines()]
    expected=oracle(lib,statements)
    assert len(actual)==len(expected)
    for i,(a,b) in enumerate(zip(actual,expected)):
        assert a==b, {'statements':statements,'at':i,'rust':a,'sqlite':b}

def main():
    subprocess.run(['cargo','build','--example','statement_probe','--bin','sqlite-safe-sql'],cwd=ROOT,check=True)
    lib=reference_library();cases=0
    constraints=[
        ('id INTEGER PRIMARY KEY,x', '(1,1),(2,2)', '(3,3),(1,4),(5,5)'),
        ('id INTEGER PRIMARY KEY,x UNIQUE', '(1,1),(2,2)', '(3,3),(4,1),(5,5)'),
        ('id INTEGER PRIMARY KEY,x NOT NULL DEFAULT 9', '(1,1),(2,2)', '(3,3),(4,NULL),(5,5)'),
        ('id INTEGER PRIMARY KEY,x CHECK(x>0)', '(1,1),(2,2)', '(3,3),(4,-1),(5,5)'),
        ('id,x,UNIQUE(id,x)', '(1,1),(2,2)', '(3,3),(1,1),(5,5)'),
    ]
    for schema,seed,values in constraints:
        for policy in POLICIES:
            for begin in [[],['BEGIN','INSERT INTO t VALUES(10,10)','SAVEPOINT s'],['SAVEPOINT s','INSERT INTO t VALUES(10,10)']]:
                compare(lib,[f'CREATE TABLE t({schema})',f'INSERT INTO t VALUES{seed}',*begin,f'INSERT OR {policy} INTO t VALUES{values}',
                             'SELECT rowid AS storage_id,* FROM t ORDER BY rowid','ROLLBACK TO s','SELECT rowid AS storage_id,* FROM t ORDER BY rowid','COMMIT'])
                cases+=1
    for schema in ['id INTEGER PRIMARY KEY ON CONFLICT {policy},x',
                   'id,x UNIQUE ON CONFLICT {policy}', 'id,x NOT NULL ON CONFLICT {policy} DEFAULT 7',
                   'id,x,UNIQUE(x) ON CONFLICT {policy}', 'id,x,PRIMARY KEY(x) ON CONFLICT {policy}']:
        for policy in POLICIES:
            for override in ['',*[f' OR {p}' for p in POLICIES]]:
                values='(3,3),(1,1),(4,4)' if 'PRIMARY KEY ON' in schema else '(3,3),(4,NULL),(5,5)' if 'NOT NULL' in schema else '(3,3),(4,1),(5,5)'
                compare(lib,[f'CREATE TABLE t({schema.format(policy=policy)})','INSERT INTO t VALUES(1,1),(2,2)',
                             f'INSERT{override} INTO t VALUES{values}','SELECT rowid AS storage_id,* FROM t ORDER BY rowid'])
                cases+=1
    for policy in POLICIES:
        for schema,assignment in [
            ('id INTEGER PRIMARY KEY,x UNIQUE','x=CASE id WHEN 1 THEN 9 ELSE 3 END'),
            ('id INTEGER PRIMARY KEY,x UNIQUE','x=3'),
            ('id INTEGER PRIMARY KEY,x','id=id+1'),
            ('id INTEGER PRIMARY KEY,x NOT NULL DEFAULT 8','x=CASE id WHEN 1 THEN 9 ELSE NULL END'),
            ('id INTEGER PRIMARY KEY,x CHECK(x>0)','x=CASE id WHEN 1 THEN 9 ELSE -1 END'),
        ]:
            compare(lib,[f'CREATE TABLE t({schema})','INSERT INTO t VALUES(1,1),(2,2),(3,3)','BEGIN',f'UPDATE OR {policy} t SET {assignment}',
                         'SELECT rowid AS storage_id,* FROM t ORDER BY rowid','COMMIT','SELECT rowid AS storage_id,* FROM t ORDER BY rowid'])
            cases+=1
    for first in POLICIES:
        for second in POLICIES:
            for schema in [f'id INTEGER PRIMARY KEY ON CONFLICT {first},x UNIQUE ON CONFLICT {second}',
                           f'id UNIQUE ON CONFLICT {first},x UNIQUE ON CONFLICT {second}']:
                compare(lib,[f'CREATE TABLE t({schema})','INSERT INTO t VALUES(1,1),(2,2)','INSERT INTO t VALUES(1,2)',
                             'SELECT rowid AS storage_id,* FROM t ORDER BY rowid'])
                cases+=1
    for statements in [
        ['CREATE TABLE t(a NOT NULL ON CONFLICT REPLACE DEFAULT NULL,b NOT NULL ON CONFLICT IGNORE)','INSERT INTO t VALUES(NULL,NULL)','INSERT INTO t VALUES(NULL,1)','SELECT * FROM t'],
        ['CREATE TABLE t(a UNIQUE ON CONFLICT IGNORE,UNIQUE(a))','INSERT INTO t VALUES(1),(1)','SELECT * FROM t'],
        ['CREATE TABLE t(a UNIQUE,UNIQUE(a) ON CONFLICT IGNORE)','INSERT INTO t VALUES(1),(1)','SELECT * FROM t'],
        ['CREATE TABLE t(a UNIQUE ON CONFLICT FAIL,UNIQUE(a) ON CONFLICT IGNORE)','CREATE TABLE t(a)','INSERT INTO t VALUES(1)'],
        ['CREATE TABLE t(a CHECK(a>0) ON CONFLICT IGNORE)','INSERT INTO t VALUES(-1)','INSERT OR IGNORE INTO t VALUES(-1)','SELECT * FROM t'],
        ['CREATE TABLE t(a, CHECK(a>0) ON CONFLICT IGNORE)','INSERT INTO t VALUES(-1)','INSERT OR IGNORE INTO t VALUES(-1)','SELECT * FROM t'],
        ['CREATE TABLE t(id INTEGER PRIMARY KEY,x UNIQUE)','INSERT INTO t VALUES(1,1),(2,2)','REPLACE INTO t VALUES(3,1)','WITH q(x) AS(VALUES(5)) REPLACE INTO t SELECT x,x FROM q','SELECT * FROM t ORDER BY id'],
        ['CREATE TABLE t(x UNIQUE)','INSERT OR FAIL INTO t VALUES(1),(abs(-9223372036854775808))','SELECT * FROM t'],
        ['CREATE TABLE t(x UNIQUE)','INSERT OR IGNORE INTO t VALUES(1),(abs(-9223372036854775808))','SELECT * FROM t'],
        ['CREATE TABLE t(id INTEGER PRIMARY KEY,x UNIQUE)','INSERT INTO t VALUES(1,1),(2,2),(3,3)','UPDATE OR IGNORE t SET id=NULL','SELECT * FROM t ORDER BY id'],
    ]:
        compare(lib,statements);cases+=1
    with tempfile.TemporaryDirectory(prefix='safe-conflicts-') as directory:
        directory=Path(directory)
        for encoding in ['UTF-8','UTF-16le','UTF-16be']:
            for policy in ['IGNORE','REPLACE']:
                original=directory/f'{encoding}-{policy}.db';saved=directory/f'{encoding}-{policy}-out.db'
                native(original,f"PRAGMA encoding='{encoding}';CREATE TABLE t(a TEXT UNIQUE ON CONFLICT {policy},b NOT NULL ON CONFLICT REPLACE DEFAULT 9);INSERT INTO t VALUES('a',1);")
                run_engine("INSERT INTO t VALUES('a',NULL),('b',NULL);",'--load',original,'--save',saved)
                native(original,"INSERT INTO t VALUES('a',NULL),('b',NULL);")
                assert native(saved,'SELECT rowid AS storage_id,* FROM t ORDER BY rowid;')==native(original,'SELECT rowid AS storage_id,* FROM t ORDER BY rowid;')
                assert native(saved,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
                run_engine('SELECT * FROM t;','--load',saved)
                cases+=1
    print(f'PASS: {cases} conflict-policy/error/state/counter/image scenarios against SQLite 3.53.4')

if __name__=='__main__':main()
