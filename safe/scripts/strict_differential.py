#!/usr/bin/env python3
"""Compare STRICT declarations, storage classes and statement rollback with SQLite."""
import itertools
import json
from pathlib import Path
import subprocess
import tempfile

from conflict_differential import reference_library, compare, oracle, POLICIES
from sql_differential import ROOT, run_engine
from index_differential import native

ROWS = 'SELECT rowid AS rid,* FROM t ORDER BY rowid'


def main():
    subprocess.run(['cargo', 'build', '--example', 'statement_probe', '--bin', 'sqlite-safe-sql'], cwd=ROOT, check=True)
    lib = reference_library()
    cases = 0
    for typ, suffix in itertools.product([
        '', 'INT', 'INTEGER', 'REAL', 'TEXT', 'BLOB', 'ANY', 'any', '"int"', "'text'",
        'VARCHAR(10)', 'NUMERIC', 'FLOAT', 'BOOLEAN', 'INTEGER(3)', 'DOUBLE PRECISION',
        '"INT"(3)', "'INT'(3)", '[INT](3)', '`INT`(3)', '"INT" foo', '"INT" "foo"', '"REAL"(2,1)',
    ], ['STRICT', 'STRICT,STRICT', 'STRICT STRICT']):
        compare(lib, [f'CREATE TABLE t(x {typ}) {suffix}', 'PRAGMA table_info(t)',
                      "INSERT INTO t VALUES('000123')", ROWS])
        cases += 1
    values = ['NULL', '0', '-1', '1.0', '-0.0', '1.5', '1e999', '-1e999',
              '9223372036854775807', '-9223372036854775808', '9223372036854775808',
              "'123'", "'000123'", "'1.5'", "'1e3'", "' 12 '" , "'0x10'", "'1x'", "''",
              "'é🦀'", "x'3132'", "x''", "x'00FF'", "'12'||char(0)", "'12'||char(0)||'bad'",
              "char(0)||'12'", "'1.5'||char(0)||'tail'", "'9007199254740993.0'"]
    for typ, value in itertools.product(['INT', 'INTEGER', 'REAL', 'TEXT', 'BLOB', 'ANY'], values):
        compare(lib, [f'CREATE TABLE t(x {typ}) STRICT', f'INSERT INTO t VALUES({value})',
                      'SELECT x,typeof(x) FROM t', f'UPDATE t SET x={value}',
                      'SELECT x,typeof(x) FROM t'])
        cases += 1
    for schema in [
        'id INTEGER PRIMARY KEY,x INT,y INT',
        'id INTEGER PRIMARY KEY AUTOINCREMENT,x INT,y INT',
        'id INTEGER PRIMARY KEY ON CONFLICT IGNORE,x INT,y INT',
        'id INTEGER PRIMARY KEY ON CONFLICT REPLACE,x INT,y INT',
        'id INTEGER PRIMARY KEY,x INT UNIQUE,y INT',
        'id INTEGER PRIMARY KEY,x INT,y INT UNIQUE',
        'id INTEGER PRIMARY KEY,x INT NOT NULL DEFAULT 5,y INT',
        'id INTEGER PRIMARY KEY,x INT NOT NULL ON CONFLICT IGNORE,y INT',
        'id INTEGER PRIMARY KEY,x INT,y INT NOT NULL',
        'id INTEGER PRIMARY KEY,x INT CHECK(x>0),y INT',
        'id INTEGER PRIMARY KEY,x INT,y INT CHECK(y>0)',
        'id INT PRIMARY KEY,x INT,y INT',
        'id ANY PRIMARY KEY,x INT,y INT',
        'id INT,x INT,y INT,PRIMARY KEY(id,y)',
    ]:
        for policy, begin, dml in itertools.product(['', *[' OR '+p for p in POLICIES]], [[], ['BEGIN'], ['SAVEPOINT s']], [
            "INSERT{p} INTO t VALUES(3,3,3),(4,'bad',4)",
            "INSERT{p} INTO t VALUES(3,3,3),('bad',4,4)",
            "INSERT{p} INTO t(x,y) VALUES(3,3),('bad',4)",
            "INSERT{p} INTO t VALUES(3,3,3),(1,'bad',1)",
            "INSERT{p} INTO t VALUES(3,3,3),(4,NULL,4),(5,'bad',5)",
            "UPDATE{p} t SET x=CASE id WHEN 1 THEN 3 ELSE 'bad' END",
            "UPDATE{p} t SET id=id+10,x=CASE id WHEN 1 THEN 3 ELSE 'bad' END",
            "UPDATE{p} t SET id=CASE id WHEN 1 THEN 10 ELSE 'bad' END",
        ]):
            compare(lib, [f'CREATE TABLE t({schema}) STRICT', 'INSERT INTO t VALUES(1,1,1),(2,2,2)',
                          *begin, dml.format(p=policy), ROWS, 'ROLLBACK TO s', ROWS, 'COMMIT', ROWS])
            cases += 1
    for policy, begin, suffix in itertools.product(['', *[' OR '+p for p in POLICIES]], [[], ['BEGIN']], [
        'ON CONFLICT(x) DO NOTHING',
        'ON CONFLICT(id) DO NOTHING',
        'ON CONFLICT DO NOTHING',
        "ON CONFLICT(x) DO UPDATE SET y='bad'",
        "ON CONFLICT(x) DO UPDATE SET x='bad'",
        "ON CONFLICT(id) DO UPDATE SET y='bad'",
        "ON CONFLICT(id) DO UPDATE SET x='bad'",
        "ON CONFLICT(x) DO UPDATE SET y=excluded.y ON CONFLICT DO NOTHING",
    ]):
        for values in ["(3,3,3),(4,'bad',4)", '(3,3,3),(1,1,1)', "(3,3,3),(1,'bad',1)"]:
            compare(lib, ['CREATE TABLE t(id INTEGER PRIMARY KEY,x INT UNIQUE,y INT) STRICT',
                          'INSERT INTO t VALUES(1,1,1),(2,2,2)', *begin,
                          f'INSERT{policy} INTO t VALUES{values} {suffix} RETURNING *', ROWS,
                          'COMMIT', ROWS])
            cases += 1
    for statements in [
        ['CREATE TABLE t(x INT PRIMARY KEY)', 'INSERT INTO t VALUES(NULL)', ROWS],
        ['CREATE TABLE t(x INT PRIMARY KEY) STRICT', 'INSERT INTO t VALUES(NULL)', ROWS, 'PRAGMA table_info(t)'],
        ['CREATE TABLE t(x INTEGER PRIMARY KEY DESC) STRICT', 'INSERT INTO t VALUES(NULL)', 'PRAGMA table_info(t)'],
        ['CREATE TABLE t(x INTEGER PRIMARY KEY) STRICT', 'INSERT INTO t VALUES(NULL)', ROWS, 'PRAGMA table_info(t)'],
        ['CREATE TABLE t(x INT PRIMARY KEY NOT NULL ON CONFLICT IGNORE) STRICT', 'INSERT INTO t VALUES(NULL),(1)', ROWS],
        ['CREATE TABLE t(x INT NOT NULL ON CONFLICT REPLACE DEFAULT \'bad\',y INT) STRICT', 'BEGIN', 'INSERT INTO t VALUES(1,1),(NULL,2)', ROWS, 'COMMIT'],
        ['CREATE TABLE t(x TEXT NOT NULL ON CONFLICT IGNORE,y INT) STRICT', "INSERT INTO t VALUES(NULL,'bad')", ROWS],
        ['CREATE TABLE t(x ANY UNIQUE) STRICT', "INSERT INTO t VALUES('001'),(1),('1')", ROWS, 'SELECT typeof(x) FROM t ORDER BY rowid'],
        ['CREATE TABLE t(x ANY PRIMARY KEY,y INT) STRICT', "INSERT INTO t VALUES('001',1),(1,2)", 'UPDATE t SET y=y+1', ROWS],
        ['CREATE TABLE t(id INTEGER PRIMARY KEY,x INT) STRICT', 'CREATE INDEX ix ON t(x)', 'INSERT INTO t VALUES(1,1)', "INSERT OR IGNORE INTO t VALUES(1,'bad')", ROWS],
        ['CREATE TABLE t(x INT CHECK(1),y INT) STRICT', 'INSERT INTO t VALUES(1,1),(2,2)', 'BEGIN', "UPDATE t SET x=CASE x WHEN 1 THEN 3 ELSE 'bad' END", ROWS, 'COMMIT'],
    ]:
        compare(lib, statements)
        cases += 1
    for schema, argument in itertools.product(['', 'main.', 'temp.'], [
        '', '(t)', '(T)', "='t'", '(u)', '(v)', '(broken)', '(sqlite_sequence)',
        '(sqlite_master)', '(sqlite_schema)', '(sqlite_temp_master)', '(sqlite_temp_schema)', '(missing)',
    ]):
        statements = ['CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,x ANY) STRICT',
                      'CREATE TABLE u AS SELECT * FROM t', 'CREATE VIEW v AS SELECT x FROM t']
        # Invalid-view resolution shares parser error state in native catalog
        # enumeration. Compare that case in isolation; the mixed-catalog limit
        # is documented in STRICT.md rather than claimed as compatible.
        if argument == '(broken)':
            statements.append('CREATE VIEW broken AS SELECT * FROM missing')
        statements.append(f'PRAGMA {schema}table_list{argument}')
        result = subprocess.run([str(ROOT/'target/debug/examples/statement_probe')], input='\n'.join(statements)+'\n', text=True, capture_output=True, check=True)
        actual = [json.loads(line) for line in result.stdout.splitlines()]
        expected = oracle(lib, statements)
        # Catalog enumeration order is not guaranteed by SQLite.
        for event in [actual[-1], expected[-1]]:
            event['rows'].sort(key=lambda row: json.dumps(row, sort_keys=True))
        assert actual == expected, (statements, actual, expected)
        cases += 1
    with tempfile.TemporaryDirectory(prefix='safe-strict-') as folder:
        folder = Path(folder)
        for encoding, mode in itertools.product(['UTF-8', 'UTF-16le', 'UTF-16be'], [0, 1, 2]):
            original = folder/f'{encoding}-{mode}.db'
            saved = folder/f'{encoding}-{mode}-out.db'
            native(original, f"PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,x ANY UNIQUE,y REAL,z BLOB) STRICT;INSERT INTO t VALUES(1,'001',1,x'FF');")
            sql = "INSERT INTO t(x,y,z) VALUES(1,'1.5',x'00'),('é🦀',2.0,x'');UPDATE t SET y=y+1;"
            run_engine(sql, '--load', original, '--save', saved)
            native(original, sql)
            for query in [ROWS, 'PRAGMA table_info(t)', "SELECT typeof(x),typeof(y),typeof(z) FROM t ORDER BY id", 'SELECT * FROM sqlite_sequence']:
                assert native(saved, query) == native(original, query)
            assert native(saved, 'PRAGMA integrity_check') == [{'integrity_check': 'ok'}]
            cases += 1
        for page_size in [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536]:
            empty = folder/f'empty-{page_size}.db'
            fresh = folder/f'fresh-{page_size}.db'
            native(empty, f'PRAGMA page_size={page_size};VACUUM;')
            run_engine("CREATE TABLE t(id INT PRIMARY KEY,x ANY,y REAL) STRICT;INSERT INTO t VALUES(1,'0001',1),(2,2,2.5);", '--load', empty, '--save', fresh)
            assert native(fresh, 'PRAGMA integrity_check') == [{'integrity_check': 'ok'}]
            assert native(fresh, "SELECT strict FROM pragma_table_list WHERE name='t'") == [{'strict': 1}]
            native(fresh, "INSERT INTO t VALUES(3,'é',3);")
            run_engine('UPDATE t SET y=y+1;', '--load', fresh)
            cases += 1
    print(f'PASS: {cases} STRICT declaration/value/conflict/transaction/image scenarios against SQLite 3.53.4')


if __name__ == '__main__':
    main()
