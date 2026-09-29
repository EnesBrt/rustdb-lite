#!/usr/bin/env python3
"""Test Rust undo journals with native recovery, and native spill journals with Rust."""
from pathlib import Path
import shutil
import sqlite3
import subprocess
import tempfile
from index_differential import native
from sql_differential import ROOT

CODEC=ROOT/'target/debug/examples/journal_snapshot'

def main():
    subprocess.run(['cargo','build','--example','journal_snapshot'],cwd=ROOT,check=True)
    cases=0
    with tempfile.TemporaryDirectory(prefix='safe-journal-') as tmp:
        tmp=Path(tmp)
        for size in [512,1024,2048,4096,8192,16384,32768,65536]:
            path=tmp/f'rust-{size}.db'
            native(path,f"PRAGMA page_size={size};CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT UNIQUE);INSERT INTO t VALUES(1,'old'),(2,'🦀');")
            original=path.read_bytes()
            undo=tmp/f'undo-{size}'
            subprocess.run([str(CODEC),'encode',str(path),str(undo)],check=True)
            native(path,"UPDATE t SET v='new'||id;")
            shutil.copyfile(undo,str(path)+'-journal')
            assert native(path,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
            assert native(path,'SELECT * FROM t ORDER BY id;')==[{'id':1,'v':'old'},{'id':2,'v':'🦀'}]
            assert path.read_bytes()==original
            assert not Path(str(path)+'-journal').exists()
            cases+=1
        for size in [512,4096,65536]:
            path=tmp/f'native-{size}.db'
            con=sqlite3.connect(path)
            con.execute(f'PRAGMA page_size={size}')
            con.execute('PRAGMA journal_mode=DELETE')
            con.execute('PRAGMA synchronous=FULL')
            con.execute('PRAGMA cache_size=3')
            con.execute('CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT)')
            con.executemany('INSERT INTO t VALUES(?,?)',[(i,f'{i:04d}'+ 'x'*2000) for i in range(1000)])
            con.commit()
            original=path.read_bytes()
            con.execute('BEGIN IMMEDIATE')
            con.execute("UPDATE t SET v='changed'||substr(v,8)")
            main=tmp/f'spill-{size}.db'; journal=tmp/f'spill-{size}.journal'
            shutil.copyfile(path,main)
            shutil.copyfile(str(path)+'-journal',journal)
            assert journal.read_bytes()[:8]==bytes.fromhex('d9d505f920a163d7'), 'spill did not sync journal'
            assert main.read_bytes()!=original, 'spill did not reach main file'
            con.rollback(); con.close()
            recovered=tmp/f'recovered-{size}.db'
            subprocess.run([str(CODEC),'recover',str(main),str(journal),str(recovered)],check=True)
            assert recovered.read_bytes()==original,(size,'Rust did not recover native pretransaction bytes')
            assert native(recovered,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
            # Independently have the pinned reference recover the same snapshots.
            shutil.copyfile(journal,str(main)+'-journal')
            assert native(main,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
            assert main.read_bytes()==original
            cases+=1
    print(f'PASS: {cases} rollback-journal scenarios; SQLite 3.53.4 oracle, Python SQLite {sqlite3.sqlite_version} spill producer')

if __name__=='__main__': main()
