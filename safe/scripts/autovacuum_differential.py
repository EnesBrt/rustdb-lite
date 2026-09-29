#!/usr/bin/env python3
"""Check Rust pointer maps using native integrity checks, relocation and vacuum.

These are image-format/interchange tests. Rust still rebuilds complete compact
images and does not implement SQLite's incremental-vacuum scheduling semantics.
"""
from pathlib import Path
import subprocess
import tempfile
from index_differential import native
from sql_differential import ROOT, run_engine


def check(path, mode):
    assert native(path, 'PRAGMA integrity_check;') == [{'integrity_check': 'ok'}], path
    assert native(path, 'PRAGMA auto_vacuum;') == [{'auto_vacuum': mode}], path


def indexes(path):
    for index, order in [('mixed', 'a DESC,b COLLATE RTRIM,c,rowid'),
                         ('uid', 'b COLLATE BINARY DESC,id DESC,rowid'),
                         ('repeat_col', 'a,a DESC,id,rowid')]:
        query = 'SELECT id,typeof(a) AS typ,hex(a) AS a,hex(b) AS b,hex(c) AS c FROM t'
        assert native(path, f'{query} INDEXED BY {index} ORDER BY {order};') == \
            native(path, f'{query} NOT INDEXED ORDER BY {order};'), (path, index)


def main():
    subprocess.run(['cargo', 'build', '--bins', '--example', 'index_fixture'], cwd=ROOT, check=True)
    cases = 0
    with tempfile.TemporaryDirectory(prefix='safe-autovacuum-') as directory:
        directory = Path(directory)
        configurations = [(size, 'UTF-8') for size in [512,1024,2048,4096,8192,16384,32768,65536]]
        configurations += [(512, encoding) for encoding in ['UTF-16le', 'UTF-16be']]
        for mode in [1, 2]:
            for size, encoding in configurations:
                path = directory / f'fixture-{mode}-{size}-{encoding}.db'
                subprocess.run([str(ROOT/'target/debug/examples/index_fixture'), str(path), str(size), str(mode), encoding], check=True)
                check(path, mode)
                indexes(path)
                # Native page relocation uses the back-links the Rust writer made.
                native(path, "DELETE FROM t WHERE id%3=0;DROP INDEX cover_id;CREATE TABLE extra(x TEXT UNIQUE);INSERT INTO extra VALUES('tail');PRAGMA incremental_vacuum(7);")
                check(path, mode)
                indexes(path)
                reexport = directory / f'reexport-{mode}-{size}-{encoding}.db'
                mutation = "UPDATE t SET c=x'010203' WHERE id%5=0;DROP TABLE auto7;CREATE INDEX extra_idx ON extra(x DESC);"
                run_engine(mutation, '--load', path, '--save', reexport)
                native(path, mutation)
                check(reexport, mode)
                indexes(reexport)
                assert native(reexport, 'PRAGMA encoding;') == [{'encoding': encoding}]
                assert native(path, 'SELECT * FROM t ORDER BY rowid;') == native(reexport, 'SELECT * FROM t ORDER BY rowid;')
                # Exercise both incremental and full native vacuum on our output.
                native(reexport, 'DELETE FROM t WHERE id%2=0;PRAGMA incremental_vacuum;')
                check(reexport, mode)
                native(reexport, 'VACUUM;')
                check(reexport, mode)
                cases += 1
            # Enough roots to span several ptrmap pages, plus overflow in schema.
            path = directory / f'roots-{mode}.db'
            native(path, f'PRAGMA page_size=512;PRAGMA auto_vacuum={mode};CREATE TABLE seed(x);')
            reexport = directory / f'roots-{mode}-out.db'
            schema = ''.join(f'CREATE TABLE t{n}(x UNIQUE);CREATE INDEX i{n} ON t{n}(x);' for n in range(120))
            # Long SQL text forces schema-cell overflow (whose owner is page 1 or
            # a schema leaf, never the root of the table described by the cell).
            schema += "CREATE TABLE long_default(x DEFAULT '" + 'z'*2400 + "');"
            run_engine(schema, '--load', path, '--save', reexport)
            check(reexport, mode)
            native(reexport, 'DROP TABLE t1;DROP INDEX i99;CREATE TABLE last(x UNIQUE);VACUUM;')
            check(reexport, mode)
            cases += 1
            # Empty auto-vacuum files have only page 1; no dangling ptrmap page.
            path = directory / f'empty-{mode}.db'
            native(path, f'PRAGMA auto_vacuum={mode};CREATE TABLE t(x);DROP TABLE t;VACUUM;')
            reexport = directory / f'empty-{mode}-out.db'
            run_engine('PRAGMA auto_vacuum;', '--load', path, '--save', reexport)
            assert len(reexport.read_bytes()) == 4096
            check(reexport, mode)
            native(reexport, 'CREATE TABLE t(x);INSERT INTO t VALUES(1);')
            check(reexport, mode)
            cases += 1
            for encoding in ['UTF-8', 'UTF-16le', 'UTF-16be']:
                path = directory / f'native-{mode}-{encoding}.db'
                native(path, f"PRAGMA page_size=512;PRAGMA auto_vacuum={mode};PRAGMA encoding='{encoding}';CREATE TABLE t(id INTEGER PRIMARY KEY,x TEXT UNIQUE);" +
                       ''.join(f"INSERT INTO t VALUES({n},'{n:04d}{'🦀'*300}');" for n in range(80)) +
                       'DELETE FROM t WHERE id%2=0;')
                reexport = directory / f'native-{mode}-{encoding}-out.db'
                run_engine("UPDATE t SET x='changed' WHERE id=1;", '--load', path, '--save', reexport)
                native(path, "UPDATE t SET x='changed' WHERE id=1;")
                check(reexport, mode)
                assert native(path, 'SELECT * FROM t ORDER BY id;') == native(reexport, 'SELECT * FROM t ORDER BY id;')
                native(reexport, 'DELETE FROM t;PRAGMA incremental_vacuum;VACUUM;')
                check(reexport, mode)
                cases += 1
    print(f'PASS: {cases} auto-vacuum pointer-map/interchange scenarios against SQLite 3.53.4')


if __name__ == '__main__':
    main()
