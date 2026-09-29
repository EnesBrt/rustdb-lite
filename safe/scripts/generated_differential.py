#!/usr/bin/env python3
"""Generated expressions, DML, lazy reads and native record/index interchange."""
import itertools
from pathlib import Path
import subprocess
import tempfile
from conflict_differential import reference_library, compare, POLICIES
from sql_differential import ROOT, ORACLE, run_engine
from index_differential import native


def main():
    subprocess.run(['cargo', 'build', '--example', 'statement_probe', '--bin', 'sqlite-safe-sql'], cwd=ROOT, check=True)
    lib = reference_library()
    cases = 0
    for storage, suffix, expr in itertools.product(['VIRTUAL', 'STORED'], ['', ' STRICT', ' WITHOUT ROWID', ' WITHOUT ROWID,STRICT'], [
        'x+id', 'length(x)', 'upper(x)', 'CASE WHEN id=2 THEN NULL ELSE x END',
        'coalesce(x,id)', 'x COLLATE NOCASE', 'CAST(x AS TEXT)', 'hex(x)',
    ]):
        compare(lib, [f'CREATE TABLE t(id INTEGER PRIMARY KEY,x ANY,g TEXT GENERATED ALWAYS AS({expr}) {storage},h INT AS(length(g)) STORED)',
            'PRAGMA table_info(t)', 'PRAGMA table_xinfo(t)', 'PRAGMA table_list(t)',
            "INSERT INTO t VALUES(1,'ab'),(2,'001'),(3,NULL) RETURNING *", 'SELECT * FROM t ORDER BY id',
            'CREATE INDEX ix ON t(g DESC,h)', 'PRAGMA index_xinfo(ix)',
            'SELECT t.*,typeof(g),typeof(h),(SELECT g),(SELECT t.g) FROM t ORDER BY id',
            'SELECT a.id,b.* FROM t a LEFT JOIN t b ON b.id=a.id+1 ORDER BY a.id',
            'CREATE VIEW v AS SELECT * FROM t', 'PRAGMA table_info(v)', 'CREATE TABLE copied AS SELECT g,h FROM v', 'PRAGMA table_info(copied)',
            "UPDATE t SET x='updated'||id WHERE id<3 RETURNING *", 'SELECT * FROM t ORDER BY id',
            "INSERT INTO t VALUES(1,'upsert') ON CONFLICT(id) DO UPDATE SET x=excluded.g RETURNING *", 'SELECT * FROM t ORDER BY id',
            'DELETE FROM t WHERE id=2 RETURNING *', 'SELECT * FROM t ORDER BY id',
            'INSERT INTO t(g) VALUES(1)', 'UPDATE t SET g=1', 'SELECT * FROM t ORDER BY id'])
        cases += 1
    for storage, suffix, policy, begin, schema, dml in itertools.product(['VIRTUAL', 'STORED'], ['', ' STRICT'], ['', *[' OR '+p for p in POLICIES]], [[], ['BEGIN'], ['SAVEPOINT s']], [
        'id INTEGER PRIMARY KEY,x INT,g INT AS(x*2) {storage} UNIQUE',
        'id INTEGER PRIMARY KEY,x INT,g INT AS(nullif(x,9)) {storage} NOT NULL',
        'id INTEGER PRIMARY KEY,x INT,g INT AS(x+1) {storage} CHECK(g<10)',
        'id INTEGER PRIMARY KEY,x INT NOT NULL ON CONFLICT REPLACE DEFAULT 7,g INT AS(nullif(x,9)) {storage} NOT NULL',
        'id INTEGER PRIMARY KEY,g INT AS(x*2) {storage} NOT NULL,x INT NOT NULL DEFAULT 7,h INT AS(g+1) STORED',
    ], ["INSERT{p} INTO t VALUES(3,3),(4,9),(5,NULL),(6,2)", "UPDATE{p} t SET x=CASE id WHEN 1 THEN 3 ELSE 9 END", "UPDATE{p} t SET id=id+10"]):
        declaration = schema.format(storage=storage)
        compare(lib, [f'CREATE TABLE t({declaration}){suffix}', 'INSERT INTO t VALUES(1,1),(2,2)', *begin,
            dml.format(p=policy), 'SELECT * FROM t ORDER BY id', 'ROLLBACK TO s', 'SELECT * FROM t ORDER BY id', 'COMMIT'])
        cases += 1
    for storage, policy, begin, expr in itertools.product(['VIRTUAL', 'STORED'], ['', *[' OR '+p for p in POLICIES]], [[], ['BEGIN']], ["CASE x WHEN 9 THEN 'bad' ELSE x END", 'abs(x)']):
        compare(lib, [f'CREATE TABLE t(x INT,g INT AS({expr}) {storage}) STRICT', 'INSERT INTO t VALUES(1)', *begin,
            f'INSERT{policy} INTO t VALUES(2),(9),(-9223372036854775808)', 'SELECT * FROM t ORDER BY x', 'COMMIT'])
        cases += 1
    for declaration in [
        'a,b AS(a+1)', 'a,b GENERATED ALWAYS AS(a+1) STORED', 'a,b GENERATED AS(a+1)',
        'a,b AS(a+1) DEFAULT 5', 'a,b DEFAULT 5 AS(a+1)', 'a,b AS(a+1) PRIMARY KEY',
        'a,b AS(a+1),PRIMARY KEY(b)', 'a AS(1)', 'a,b AS(rowid)', 'a,b AS(_rowid_)',
        'rowid,b AS(rowid+1)', 'a INTEGER PRIMARY KEY,b AS(a+1)', 'a,b AS(?1)',
        'a,b AS(changes())', 'a,b AS(last_insert_rowid())', 'a,b AS(sum(a))',
        'a,b AS((SELECT 1))', 'a,b AS(a+1) AS(a+2)', 'a,b AS(b+1)',
        'a,b AS(c+1),c AS(b+1)', 'a,b AS(c+1),c AS(b+1) STORED',
        'a,b AS(c+1) STORED,c AS(b+1) STORED', 'a,b AS(b+1) STORED', 'a,b AS("missing")',
        'a,b AS(t.a+1)', 'a,b AS(no_such(a))', 'a,b AS(c+1),c AS(a+1) STORED',
    ]:
        compare(lib, [f'CREATE TABLE t({declaration})', 'PRAGMA table_xinfo(t)', 'SELECT a FROM t', 'SELECT b FROM t',
            'INSERT INTO t VALUES(1)', 'SELECT * FROM t', 'UPDATE t SET a=2', 'SELECT * FROM t', 'DELETE FROM t RETURNING a'])
        cases += 1
    for statement in ['INSERT INTO t VALUES(1) RETURNING G', 'UPDATE t SET x=2 RETURNING G', 'DELETE FROM t RETURNING G']:
        compare(lib,['CREATE TABLE t(x,g AS(x+1))','INSERT INTO t VALUES(0)',statement])
        cases += 1
    with tempfile.TemporaryDirectory(prefix='safe-generated-') as folder:
        folder = Path(folder)
        for n, (page_size, encoding, mode, suffix) in enumerate(itertools.product([512,1024,2048,4096,8192,16384,32768,65536], ['UTF-8', 'UTF-16le', 'UTF-16be'], [0, 1, 2], ['', ' WITHOUT ROWID'])):
            original = folder/f'{n}-native.db'; saved = folder/f'{n}-rust.db'; final = folder/f'{n}-final.db'
            schema = "id INTEGER PRIMARY KEY,v TEXT AS(upper(x)) VIRTUAL,x TEXT,s INT AS(length(v)) STORED,b BLOB DEFAULT x'1234',w TEXT AS(v||s)"
            native(original, f"PRAGMA page_size={page_size};PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t({schema}){suffix};CREATE UNIQUE INDEX ix ON t(v);CREATE INDEX iy ON t(w,s DESC);INSERT INTO t(id,x) VALUES(1,'é'),(2,'🦀'),(3,'abc');")
            sql = "UPDATE t SET x=x||id;INSERT INTO t(id,x) VALUES(4,'rust');CREATE TABLE u(a INT,b TEXT AS(a||'é'),c INT AS(length(b)) STORED);CREATE INDEX ui ON u(b DESC,c);INSERT INTO u VALUES(5);"
            run_engine(sql, '--load', original, '--save', saved)
            native(original, sql)
            query = 'SELECT * FROM t ORDER BY id'
            assert native(saved, query) == native(original, query), (n, 'rows')
            for q in ['PRAGMA table_info(t)', 'PRAGMA table_xinfo(t)', 'PRAGMA index_xinfo(ix)', 'PRAGMA index_xinfo(iy)', 'PRAGMA index_xinfo(t)']:
                assert native(saved, q) == native(original, q), (n, q)
            assert native(saved, 'PRAGMA integrity_check') == [{'integrity_check': 'ok'}], n
            assert native(saved, 'SELECT * FROM t INDEXED BY ix ORDER BY id') == native(saved, query)
            native(saved, "INSERT INTO t(id,x) VALUES(5,'native');DELETE FROM t WHERE id=2;UPDATE t SET x='changed' WHERE id=3;")
            run_engine('CREATE VIEW v AS SELECT * FROM t;', '--load', saved, '--save', final)
            assert native(final, query) == native(saved, query)
            assert native(final, 'PRAGMA integrity_check') == [{'integrity_check': 'ok'}], n
            cases += 1
        # ALTER adds virtual columns without evaluating pre-existing rows. Import
        # must retain lazy errors, and must never recalculate stored file values.
        source = folder/'lazy.db'; result = folder/'lazy-result.db'
        native(source, 'CREATE TABLE t(a);INSERT INTO t VALUES(-9223372036854775808);ALTER TABLE t ADD COLUMN b AS(abs(a));')
        run_engine('SELECT a FROM t;SELECT CASE WHEN a<0 THEN 1 ELSE b END FROM t;', '--load', source, '--save', result)
        assert native(source, 'SELECT a FROM t') == native(result, 'SELECT a FROM t')
        run_engine('SELECT b FROM t;', '--load', source, fail=True)
        cases += 1
        # Preserve stored values even when the generating expression in the
        # schema changes; only subsequent writes should recompute them.
        source = folder/'stored.db'; result = folder/'stored-result.db'
        native(source, 'CREATE TABLE t(a,b AS(a+1) STORED,c AS(b+1));INSERT INTO t VALUES(5);')
        changed = subprocess.run([str(ORACLE), str(source)], input=".dbconfig defensive off\nPRAGMA writable_schema=ON;UPDATE sqlite_schema SET sql=replace(sql,'a+1','a+9') WHERE name='t';\n", text=True, capture_output=True)
        assert changed.returncode == 0, changed.stderr
        expected = native(source, 'SELECT * FROM t')
        run_engine('SELECT * FROM t;', '--load', source, '--save', result)
        assert native(result, 'SELECT * FROM t') == expected == [{'a':5,'b':6,'c':7}]
        run_engine('UPDATE t SET a=10;', '--load', result, '--save', folder/'stored-update.db')
        assert native(folder/'stored-update.db', 'SELECT * FROM t') == [{'a':10,'b':19,'c':20}]
        cases += 1
    print(f'PASS: {cases} generated-column declaration/value/conflict/counter/metadata/image scenarios against SQLite 3.53.4')


if __name__ == '__main__': main()
