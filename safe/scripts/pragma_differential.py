#!/usr/bin/env python3
"""Compare schema-inspection pragmas with the pinned native SQLite reference."""
from pathlib import Path
import subprocess
import tempfile
from index_differential import native
from sql_differential import ROOT, run_engine
from constraint_differential import outcome


def compare(path, pragma):
    expected = native(path, pragma)
    actual = run_engine(pragma, '--load', path)[-1]
    decoded = []
    for row in actual['rows']:
        decoded.append([None if v['type']=='null' else
                        bytes.fromhex(v['hex']).decode('utf-8') if v['type']=='text' else
                        v['value'] for v in row])
    assert decoded == [list(row.values()) for row in expected], (pragma, actual, expected)
    if expected:
        assert actual['columns'] == list(expected[0]), (pragma,actual,expected)


def main():
    subprocess.run(['cargo','build','--bin','sqlite-safe-sql'],cwd=ROOT,check=True)
    cases=0
    schemas=[
        "a integer PRIMARY KEY,b text NOT NULL DEFAULT ( 1 + 2 ),c blob DEFAULT X'00fF',d numeric ( 10 , +2 ) DEFAULT ((-1)),e double  precision,f any,g DEFAULT NULL",
        "a TEXT collate nocase,b integer,PRIMARY KEY(a DESC,b),UNIQUE(a ASC,b DESC),UNIQUE(a COLLATE rtrim,b),CHECK(b>0)",
        "a INTEGER PRIMARY KEY DESC UNIQUE,b TEXT UNIQUE,c int",
        "a INTEGER UNIQUE,b,PRIMARY KEY(a DESC)",
        "a 'INTEGER' extra,b 'abc' xyz,c [text],d DEFAULT + 123,e DEFAULT 'A''B',f DEFAULT (/* keep */ 1+2 /* tail */)",
        "a,b,PRIMARY KEY(a,a),UNIQUE(b),UNIQUE(a)",
    ]
    with tempfile.TemporaryDirectory(prefix='safe-pragma-') as directory:
        directory=Path(directory)
        for encoding in ['UTF-8','UTF-16le','UTF-16be']:
            for n,schema in enumerate(schemas):
                path=directory/f'{encoding}-{n}.db'
                native(path,f"PRAGMA encoding='{encoding}';CREATE TABLE t({schema});CREATE INDEX idx ON t(b COLLATE rtrim DESC,a);CREATE UNIQUE INDEX ix2 ON t(a COLLATE BiNaRy);")
                names=native(path,"SELECT name FROM sqlite_schema WHERE type='index' ORDER BY name;")
                pragmas=['PRAGMA table_info(t);',"PRAGMA main.table_xinfo='T';",'PRAGMA index_list(t);',
                         'PRAGMA table_info(missing);','PRAGMA table_xinfo;','PRAGMA index_list(missing);',
                         'PRAGMA index_info(missing);','PRAGMA index_xinfo(missing);',
                         'PRAGMA table_info(sqlite_schema);','PRAGMA table_xinfo(sqlite_master);']
                pragmas += [f"PRAGMA {pragma}('{item['name']}');" for item in names for pragma in ['index_info','index_xinfo']]
                for pragma in pragmas:
                    compare(path,pragma)
                    cases+=1
                # SQL-created metadata must match import-created metadata too.
                saved=directory/f'{encoding}-{n}-saved.db'
                run_engine('DROP INDEX idx;CREATE INDEX idx ON t(a,b DESC);','--load',path,'--save',saved)
                for pragma in ['PRAGMA table_info(t);','PRAGMA index_list(t);','PRAGMA index_xinfo(idx);']:
                    compare(saved,pragma)
                    cases+=1
        for name in ['a b','123','-123','1_2','a"b','🦀']:
            quoted='"'+name.replace('"','""')+'"'
            path=directory/f'name-{cases}.db'
            native(path,f'CREATE TABLE {quoted}(x integer UNIQUE);')
            for pragma in [f'PRAGMA table_info({quoted});', f'PRAGMA table_xinfo={quoted};',f'PRAGMA index_list({quoted});']:
                compare(path,pragma)
                cases+=1
            if name in ['123','-123']:
                argument = '+'+name if name=='123' else name
                compare(path,f'PRAGMA table_info({argument});')
                cases+=1
        outcome('PRAGMA table_info(1_2);')
        cases+=1
    print(f'PASS: {cases} schema-inspection pragma scenarios against SQLite 3.53.4')


if __name__=='__main__':
    main()
