#!/usr/bin/env python3
"""Aggregate FILTER and ORDER BY against the independent SQLite 3.53.4 oracle."""
import itertools
import subprocess
import tempfile
from pathlib import Path
from conflict_differential import reference_library, compare
from sql_differential import ROOT, run_engine
from index_differential import native

SEED = [
    'CREATE TABLE t(g INT,x,y TEXT COLLATE NOCASE,z INT,k INT)',
    "INSERT INTO t VALUES(1,3,'b',1,5),(1,1,'A',1,4),(1,2,'a',0,3),(1,2.0,'B',NULL,2),(1,NULL,'c',1,1),(2,4,'z',0,7),(2,5,'Y',1,6)",
]

def main():
    subprocess.run(['cargo', 'build', '--example', 'statement_probe', '--bin', 'sqlite-safe-sql'], cwd=ROOT, check=True)
    lib = reference_library()
    cases = 0
    for function, distinct, order, predicate in itertools.product(
        ['count(x)', 'sum(x)', 'total(x)', 'avg(x)', 'min(y)', 'max(y)', 'group_concat(y)', "group_concat(y,':'||k)", "string_agg(y,':'||k)"],
        ['', 'DISTINCT '],
        ['', ' ORDER BY k', ' ORDER BY k DESC', ' ORDER BY z NULLS LAST,k', ' ORDER BY y COLLATE NOCASE DESC,k', ' ORDER BY x', ' ORDER BY 1'],
        ['', ' FILTER(WHERE z)', ' FILTER(WHERE x>2)', ' FILTER(WHERE NULL)', ' FILTER(WHERE 0)', ' FILTER(WHERE (SELECT t.k)%2)'],
    ):
        name, args = function[:-1].split('(', 1)
        expr = f'{name}({distinct}{args}{order}){predicate}'
        compare(lib, SEED + [f'SELECT {expr} FROM t', f'SELECT g,{expr} FROM t GROUP BY g ORDER BY g'])
        cases += 1
    for function, expression, order in itertools.product(
        ['group_concat', 'sum', 'total', 'avg', 'min', 'max', 'count'],
        ['x', 'y COLLATE NOCASE', 'y COLLATE BINARY', '+y', 'length(y)'],
        ['ASC', 'DESC', 'ASC NULLS LAST', 'DESC NULLS FIRST'],
    ):
        compare(lib, SEED + [f'SELECT {function}(DISTINCT {expression} ORDER BY {expression} {order}) FILTER(WHERE z IS NOT NULL) FROM t'])
        cases += 1
    for q in [
        'SELECT count(*) FILTER(WHERE z),count() FILTER(WHERE x IS NULL) FROM t',
        'SELECT count(ORDER BY missing),count(ALL ORDER BY abs(-9223372036854775808)) FILTER(WHERE z) FROM t',
        'SELECT min(x ORDER BY abs(-9223372036854775808)),max(x ORDER BY abs(-9223372036854775808)) FROM t',
        'SELECT group_concat(abs(-9223372036854775808) ORDER BY k) FILTER(WHERE 0) FROM t',
        'SELECT group_concat(y ORDER BY abs(-9223372036854775808)) FILTER(WHERE 0) FROM t',
        'SELECT sum(abs(-9223372036854775808)) FILTER(WHERE x IS NULL) FROM t',
        'SELECT count(x ORDER BY CASE WHEN x IS NULL THEN abs(-9223372036854775808) ELSE 1 END) FROM t',
        'SELECT g,min(k) FILTER(WHERE z),y FROM t GROUP BY g ORDER BY g',
        'SELECT g,max(k) FILTER(WHERE z),y,count(*) FROM t GROUP BY g ORDER BY g',
        'SELECT g,y,min(k) FILTER(WHERE 0) FROM t GROUP BY g ORDER BY g',
        'SELECT y,min(k) FILTER(WHERE z),min(k) FILTER(WHERE z) FROM t',
        'SELECT g,group_concat(y ORDER BY k) FILTER(WHERE z) AS label FROM t GROUP BY g HAVING count(*) FILTER(WHERE z)>0 ORDER BY label',
        'SELECT g FROM t GROUP BY g HAVING min(k) FILTER(WHERE z)>0 ORDER BY sum(x ORDER BY k) FILTER(WHERE z)',
        'SELECT group_concat(y ORDER BY (SELECT t.k)) FILTER(WHERE EXISTS(SELECT 1 WHERE t.z)) FROM t',
        'SELECT a.g,(SELECT group_concat(t.y ORDER BY a.k,t.k) FILTER(WHERE t.z AND t.g=a.g) FROM t) FROM t a ORDER BY a.k',
        'SELECT EXISTS(SELECT group_concat(abs(-9223372036854775808) ORDER BY k) FILTER(WHERE 0) FROM t)',
        'SELECT EXISTS(SELECT group_concat(abs(-9223372036854775808) ORDER BY k) FROM t)',
        'SELECT sum(ALL x ORDER BY k) FILTER(WHERE z) FROM t',
        'SELECT abs(x) filter FROM t ORDER BY k',
        'SELECT sum(x) FILTER(WHERE z)+1,typeof(sum(x ORDER BY k)) FROM t',
        'SELECT group_concat(y ORDER BY k) FILTER(WHERE z) FROM t WHERE 0',
        'SELECT count(x ORDER BY k) FILTER(WHERE z),total(x ORDER BY k) FILTER(WHERE z) FROM t WHERE 0',
        'SELECT abs(x ORDER BY k) FROM t',
        'SELECT abs(x) FILTER(WHERE z) FROM t',
        'SELECT min(x,y) FILTER(WHERE z) FROM t',
        'SELECT sum(x ORDER BY sum(k)) FROM t',
        'SELECT sum(x) FILTER(WHERE sum(z)) FROM t',
        'SELECT sum(x ORDER BY missing) FILTER(WHERE 0) FROM t',
        'SELECT min(x ORDER BY missing) FROM t',
        'SELECT sum(x) FILTER(WHERE missing) FROM t',
        'SELECT sum(x) FILTER(WHERE z) FROM t WHERE sum(x) FILTER(WHERE z)',
        'SELECT sum(x) FILTER(WHERE z) FROM t GROUP BY sum(x) FILTER(WHERE z)',
        'SELECT count(* ORDER BY k) FROM t',
        'SELECT count(DISTINCT *) FILTER(WHERE z) FROM t',
        'SELECT sum(x ORDER BY k LIMIT 1) FROM t',
        'SELECT sum(x) FILTER(WHERE z) FILTER(WHERE k) FROM t',
        'CREATE INDEX bad ON t(sum(x ORDER BY k))',
        'CREATE TABLE bad(x,y AS(sum(x) FILTER(WHERE x)))',
        'WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT sum(x) FILTER(WHERE x<3) FROM q) SELECT * FROM q',
    ]:
        compare(lib, SEED + [q])
        cases += 1
    for function, predicate in itertools.product(['min','max'], ['', ' FILTER(WHERE z)', ' FILTER(WHERE 0)']):
        compare(lib, ['CREATE TABLE t(x,y,z)',"INSERT INTO t VALUES('a',NULL,1),('b',NULL,0),('c',NULL,1)",f'SELECT x,{function}(y){predicate} FROM t'])
        cases += 1
    for order in ['k', 'k DESC', 'x', 'x DESC']:
        compare(lib, ['CREATE TABLE t(x,k)', 'INSERT INTO t VALUES(9223372036854775807,1),(1,2),(-1,3),(0.0,4)', f'SELECT sum(x ORDER BY {order}),total(x ORDER BY {order}),avg(x ORDER BY {order}) FROM t'])
        cases += 1
    for vals, function, distinct, predicate in itertools.product(
        ["(1,NULL),(2,NULL),(3,NULL)", "(1,2),(2,1),(3,1)", "(1,1),(2,2),(3,1)", "(1,NULL),(2,1),(3,NULL)"],
        ['min', 'max'], ['', 'DISTINCT '], ['', ' FILTER(WHERE a!=2)', ' FILTER(WHERE 1)'],
    ):
        compare(lib, ['CREATE TABLE t(a,b)', 'INSERT INTO t VALUES'+vals, f'SELECT a,{function}({distinct}b){predicate} FROM t'])
        cases += 1
    for first, second, first_filter, second_filter in itertools.product(
        ['min(b)', 'max(b)', 'min(DISTINCT b)'], ['min(a)', 'max(b)', 'max(DISTINCT a)'],
        ['', ' FILTER(WHERE a!=2)'], ['', ' FILTER(WHERE a!=1)'],
    ):
        compare(lib, ['CREATE TABLE t(a,b)', 'INSERT INTO t VALUES(1,NULL),(2,1),(3,1),(4,3)', f'SELECT a,b,{first}{first_filter},{second}{second_filter} FROM t'])
        cases += 1
    for q in [
        "SELECT group_concat(x ORDER BY y COLLATE NOCASE)='a,A' FROM t",
        "SELECT min(x ORDER BY y COLLATE NOCASE)='a' FROM t",
        "SELECT min(x) FILTER(WHERE z COLLATE NOCASE)='a' FROM t",
        "SELECT min(x COLLATE BINARY ORDER BY y COLLATE NOCASE)='a' FROM t",
        'SELECT count(ALL *) FROM t', 'SELECT count(DISTINCT ORDER BY y) FROM t',
        "SELECT group_concat(x,CAST(x'002c' AS TEXT) ORDER BY y DESC) FILTER(WHERE z) FROM t",
    ]:
        compare(lib, ['CREATE TABLE t(x,y,z)', "INSERT INTO t VALUES('A',1,1),('a',2,0),('é',3,1)", q])
        cases += 1
    for schema in ['', ' WITHOUT ROWID']:
        statements = [f'CREATE TABLE t(id INT PRIMARY KEY,g INT,x TEXT,k INT,keep INT){schema}',
            "INSERT INTO t VALUES(1,1,'a',3,1),(2,1,'b',2,0),(3,2,'c',1,1)",
            'CREATE VIEW v AS SELECT g,group_concat(x ORDER BY k) FILTER(WHERE keep) label FROM t GROUP BY g',
            'CREATE TABLE copied AS SELECT * FROM v', 'BEGIN',
            "UPDATE t SET x=(SELECT group_concat(b.x ORDER BY b.k) FILTER(WHERE b.keep) FROM t b WHERE b.g=t.g)",
            'SELECT * FROM v ORDER BY g', 'ROLLBACK',
            "INSERT INTO t SELECT 10+g,g,label,5,1 FROM v WHERE 1 ON CONFLICT(id) DO UPDATE SET x=excluded.x",
            'SELECT * FROM v ORDER BY g', 'SELECT * FROM copied ORDER BY g',
            'UPDATE t SET keep=0 WHERE id=1 RETURNING (SELECT count(*) FILTER(WHERE keep) FROM t)']
        compare(lib, statements)
        cases += 1
    with tempfile.TemporaryDirectory(prefix='safe-aggregate-') as folder:
        folder = Path(folder)
        for n, (page_size, encoding, mode) in enumerate(itertools.product(
            [512,1024,2048,4096,8192,16384,32768,65536], ['UTF-8','UTF-16le','UTF-16be'], [0,1,2],
        )):
            source=folder/f'{n}.db'; result=folder/f'{n}-rust.db'; final=folder/f'{n}-final.db'
            native(source, f"PRAGMA page_size={page_size};PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(g INT,x TEXT,k INT,keep INT);INSERT INTO t VALUES(1,'é',3,1),(1,'Rust',2,1),(1,'🦀',1,0),(2,'safe',4,1);CREATE VIEW v AS SELECT g,group_concat(x,'|' ORDER BY k) FILTER(WHERE keep) AS label,count(*) FILTER(WHERE keep) AS n FROM t GROUP BY g;")
            sql='CREATE TABLE copied AS SELECT * FROM v;UPDATE t SET keep=1 WHERE k=1;'
            run_engine(sql,'--load',source,'--save',result); native(source,sql)
            for q in ['SELECT * FROM v ORDER BY g','SELECT * FROM copied ORDER BY g','PRAGMA table_info(v)','PRAGMA table_info(copied)']:
                assert native(result,q)==native(source,q),(n,q)
            assert native(result,'PRAGMA integrity_check')==[{'integrity_check':'ok'}]
            native(result,"INSERT INTO t VALUES(2,'native',5,1)")
            run_engine('UPDATE t SET k=-k;','--load',result,'--save',final); native(result,'UPDATE t SET k=-k;')
            assert native(final,'SELECT * FROM v ORDER BY g')==native(result,'SELECT * FROM v ORDER BY g')
            assert native(final,'PRAGMA integrity_check')==[{'integrity_check':'ok'}]
            cases += 1
    print(f'PASS: {cases} aggregate FILTER/ORDER BY/type/scope/error scenarios against SQLite 3.53.4')

if __name__ == '__main__':
    main()
