#!/usr/bin/env python3
"""Compare table constraints, rowid aliasing and automatic-index interchange."""
from pathlib import Path
import subprocess
import tempfile
from index_differential import native
from sql_differential import ROOT, ORACLE, ENGINE, check, run_engine


def outcome(script):
    reference = subprocess.run([str(ORACLE), '-bail', ':memory:'], input=script,
                               text=True, capture_output=True, timeout=30)
    actual = subprocess.run([str(ENGINE)], input=script, text=True, capture_output=True, timeout=30)
    assert (reference.returncode == 0) == (actual.returncode == 0), (script, reference.stderr, actual.stderr)


def index_metadata(path):
    names = native(path, "SELECT name FROM sqlite_schema WHERE type='index' ORDER BY name;")
    return [(row['name'], native(path, f"PRAGMA index_xinfo('{row['name']}');")) for row in names]


def main():
    subprocess.run(['cargo', 'build', '--bin', 'sqlite-safe-sql'], cwd=ROOT, check=True)
    cases = 0
    definitions = [f'a {typ},b,PRIMARY KEY(a{collation}{direction})'
                   for typ in ['INTEGER','INT','TEXT']
                   for collation in ['', ' COLLATE NOCASE']
                   for direction in ['', ' ASC', ' DESC']]
    definitions += [
        'a INTEGER PRIMARY KEY DESC,b',
        'a INTEGER CONSTRAINT p PRIMARY KEY,b',
        'a INTEGER CONSTRAINT u UNIQUE,b,CONSTRAINT p PRIMARY KEY(a DESC)',
        'a INTEGER,b,PRIMARY KEY(a),UNIQUE(a)',
        'a INTEGER,b,PRIMARY KEY(a,a)',
        'a INTEGER,b,PRIMARY KEY(a,b)',
        'a TEXT COLLATE NOCASE,b,PRIMARY KEY(b DESC,a)',
        'a,b,UNIQUE(a,b),UNIQUE(a DESC,b DESC)',
        'a TEXT UNIQUE COLLATE NOCASE,b,PRIMARY KEY(a DESC),UNIQUE(a),UNIQUE(a COLLATE BINARY)',
        'a TEXT,b,UNIQUE(a DESC),PRIMARY KEY(a),UNIQUE(a ASC)',
        'a TEXT,b,PRIMARY KEY(a DESC),UNIQUE(a ASC)',
        'a TEXT,b,UNIQUE(a,b),UNIQUE(b,a),UNIQUE(a COLLATE NOCASE,b)',
        'a,b,CONSTRAINT positive CHECK(b>=0),CHECK(a IS NULL OR length(a)>0)',
        'a CONSTRAINT positive CHECK(a>0),b,CONSTRAINT pair UNIQUE(a,b)',
        'a,b,CONSTRAINT first UNIQUE(a) CONSTRAINT second UNIQUE(b)',
        'a,b,CONSTRAINT unused',
        'a CONSTRAINT unused,b',
    ]
    data = "INSERT INTO t VALUES(NULL,NULL),(5,3),('7',NULL),(NULL,4);"
    query = 'SELECT rowid c0,a c1,b c2 FROM t ORDER BY rowid'
    with tempfile.TemporaryDirectory(prefix='safe-constraints-') as directory:
        directory = Path(directory)
        for number, definition in enumerate(definitions):
            setup = f'CREATE TABLE t({definition});' + data
            check(setup, query, 3)
            rust = directory/f'rust-{number}.db'
            reference = directory/f'reference-{number}.db'
            run_engine(setup, '--save', rust)
            native(reference, setup)
            assert native(rust, 'PRAGMA integrity_check;') == [{'integrity_check':'ok'}], definition
            assert index_metadata(rust) == index_metadata(reference), definition
            assert native(rust, query) == native(reference, query), definition
            mutation = "UPDATE t SET a=9 WHERE b=3;"
            imported = directory/f'imported-{number}.db'
            run_engine(mutation, '--load', reference, '--save', imported)
            native(reference, mutation)
            assert native(imported, 'PRAGMA integrity_check;') == [{'integrity_check':'ok'}], definition
            assert index_metadata(imported) == index_metadata(reference), definition
            assert native(imported, query) == native(reference, query), definition
            for index, _ in index_metadata(rust):
                forced = f'SELECT rowid,a,b FROM t INDEXED BY "{index}" ORDER BY rowid;'
                assert native(rust, forced) == native(rust, 'SELECT rowid,a,b FROM t NOT INDEXED ORDER BY rowid;'), (definition,index)
            cases += 1
        for mode in [1,2]:
            for encoding in ['UTF-8','UTF-16le','UTF-16be']:
                reference = directory/f'mode-{mode}-{encoding}.db'
                native(reference, f"PRAGMA page_size=512;PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(a TEXT COLLATE NOCASE,b INTEGER,PRIMARY KEY(a DESC,b),UNIQUE(b,a COLLATE RTRIM),CHECK(b>0));INSERT INTO t VALUES('🦀',1),('a',2),('A',3),(NULL,4),(NULL,4);")
                rust = directory/f'mode-{mode}-{encoding}-out.db'
                run_engine('UPDATE t SET b=b+10;', '--load', reference, '--save', rust)
                native(reference,'UPDATE t SET b=b+10;')
                assert native(rust,'PRAGMA integrity_check;') == [{'integrity_check':'ok'}]
                assert native(rust,'PRAGMA auto_vacuum;') == [{'auto_vacuum':mode}]
                assert index_metadata(rust) == index_metadata(reference)
                assert native(rust,query) == native(reference,query)
                cases += 1
    errors = [
        'a(3),b',
        'a PRIMARY KEY,b,PRIMARY KEY(b)', 'a,b,PRIMARY KEY(a),PRIMARY KEY(a)',
        'a,b,UNIQUE(missing)', 'a,b,PRIMARY KEY(rowid)', 'a,b,UNIQUE(a+1)',
        'a,b,CHECK(missing)', 'a,b,CHECK(?1)', 'a,b,CHECK(count(a)>0)',
        'a,b,CHECK(a),c', 'UNIQUE(a),a', 'a,b,UNIQUE()',
        'a,b,PRIMARY KEY()', 'a,b,CONSTRAINT',
    ]
    # Conflict-clause support is still pending; exercise only shared syntax.
    for definition in errors:
        outcome(f'CREATE TABLE t({definition});')
        cases += 1
    definitions = [
        'a TEXT COLLATE NOCASE,b INTEGER,PRIMARY KEY(a,b)',
        'a TEXT,b INTEGER,UNIQUE(a COLLATE NOCASE,b)',
        'a,b,CHECK(b>0 AND a>0)',
        'a INTEGER,b,PRIMARY KEY(a DESC)',
    ]
    for definition in definitions:
        setup = f'CREATE TABLE t({definition});INSERT INTO t VALUES(1,2);'
        for mutation in [
            "INSERT INTO t VALUES('1','2')", 'INSERT INTO t VALUES(NULL,NULL)',
            'INSERT INTO t VALUES(1,NULL)', 'INSERT INTO t VALUES(0,2)',
            'INSERT INTO t VALUES(1,-2)', 'UPDATE t SET a=NULL,b=NULL',
            'UPDATE t SET a=0,b=0', 'INSERT INTO t VALUES(2,3),(1,2)',
        ]:
            outcome(setup+mutation+';')
            cases += 1
    print(f'PASS: {cases} table-constraint/query/error/index-interchange scenarios against SQLite 3.53.4')


if __name__ == '__main__':
    main()
