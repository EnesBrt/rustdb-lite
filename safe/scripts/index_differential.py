#!/usr/bin/env python3
"""Validate Rust-written index pages and native indexed-schema imports."""
import json
from pathlib import Path
import subprocess
import tempfile
from sql_differential import ROOT, ORACLE, run_engine


def native(path, sql):
    result = subprocess.run([str(ORACLE), '-json', str(path)], input=sql, text=True, capture_output=True, timeout=60)
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout) if result.stdout.strip() else []


def main():
    subprocess.run(['cargo', 'build', '--bins', '--example', 'index_fixture'], cwd=ROOT, check=True)
    cases = 0
    with tempfile.TemporaryDirectory(prefix='safe-index-') as tmp:
        for size in [512,1024,2048,4096,8192,16384,32768,65536]:
            path = Path(tmp)/f'index-{size}.db'
            subprocess.run([str(ROOT/'target/debug/examples/index_fixture'), str(path), str(size)], check=True)
            assert native(path, 'PRAGMA integrity_check;') == [{'integrity_check':'ok'}], size
            # Force native SQLite to consume every index, including promoted keys.
            for index, order in [('mixed','a DESC,b COLLATE RTRIM,c,rowid'), ('uid','b COLLATE BINARY DESC,id DESC,rowid'), ('repeat_col','a,a DESC,id,rowid'), ('cover_id','id,rowid')]:
                query = 'SELECT id,typeof(a) AS typ,hex(a) AS a,hex(b) AS b,hex(c) AS c FROM t'
                actual = native(path, f'{query} INDEXED BY {index} ORDER BY {order};')
                expected = native(path, f'{query} NOT INDEXED ORDER BY {order};')
                assert actual == expected, (size,index)
            # Native mutation exercises rebalancing of our pages, then Rust import/export.
            native(path, "UPDATE t SET b='changed',c=x'00FF' WHERE id%5=0;DELETE FROM t WHERE id%7=0;INSERT INTO t VALUES(5000,'new','tail',x'FFFF');")
            assert native(path, 'PRAGMA integrity_check;') == [{'integrity_check':'ok'}]
            reexport = Path(tmp)/f'reexport-{size}.db'
            run_engine('SELECT count(*) FROM t;', '--load', path, '--save', reexport)
            assert native(reexport, 'PRAGMA integrity_check;') == [{'integrity_check':'ok'}]
            for name in ['t']+[f'auto{n}' for n in range(8)]:
                assert native(path,f'SELECT * FROM {name} ORDER BY rowid;') == native(reexport,f'SELECT * FROM {name} ORDER BY rowid;')
            cases += 1
        for n, setup in enumerate([
            'CREATE TABLE main.t(x UNIQUE); CREATE INDEX IF NOT EXISTS main.idx ON t(x DESC);',
            'CREATE TABLE IF NOT EXISTS t(x UNIQUE); CREATE INDEX IF NOT EXISTS idx ON t(x DESC);',
            'CREATE TABLE "a b"("c d" TEXT UNIQUE); CREATE INDEX "i x" ON "a b"("c d" COLLATE NOCASE DESC);',
        ]):
            path=Path(tmp)/f'names-{n}.db'
            run_engine(setup,'--save',path)
            assert native(path,'PRAGMA integrity_check;') == [{'integrity_check':'ok'}]
            cases += 1
        # Build native indexed databases, rather than relying only on Rust origins.
        for encoding in ['UTF-8','UTF-16le','UTF-16be']:
            path=Path(tmp)/f'native-{encoding}.db'
            native(path, f"PRAGMA encoding='{encoding}';CREATE TABLE t(a TEXT UNIQUE COLLATE NOCASE,b REAL,c INTEGER PRIMARY KEY DESC);CREATE UNIQUE INDEX idx ON t(b DESC,a COLLATE RTRIM);INSERT INTO t VALUES('🦀',2.5,3),('A',NULL,1),(NULL,NULL,NULL),(NULL,7,NULL);")
            reexport=Path(tmp)/f'native-{encoding}-reexport.db'
            run_engine('UPDATE t SET b=9 WHERE c=3;', '--load',path,'--save',reexport)
            assert native(reexport,'PRAGMA integrity_check;') == [{'integrity_check':'ok'}]
            native(path,'UPDATE t SET b=9 WHERE c=3;')
            assert native(path,'SELECT * FROM t ORDER BY rowid;') == native(reexport,'SELECT * FROM t ORDER BY rowid;')
            cases += 1
    print(f'PASS: {cases} indexed database scenarios against SQLite 3.53.4')

if __name__ == '__main__': main()
