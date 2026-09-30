#!/usr/bin/env python3
"""ALTER constraint validation, policies, schema text and files vs SQLite 3.53.4."""
import itertools
import subprocess
import tempfile
from pathlib import Path
from conflict_differential import reference_library, compare
from index_differential import native
from sql_differential import ROOT, run_engine


def main():
    subprocess.run(['cargo', 'build', '--example', 'statement_probe', '--bin', 'sqlite-safe-sql'], cwd=ROOT, check=True)
    lib = reference_library()
    cases = 0

    def check(statements):
        nonlocal cases
        compare(lib, statements)
        cases += 1

    policies = ['', ' ON CONFLICT ABORT', ' ON CONFLICT ROLLBACK', ' ON CONFLICT FAIL', ' ON CONFLICT IGNORE', ' ON CONFLICT REPLACE']
    definitions = ['a INT', 'a INT NOT NULL', 'a INT CONSTRAINT nn NOT NULL',
        'a INT DEFAULT 9 NOT NULL ON CONFLICT REPLACE',
        'a INT NOT NULL ON CONFLICT IGNORE NOT NULL ON CONFLICT FAIL',
        'a INT CONSTRAINT one CONSTRAINT two NOT NULL ON CONFLICT REPLACE',
        'a INT CONSTRAINT ck CHECK(a>0)', 'a INT UNIQUE',
        'a INT AS(id+1)', 'a INT AS(NULL)', 'a INT AS(id+1) STORED']
    for schema, definition, value, policy in itertools.product(['', ' STRICT', ' WITHOUT ROWID'], definitions, [None,'1','NULL'], policies):
        generated = ' AS(' in definition
        initial = [] if value is None else [f'INSERT INTO t(id{",a" if not generated else ""}) VALUES(1{","+value if not generated else ""})']
        check([f'CREATE TABLE t(id INT PRIMARY KEY,{definition}){schema}'] + initial + [
            f'ALTER TABLE main.t ALTER COLUMN a SET NOT NULL{policy}', 'PRAGMA table_xinfo(t)',
            'BEGIN', 'INSERT INTO t(id,a) VALUES(2,NULL),(3,3)', 'SELECT * FROM t ORDER BY id',
            'COMMIT', 'ALTER TABLE t ALTER a DROP NOT NULL', 'PRAGMA table_xinfo(t)',
            'INSERT INTO t(id,a) VALUES(4,NULL)', 'SELECT * FROM t ORDER BY id'])
    expressions = ['a>0','a','a IS NOT NULL','NULL','0','1',"'true'","'1x'", 'a+id>0',
        'abs(a)<10', 'a GLOB \'[0-9]*\'', 'typeof(a)=\'integer\'', '(id,a)>(0,0)',
        'changes()>=0', 'missing>0','?','(SELECT a)','sum(a)>0',
         'abs(-9223372036854775808)>0','t.a>0','"a">0',
        '0 AND abs(-9223372036854775808)', 'abs(-9223372036854775808) AND 0',
        '1 OR abs(-9223372036854775808)', 'abs(-9223372036854775808) OR true',
        'a>0 OR abs(-9223372036854775808)>0', 'a>0 AND abs(-9223372036854775808)>0',
        'NOT (a<0 AND abs(-9223372036854775808)>0)',
        'CASE WHEN a>0 THEN 1 ELSE abs(-9223372036854775808) END',
        "like('a','a','')"]
    for schema, value, expression, named in itertools.product(['', ' STRICT', ' WITHOUT ROWID'], [None,'2','NULL','-1'], expressions, [False,True]):
        check([f'CREATE TABLE t(id INT PRIMARY KEY,a INT){schema}']
            + ([] if value is None else [f'INSERT INTO t VALUES(1,{value})'])
            + [f'ALTER TABLE t ADD {"CONSTRAINT added " if named else ""}CHECK({expression})',
                'PRAGMA table_xinfo(t)', 'INSERT INTO t VALUES(2,NULL)', 'INSERT INTO t VALUES(3,-2)',
                'SELECT * FROM t ORDER BY id', 'ALTER TABLE t DROP CONSTRAINT added',
                'INSERT INTO t VALUES(4,-2)', 'SELECT * FROM t ORDER BY id'])
    for expression, true_value in itertools.product(['a','NULL','0','1'], ['0','1','NULL','2']):
        check(['CREATE TABLE t(a,"true")',f'INSERT INTO t VALUES(1,{true_value})',
            f'ALTER TABLE t ADD CHECK({expression})','INSERT INTO t VALUES(NULL,0)','SELECT * FROM t'])
    for name in ['c', 'C', 'é', '🦀', "a'b", 'a"b', 'constraint', 'with space']:
        quoted = '"'+name.replace('"','""')+'"'
        check(['CREATE TABLE t(a)', 'INSERT INTO t VALUES(1)',
            f'ALTER TABLE t ADD CONSTRAINT {quoted} CHECK(a>0)',
            f'ALTER TABLE t ADD CONSTRAINT {quoted} CHECK(a<10)',
            f'ALTER TABLE t DROP CONSTRAINT {quoted}',
            'INSERT INTO t VALUES(-1)', 'SELECT * FROM t'])
    for definition, name in itertools.product([
        'a CONSTRAINT nn NOT NULL,b', 'a CONSTRAINT ck CHECK(a>0),b',
        'a,b,CONSTRAINT ck CHECK(a>0)', 'a,b,CONSTRAINT ck CHECK(a>0),CHECK(b>0)',
        'a CONSTRAINT label DEFAULT 9,b', 'a CONSTRAINT label COLLATE nocase,b',
        'a CONSTRAINT label AS(b+1),b', 'a CONSTRAINT label GENERATED ALWAYS AS(b+1),b',
        'a CONSTRAINT label,b', 'a CONSTRAINT one CONSTRAINT two NOT NULL,b',
        'a CONSTRAINT pk PRIMARY KEY,b', 'a CONSTRAINT uq UNIQUE,b',
        'a,b,CONSTRAINT pk PRIMARY KEY(a,b)', 'a,b,CONSTRAINT uq UNIQUE(a,b)',
        'a CONSTRAINT ck CHECK(a>0),b CONSTRAINT ck CHECK(b>0)',
        'a NOT NULL ON CONFLICT IGNORE,b CONSTRAINT nn NOT NULL',
    ], ['nn','ck','label','one','two','pk','uq','missing']):
        check([f'CREATE TABLE t({definition})', f'ALTER TABLE t DROP CONSTRAINT {name}',
            'PRAGMA table_xinfo(t)', 'PRAGMA index_list(t)',
            'INSERT INTO t VALUES(NULL,NULL)', 'SELECT * FROM t'])
    for schema in ['', ' STRICT', ' WITHOUT ROWID']:
        check([f'CREATE TABLE t(id INTEGER PRIMARY KEY,a INT){schema}','INSERT INTO t VALUES(1,2)',
            'CREATE VIEW broken AS SELECT * FROM absent', 'BEGIN',
            'ALTER TABLE t ALTER id SET NOT NULL ON CONFLICT IGNORE',
            'ALTER TABLE t ALTER id DROP NOT NULL', 'ALTER TABLE t ADD CONSTRAINT c CHECK(a>0)',
            'SAVEPOINT s', 'ALTER TABLE t DROP CONSTRAINT c', 'INSERT INTO t VALUES(2,-1)',
            'ALTER TABLE t ADD CHECK(a>0) ON CONFLICT ROLLBACK', 'SELECT * FROM t ORDER BY id',
            'ROLLBACK TO s', 'INSERT INTO t VALUES(3,-1)', 'COMMIT',
            'ALTER TABLE t ALTER rowid SET NOT NULL', 'ALTER TABLE broken ADD CHECK(1)',
            'ALTER TABLE absent ALTER a DROP NOT NULL', 'SELECT * FROM t ORDER BY id'])
    for policy in policies:
        check(['CREATE TABLE t(a)', 'INSERT INTO t VALUES(NULL)', 'BEGIN',
            f'ALTER TABLE t ALTER a SET NOT NULL{policy}', f'ALTER TABLE t ADD CHECK(a>0){policy}',
            'SELECT * FROM t', 'COMMIT', 'PRAGMA table_xinfo(t)'])
    with tempfile.TemporaryDirectory(prefix='safe-alter-constraint-') as folder:
        folder = Path(folder)
        for n, (size, encoding, mode, wr) in enumerate(itertools.product(
                [512,1024,2048,4096,8192,16384,32768,65536],
                ['UTF-8','UTF-16le','UTF-16be'], [0,1,2], [False,True])):
            source,saved,final=[folder/f'{n}-{part}.db' for part in ['source','saved','final']]
            native(source,f"PRAGMA page_size={size};PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(id INT PRIMARY KEY,a INT CONSTRAINT old NOT NULL ON CONFLICT IGNORE,b TEXT,s INT AS(a*2) STORED,g INT AS(s+1), CONSTRAINT chk CHECK(a>0)) STRICT{' , WITHOUT ROWID' if wr else ''};INSERT INTO t(id,a,b) VALUES(1,2,'é🦀'),(2,3,'rust');CREATE INDEX ix ON t((s+g)) WHERE a>0;CREATE VIEW v AS SELECT id,b,g FROM t;")
            sql="ALTER TABLE t ALTER a SET NOT NULL ON CONFLICT REPLACE;ALTER TABLE t ADD CONSTRAINT [positive value] CHECK(s>0);ALTER TABLE t ALTER COLUMN b SET NOT NULL;ALTER TABLE t DROP CONSTRAINT chk;"
            run_engine(sql,'--load',source,'--save',saved)
            native(source,sql)
            queries=['SELECT * FROM t ORDER BY id','SELECT * FROM v ORDER BY id','PRAGMA table_xinfo(t)','PRAGMA index_xinfo(ix)','SELECT type,name,sql FROM sqlite_schema ORDER BY name','PRAGMA integrity_check']
            for q in queries:
                assert native(saved,q)==native(source,q),(n,q,native(saved,q),native(source,q))
            native(saved,"ALTER TABLE t ALTER b DROP NOT NULL;INSERT INTO t(id,a,b) VALUES(3,4,NULL)")
            sql='ALTER TABLE t DROP CONSTRAINT [positive value];ALTER TABLE t ALTER a DROP NOT NULL;UPDATE t SET a=-a;'
            run_engine(sql,'--load',saved,'--save',final)
            native(saved,sql)
            for q in queries:
                assert native(final,q)==native(saved,q),(n,q)
            cases += 1
        definitions=[
            'a /* before */ CONSTRAINT nn NOT /* middle */ NULL /* after */, b',
            'a NOT NULL ON CONFLICT IGNORE /* last */,b',
            'a CONSTRAINT one CONSTRAINT two NOT NULL CHECK(a>0),b',
            "a DEFAULT 'NOT NULL,CONSTRAINT c CHECK(a)' CONSTRAINT nn NOT NULL,b",
            'a,b, /* before */ CONSTRAINT ck CHECK(a IN (1,2)) /* after */',
            'a,b,CONSTRAINT ck CHECK(a>0) ON CONFLICT FAIL, UNIQUE(b)',
            'a /* c */, b, CONSTRAINT label CHECK(a>0) CONSTRAINT ck CHECK(b>0)',
        ]
        edits=['ALTER TABLE t ALTER a DROP NOT NULL','ALTER TABLE t ALTER a SET NOT NULL ON CONFLICT FAIL /* keep */ -- discard\n',
            'ALTER TABLE t ADD CONSTRAINT added CHECK(a>0) /* keep */ -- discard\n',
            'ALTER TABLE t ADD CHECK(1) -- discard\n /* keep */',
            'ALTER TABLE t DROP CONSTRAINT nn','ALTER TABLE t DROP CONSTRAINT ck','ALTER TABLE t DROP CONSTRAINT one']
        for n,(definition,sql) in enumerate(itertools.product(definitions,edits)):
            source,saved=folder/f'comments-{n}.db',folder/f'comments-{n}-saved.db'
            native(source,f'CREATE TABLE t({definition})')
            # Missing named constraints are compared as errors in the SQL matrix.
            if 'DROP CONSTRAINT' in sql and ('CONSTRAINT '+sql.split()[-1]) not in definition:
                continue
            run_engine(sql,'--load',source,'--save',saved)
            native(source,sql)
            for q in ['SELECT sql FROM sqlite_schema ORDER BY name','PRAGMA table_xinfo(t)','PRAGMA integrity_check']:
                assert native(saved,q)==native(source,q),(n,definition,sql,q,native(saved,q),native(source,q))
            cases += 1
    print(f'PASS: {cases} ALTER constraint/nullability/policy/schema/image scenarios against SQLite 3.53.4')

if __name__ == '__main__':
    main()
