#!/usr/bin/env python3
"""Compare the Rust executable to the exact upstream C build, in separate processes."""
import argparse
import json
import math
import random
import subprocess
import tempfile
from pathlib import Path
from config import ROOT

CASES = {
    "integer boundaries": "SELECT 9223372036854775807+1 AS a, -9223372036854775808/-1 AS b, abs(-5) AS c, 7/2 AS d, -7%3 AS e, ~5 AS f, 1<<63 AS g",
    "null logic": "SELECT NULL=NULL AS a,NULL IS NULL AS b,NULL AND 0 AS c,NULL OR 1 AS d,3 NOT IN(1,NULL) AS e,coalesce(NULL,0,2) AS f",
    "affinity": "SELECT CAST('123xyz' AS INTEGER) AS a, CAST('  1.5e2 ' AS NUMERIC) AS b, typeof('3'+0) AS c, '1'=1 AS d,CAST('x' AS REAL) AS e",
    "strings": "SELECT substr('hé🦀llo',-4,3) AS a, length('hé🦀llo') AS b, instr('banana','ana') AS c, trim('xxhelloox','xo') AS d, replace('abcabc','ab','z') AS e, printf('%04d %.3f %q',7,1.23456,'it''s') AS f",
    "patterns": "SELECT 'AbC' LIKE 'a_c' AS a,'Ä' LIKE 'ä' AS b,'a*b' GLOB 'a[*]b' AS c,'a_b' LIKE 'a!_b' ESCAPE '!' AS d",
    "date time": "SELECT date('2000-02-28','+1 day') AS a,datetime(0,'unixepoch') AS b,strftime('%Y-%m-%d %W','2024-01-01') AS c,julianday('2000-01-01') AS d,timediff('2024-02-01','2024-01-01') AS e",
    "math": "SELECT sqrt(2) AS a,pow(2,10) AS b,log(100) AS c,sin(0) AS d,ceil(-1.5) AS e,sqrt(-1) AS f",
    "blob functions": "SELECT hex(x'00FF80') AS a, length(zeroblob(17)) AS b,hex(unhex('CAFE')) AS c,typeof(x'') AS d,hex(CAST(char(0,255) AS BLOB)) AS e",
    "json": "SELECT json_extract('{\"a\":[1,true,null]}','$.a[1]') AS a,json_type('{\"a\":null}','$.a') AS b,json_patch('{\"x\":1,\"y\":2}','{\"x\":null}') AS c,json(jsonb('[1,2,3]')) AS d; SELECT key,value,type FROM json_each('[1,\"x\",null]')",
    "json aggregates": "WITH t(k,v) AS(VALUES('a',1),('b',2)) SELECT json_group_object(k,v) AS a,json_group_array(v) AS b FROM t",
    "recursive CTE": "WITH RECURSIVE t(n) AS(VALUES(1) UNION ALL SELECT n+1 FROM t WHERE n<100) SELECT sum(n) AS total FROM t",
    "recursive tree": "WITH RECURSIVE t(x) AS(VALUES(1) UNION ALL SELECT x*2 FROM t WHERE x<8 UNION ALL SELECT x*2+1 FROM t WHERE x<8) SELECT x FROM t ORDER BY x",
    "window frames": "WITH t(k,v) AS(VALUES('a',1),('a',2),('b',4),('b',4)) SELECT k,v,row_number() OVER(ORDER BY v) AS rn,rank() OVER(ORDER BY v) AS r,sum(v) OVER(ORDER BY v ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING) AS s,lag(v) OVER(ORDER BY v) AS previous FROM t",
    "window exclusion": "WITH t(v) AS(VALUES(1),(1),(2),(3)) SELECT v,sum(v) OVER(ORDER BY v GROUPS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW EXCLUDE TIES) AS s FROM t",
    "joins": "CREATE TABLE a(x); CREATE TABLE b(x); INSERT INTO a VALUES(1),(2),(NULL); INSERT INTO b VALUES(2),(3); SELECT a.x AS a,b.x AS b FROM a FULL OUTER JOIN b USING(x) ORDER BY 1,2; SELECT * FROM a RIGHT JOIN b USING(x) ORDER BY x",
    "subqueries": "CREATE TABLE t(x); INSERT INTO t VALUES(1),(2),(3),(NULL); SELECT x,(SELECT count(*) FROM t AS b WHERE b.x<a.x) AS n FROM t AS a WHERE EXISTS(SELECT 1 FROM t AS b WHERE b.x=a.x) ORDER BY x DESC LIMIT 2 OFFSET 1",
    "compound": "SELECT 3 AS v UNION SELECT 1 UNION SELECT 3 ORDER BY v; SELECT 1 AS v UNION ALL SELECT 2 EXCEPT SELECT 1; SELECT 2 AS v INTERSECT SELECT 2",
    "group distinct filter": "WITH t(k,v) AS(VALUES('a',1),('a',1),('a',2),('b',NULL)) SELECT k,count(*) AS n,count(DISTINCT v) AS d,sum(v) FILTER(WHERE v>1) AS s,group_concat(v,':') AS g FROM t GROUP BY k HAVING count(*)>0 ORDER BY k",
    "ordered aggregate": "WITH t(x) AS(VALUES(3),(1),(2)) SELECT group_concat(x ORDER BY x DESC) AS a,median(x) AS b,percentile_cont(x,0.25) AS c FROM t",
    "collation": "CREATE TABLE t(x TEXT COLLATE NOCASE UNIQUE); INSERT INTO t VALUES('b'),('A'),('ç'); SELECT x FROM t ORDER BY x; SELECT x FROM t WHERE x='a'",
    "upsert returning": "CREATE TABLE t(id INTEGER PRIMARY KEY,x TEXT UNIQUE,n DEFAULT 5); INSERT INTO t(x) VALUES('a'),('b') RETURNING id,x,n; INSERT INTO t(x,n) VALUES('a',9) ON CONFLICT(x) DO UPDATE SET n=excluded.n RETURNING *; UPDATE t SET n=n+1 WHERE id=2 RETURNING *; DELETE FROM t WHERE id=1 RETURNING *",
    "triggers": "CREATE TABLE t(x); CREATE TABLE log(x); CREATE TRIGGER ai AFTER INSERT ON t BEGIN INSERT INTO log VALUES(new.x*2); END; INSERT INTO t VALUES(3),(4); SELECT * FROM log ORDER BY x",
    "foreign keys": "PRAGMA foreign_keys=ON; CREATE TABLE p(id PRIMARY KEY); CREATE TABLE c(id REFERENCES p ON DELETE CASCADE); INSERT INTO p VALUES(1); INSERT INTO c VALUES(1); DELETE FROM p; SELECT count(*) AS n FROM c; PRAGMA foreign_key_check",
    "generated strict": "CREATE TABLE t(a INTEGER,b TEXT,c INTEGER AS(a*2) STORED) STRICT; INSERT INTO t(a,b) VALUES(3,4); SELECT *,typeof(b) AS ty FROM t; PRAGMA table_xinfo(t)",
    "without rowid": "CREATE TABLE t(a TEXT,b INT,c,PRIMARY KEY(a,b)) WITHOUT ROWID; INSERT INTO t VALUES('a',2,3),('a',1,4); SELECT * FROM t ORDER BY a,b; PRAGMA integrity_check",
    "schema alterations": "CREATE TABLE t(a,b DEFAULT 'x'); INSERT INTO t(a) VALUES(1); ALTER TABLE t ADD COLUMN c INT DEFAULT 7; ALTER TABLE t RENAME COLUMN a TO id; ALTER TABLE t DROP COLUMN b; ALTER TABLE t RENAME TO renamed; SELECT * FROM renamed",
    "savepoint": "CREATE TABLE t(x); BEGIN; INSERT INTO t VALUES(1); SAVEPOINT s; INSERT INTO t VALUES(2); ROLLBACK TO s; RELEASE s; COMMIT; SELECT * FROM t",
    "views": "CREATE TABLE t(x); INSERT INTO t VALUES(1),(2); CREATE VIEW v AS SELECT x*2 AS y FROM t; SELECT * FROM v ORDER BY y",
    "indexes planner": "CREATE TABLE t(x,y); CREATE INDEX expr ON t(lower(y)) WHERE x>0; INSERT INTO t VALUES(1,'Hello'),(2,'World'); EXPLAIN QUERY PLAN SELECT * FROM t WHERE x>0 AND lower(y)='hello'; SELECT * FROM t WHERE x>0 AND lower(y)='hello'; ANALYZE; PRAGMA integrity_check",
    "fts5": "CREATE VIRTUAL TABLE docs USING fts5(body); INSERT INTO docs VALUES('rust database engine'),('database storage'),('rust tools'); SELECT rowid,highlight(docs,0,'[',']') AS h FROM docs WHERE docs MATCH 'rust' ORDER BY rowid; INSERT INTO docs(docs) VALUES('integrity-check')",
    "fts4": "CREATE VIRTUAL TABLE docs USING fts4(body); INSERT INTO docs VALUES('hello rust'),('goodbye rust'); SELECT rowid,offsets(docs) AS off FROM docs WHERE docs MATCH 'hello' ORDER BY rowid",
    "rtree": "CREATE VIRTUAL TABLE r USING rtree(id,x1,x2,y1,y2); INSERT INTO r VALUES(1,0,10,0,10),(2,20,30,20,30); SELECT * FROM r WHERE x1<15 ORDER BY id; SELECT rtreecheck('r') AS ok",
    "geopoly": "CREATE VIRTUAL TABLE g USING geopoly(label); INSERT INTO g(_shape,label) VALUES('[[0,0],[10,0],[10,10],[0,10],[0,0]]','square'); SELECT label,geopoly_area(_shape) AS area FROM g",
    "dbstat": "CREATE TABLE t(x); INSERT INTO t VALUES(1); SELECT name,pagetype,ncell FROM dbstat ORDER BY name,pageno",
    "bytecode": "SELECT opcode FROM bytecode('SELECT 42') ORDER BY addr",
    "attach": "ATTACH ':memory:' AS aux; CREATE TABLE aux.t(x); INSERT INTO aux.t VALUES(1); SELECT * FROM aux.t; DETACH aux",
}

INVALID = ["SELECT FROM", "SELECT missing", "CREATE TABLE t(x,x)", "SELECT abs(-9223372036854775808)",
           "CREATE TABLE t(x UNIQUE); INSERT INTO t VALUES(1),(1)",
           "CREATE TABLE t(x INTEGER) STRICT; INSERT INTO t VALUES('abc')",
           "PRAGMA foreign_keys=ON; CREATE TABLE p(x PRIMARY KEY); CREATE TABLE c(x REFERENCES p); INSERT INTO c VALUES(1)"]

def decoded(text):
    decoder = json.JSONDecoder()
    values = []
    while text.strip():
        value, end = decoder.raw_decode(text.lstrip())
        if value:  # The C shell omits empty result sets; the Rust shell emits [].
            values.append(value)
        text = text.lstrip()[end:]
    return values

def equivalent(a, b):
    if isinstance(a, float) and isinstance(b, (float, int)):
        return math.isclose(a, b, rel_tol=2e-14, abs_tol=1e-14)
    if isinstance(a, dict) and isinstance(b, dict):
        return a.keys() == b.keys() and all(equivalent(a[k], b[k]) for k in a)
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(equivalent(x, y) for x, y in zip(a, b))
    return a == b

def execute(binary, sql, path=":memory:"):
    options = ["-cmd", ".explain off"] if binary.name == "sqlite3-reference" else []
    return subprocess.run([str(binary), "-json", *options, str(path), sql], capture_output=True, text=True, timeout=30)

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile", choices=["debug", "release"], default="release")
    args = parser.parse_args()
    rust = ROOT / f"target/{args.profile}/sqlite-rust"
    reference = ROOT / "build/sqlite3-reference"
    checks = 0
    for name, sql in CASES.items():
        a, b = execute(reference, sql), execute(rust, sql)
        assert a.returncode == b.returncode == 0, (name, a.stderr, b.stderr)
        assert equivalent(decoded(a.stdout), decoded(b.stdout)), (name, a.stdout, b.stdout)
        checks += 1
    for sql in INVALID:
        a, b = execute(reference, sql), execute(rust, sql)
        assert a.returncode == b.returncode == 1, (sql, a.returncode, b.returncode, a.stderr, b.stderr)
        checks += 1
    rng = random.Random(3530400)
    atoms = ["NULL", "0", "1", "-1", "9223372036854775807", "-9223372036854775808", "1.25", "'12'", "'abc'", "''"]
    operations = ["+", "-", "*", "/", "%", "=", "<", "IS", "IS NOT", "AND", "OR", "||", "&", "|", ">>"]
    statements = []
    for _ in range(600):
        expr = f"({rng.choice(atoms)} {rng.choice(operations)} {rng.choice(atoms)})"
        statements.append(f"SELECT quote({expr}) AS value,typeof({expr}) AS type;")
    a, b = execute(reference, "\n".join(statements)), execute(rust, "\n".join(statements))
    assert a.returncode == b.returncode == 0, (a.stderr, b.stderr)
    assert decoded(a.stdout) == decoded(b.stdout), "Seeded expression differential mismatch"
    checks += len(statements)
    with tempfile.TemporaryDirectory(prefix="sqlite-rust-interop-") as tmp:
        for writer, reader in [(reference, rust), (rust, reference)]:
            for encoding in ["UTF-8", "UTF-16le", "UTF-16be"]:
                path = Path(tmp) / f"{writer.name}-{encoding}.db"
                setup = f"PRAGMA encoding='{encoding}'; PRAGMA page_size=512; CREATE TABLE t(id INTEGER PRIMARY KEY,s TEXT,b BLOB); CREATE INDEX ti ON t(s); WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000) INSERT INTO t SELECT x,printf('héllo-%05d',x),zeroblob(1200+x%10) FROM n;"
                made = execute(writer, setup, path)
                assert made.returncode == 0, made.stderr
                sql = "SELECT count(*) AS n,sum(id) AS total,sum(length(b)) AS bytes,min(s) AS first FROM t; PRAGMA integrity_check;"
                a, b = execute(writer, sql, path), execute(reader, sql, path)
                assert a.returncode == b.returncode == 0, (a.stderr, b.stderr)
                assert decoded(a.stdout) == decoded(b.stdout), (encoding, a.stdout, b.stdout)
                edited = execute(reader, "BEGIN; UPDATE t SET s='modified' WHERE id=1; DELETE FROM t WHERE id%2=0; COMMIT; VACUUM;", path)
                assert edited.returncode == 0, edited.stderr
                a, b = execute(writer, sql, path), execute(reader, sql, path)
                assert a.returncode == b.returncode == 0
                assert decoded(a.stdout) == decoded(b.stdout)
                checks += 1
    print(f"PASS: {checks} differential scenarios ({len(CASES)} feature scripts, {len(INVALID)} errors, 600 seeded expressions, 6 bidirectional file/encoding cases)")

if __name__ == "__main__":
    main()
