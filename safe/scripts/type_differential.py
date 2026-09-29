#!/usr/bin/env python3
"""Compound result metadata, CTAS storage classes and scalar-query affinities."""
import subprocess
import tempfile
import json
from pathlib import Path
from sql_differential import ROOT, ORACLE, check, run_engine

SETUP = "CREATE TABLE t(a,b BLOB,n NUMERIC,r REAL,s VARCHAR(10) COLLATE NOCASE);INSERT INTO t VALUES('0123',x'31',1,1,'A');"
EXPRESSIONS = [
    'NULL', '1', '1.0', "'0123'", "x'3132'", 'a', 'b', 'n', 'r', 's',
    'CAST(1 AS INT)', 'CAST(1 AS REAL)', 'CAST(1 AS NUMERIC)',
    'CAST(1 AS BLOB)', 'CAST(1 AS TEXT)', '+n', "n||''",
    'CASE WHEN 1 THEN n ELSE s END', 'abs(1)', '(SELECT n FROM t)',
    "'A' COLLATE NOCASE",
]

def native_batch(path, script):
    result = subprocess.run([str(ORACLE), '-json', str(path)], input=script,
                            text=True, capture_output=True, timeout=30)
    assert result.returncode == 0, (script, result.returncode, result.stderr)
    decoder = json.JSONDecoder()
    rest = result.stdout.strip()
    results = []
    while rest:
        value, end = decoder.raw_decode(rest)
        results.append(value)
        rest = rest[end:].lstrip()
    return results

def metadata_rows(result):
    return [[None if v['type'] == 'null' else
             bytes.fromhex(v['hex']).decode('utf-8') if v['type'] == 'text' else
             v['value'] for v in row] for row in result['rows']]

def typed_rows(rows):
    result = []
    for row in rows:
        kind = row['t']
        value = {'type': kind}
        if kind == 'integer': value['value'] = row['v']
        elif kind == 'real': value['bits'] = row['v']
        elif kind in ('text', 'blob'): value['hex'] = row['v']
        result.append([value])
    return result

def scenario(directory, number, query, setup=SETUP):
    reference = directory / f'reference-{number}.db'
    saved = directory / f'rust-{number}.db'
    script = setup + 'CREATE VIEW v AS ' + query + ';'
    script += 'CREATE VIEW nested AS SELECT * FROM v;CREATE VIEW scalar AS SELECT (SELECT x FROM v) x;'
    script += 'CREATE TABLE z AS SELECT * FROM v;CREATE TABLE direct AS ' + query + ';'
    pragmas = ''.join(f'PRAGMA table_info({name});' for name in ['v', 'nested', 'scalar', 'z', 'direct'])
    reads = 'SELECT x FROM z ORDER BY rowid;SELECT x FROM direct ORDER BY rowid;'
    tagged = ''.join(f"SELECT typeof(x) t,CASE typeof(x) WHEN 'text' THEN hex(x) WHEN 'blob' THEN hex(x) WHEN 'real' THEN hex(ieee754_to_blob(x)) ELSE x END v FROM {name} ORDER BY rowid;" for name in ['z', 'direct'])
    inspect = pragmas + tagged + 'SELECT name,sql FROM sqlite_schema ORDER BY name;PRAGMA integrity_check;'
    expected = native_batch(reference, script + inspect)
    actual = run_engine(script + pragmas + reads, '--save', saved)[-7:]
    reopened = run_engine(pragmas + reads, '--load', saved)
    for i in range(5):
        want = [list(row.values()) for row in expected[i]]
        assert metadata_rows(actual[i]) == want, (query, i, actual[i], expected[i])
        assert metadata_rows(reopened[i]) == want, (query, i, reopened[i], expected[i])
    for i in [5, 6]:
        want = typed_rows(expected[i])
        assert actual[i]['rows'] == want, (query, i, actual[i], want)
        assert reopened[i]['rows'] == want, (query, i, reopened[i], want)
    assert native_batch(saved, inspect) == expected, query
    assert expected[-1] == [{'integrity_check': 'ok'}]

def main():
    subprocess.run(['cargo', 'build', '--bin', 'sqlite-safe-sql'], cwd=ROOT, check=True)
    cases = 0
    with tempfile.TemporaryDirectory(prefix='safe-types-') as directory:
        directory = Path(directory)
        for left in EXPRESSIONS:
            for right in EXPRESSIONS:
                scenario(directory, cases, f'SELECT {left} x FROM t UNION ALL SELECT {right} FROM t')
                cases += 1
        for query in [
            "SELECT NULL x UNION ALL SELECT CAST(1 AS REAL) UNION ALL SELECT '2'",
            'SELECT n x FROM t UNION SELECT r FROM t',
            'SELECT n x FROM t INTERSECT SELECT 1.0',
            "SELECT s x FROM t EXCEPT SELECT 'b'",
            "WITH q(x) AS(VALUES(CAST(1 AS INT)),('2')) SELECT * FROM q",
            'WITH q(x) AS(VALUES(CAST(1 AS REAL)),(2)) SELECT * FROM q',
            'WITH RECURSIVE q(x) AS(SELECT CAST(1 AS INT) UNION ALL SELECT x+1 FROM q WHERE x<3) SELECT * FROM q',
            'WITH RECURSIVE q(x) AS(SELECT n FROM t UNION ALL SELECT x+1 FROM q WHERE x<3) SELECT * FROM q',
            "WITH RECURSIVE q(x) AS(SELECT CAST(1 AS INT) UNION ALL SELECT 'z' FROM q WHERE x<2) SELECT * FROM q",
            'SELECT a x FROM t',
            'SELECT CAST(a AS BLOB) x FROM t',
            "WITH q(x) AS(VALUES(CAST(1 AS TEXT)),(2) UNION ALL SELECT NULL) SELECT * FROM q",
            "WITH q(x) AS(VALUES(NULL),(CAST(1 AS INT)) UNION ALL SELECT 2.0) SELECT * FROM q",
            "WITH q(x) AS(VALUES('a' COLLATE NOCASE),('A') UNION SELECT 'a') SELECT * FROM q",
            "WITH RECURSIVE q(x) AS(VALUES('a' COLLATE NOCASE),('A') UNION SELECT x FROM q) SELECT * FROM q",
        ]:
            scenario(directory, cases, query)
            cases += 1
    for left in ['n', 'r', 's', 'NULL', "'0123'", 'CAST(1 AS INT)']:
        for right in ['n', 'r', 's', 'NULL', "'0123'", 'CAST(1 AS INT)']:
            query = f'SELECT {left} FROM t UNION ALL SELECT {right} FROM t'
            check(SETUP, f"SELECT ({query})='1' c0,({query})=1 c1,'1' IN({query}) c2,1 IN({query}) c3", 4)
            cases += 1
    for value in ['9007199254740993','9223372036854775807','-9007199254740993','-9223372036854775808']:
        for left in [value, f'CAST({value} AS INT)',f'CAST({value} AS REAL)',f"'{value}'"]:
            check('', f'SELECT {left} IN(SELECT CAST({value} AS REAL)) c0,{left}=CAST({value} AS REAL) c1', 2)
            cases += 1
    print(f'PASS: {cases} compound-type/schema/value/scalar scenarios against SQLite 3.53.4')

if __name__ == '__main__':
    main()
