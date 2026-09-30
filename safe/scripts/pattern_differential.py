#!/usr/bin/env python3
"""LIKE/ESCAPE and GLOB compared with the independent pinned native engine."""
import itertools
import random
import subprocess
import tempfile
from pathlib import Path
from conflict_differential import reference_library, compare
from index_differential import native
from sql_differential import ROOT, run_engine


def quoted(text):
    if '\0' in text:
        return f"CAST(x'{text.encode().hex()}' AS TEXT)"
    return "'" + text.replace("'", "''") + "'"


def main():
    subprocess.run(['cargo', 'build', '--example', 'statement_probe', '--bin', 'sqlite-safe-sql'], cwd=ROOT, check=True)
    lib = reference_library()
    cases = 0
    pending = []

    def check(sql):
        nonlocal cases
        pending.append(sql)
        cases += 1
        if len(pending) == 100:
            compare(lib, pending)
            pending.clear()

    texts = ['', 'a', 'A', 'ab', 'abc', 'a_b', 'a%b', 'a*b', '?', 'b', 'c', 'z', '-', ']', '[', '^', 'é', 'É', '🦀', 'Rust', 'a\0b', '\0', 'a!b', 'a\\b', '123']
    globs = ['', '*', '?', 'a*', '*b', '*a*b', 'a?', '*?', '?*?', '[a-c]', '[^a-c]', '[z-a]', '[-a]', '[a-]', '[]]', '[^]]', '[]a]', '[[]', '[!a]', '[a-b-c]', '[a--c]', '[]', '[^]', '[', '[a', '*[a-c]*', '*[^a]*b', '[é-ê]', '[Éé]', '[🦀-🦁]', '**a**', 'a\\b', '*a?*b', '*\0z', 'a\0b', '*[', 'a[*]b']
    for pattern, text in itertools.product(globs, texts):
        p, t = quoted(pattern), quoted(text)
        check(f'SELECT {t} GLOB {p}, {t} NOT GLOB {p}, glob({p},{t})')
    likes = ['', '%', '_', 'a%', '%b', '%a%b', 'a_', '%_', '_%_', 'a!_b', 'a!%b', 'a!!b', 'a!', '%%', '__', '%é%', '%🦀%', '%!_%', '%!%%', 'a\0b', '%\0z']
    escapes = [None, '!', '%', '_', 'é', '🦀', '\0', '!\0x', '', 'xx']
    for pattern, text, escape in itertools.product(likes, texts, escapes):
        p, t = quoted(pattern), quoted(text)
        suffix = '' if escape is None else ' ESCAPE ' + quoted(escape)
        arg = '' if escape is None else ',' + quoted(escape)
        check(f'SELECT {t} LIKE {p}{suffix}, {t} NOT LIKE {p}{suffix}, like({p},{t}{arg})')
    values = ['NULL', '1', '-1', '1.5', "x'61'", "x'610062'", "'1%'", "'*'"]
    for function, a, b in itertools.product(['like', 'glob'], values, values):
        check(f'SELECT {function}({a},{b})')
    raw = ['00', '61', '80', '81', 'BF', 'C0', 'C080', 'C181', 'C2A0', 'C280', 'E0A080', 'EDA080', 'EFBFBD', 'EFBFBE', 'EFBFBF', 'F0908080', 'F4908080', 'F880808080', 'FF', 'FF8080808080808080']
    for a, b, name in itertools.product(raw, raw, ['like', 'glob']):
        check(f"SELECT {name}(x'{a}',x'{b}'),{name}(CAST(x'{a}' AS TEXT),CAST(x'{b}' AS TEXT))")
    rng = random.Random(0x51A7E)
    alphabet = 'abAB*?[]^-_%!é🦀\0'
    for _ in range(1800):
        p = quoted(''.join(rng.choices(alphabet, k=rng.randrange(18))))
        t = quoted(''.join(rng.choices(alphabet, k=rng.randrange(12))))
        e = quoted(rng.choice(['!', '%', '_', 'é', '🦀']))
        check(f'SELECT glob({p},{t}),like({p},{t}),like({p},{t},{e})')
    raw_alphabet = b'abAB*?[]^-_%!' + bytes([0,0x80,0xBF,0xC0,0xC1,0xE0,0xEF,0xFF])
    for _ in range(600):
        p = bytes(rng.choices(raw_alphabet, k=rng.randrange(20))).hex()
        t = bytes(rng.choices(raw_alphabet, k=rng.randrange(12))).hex()
        check(f"SELECT glob(x'{p}',x'{t}'),like(x'{p}',x'{t}'),like(x'{p}',x'{t}','!')")
    for name in ['like', 'glob']:
        for length in [49_999, 50_000, 50_001]:
            for text in ["''", 'NULL']:
                check(f'SELECT {name}({quoted("a" * length)},{text})')
    for sql in [
        "SELECT NULL LIKE 'a' ESCAPE '', NULL LIKE 'a' ESCAPE 'xx'",
        "SELECT 'a' LIKE NULL ESCAPE '', 'a' LIKE NULL ESCAPE 'xx'",
        "SELECT 'a' LIKE 'a' ESCAPE NULL, NULL LIKE NULL ESCAPE NULL",
        "SELECT 'a' GLOB 'a' ESCAPE '!'", "SELECT ('a' LIKE 'a') ESCAPE '!'",
        "SELECT 'a' LIKE 'a' ESCAPE '!'='x'", "SELECT 'a' LIKE 'a' ESCAPE '!' < 'z'",
        "SELECT 'a' LIKE 'a' < 'z' ESCAPE '!'", "SELECT 'a' LIKE 'a' ESCAPE '!'||''",
        "SELECT NOT 'a' GLOB 'a*', 'a' NOT GLOB 'a*'=0, 1 OR 'a' LIKE 'a' ESCAPE '!'",
        "SELECT CASE WHEN 1 THEN 1 ELSE 'a' LIKE 'a' ESCAPE '' END",
        "SELECT glob(),glob('a'),glob('a','b','c')",
        "SELECT 'a' LIKE 'a' ESCAPE abs(-9223372036854775808)",
        "SELECT glob('a' ORDER BY 1,'a')", "SELECT glob('a','a') FILTER (WHERE 1)",
        "SELECT (1,2) GLOB '*', 'a' LIKE (1,2) ESCAPE '!'",
        "SELECT 'a' LIKE 'a' ESCAPE (1,2)",
    ]:
        check(sql)
    compare(lib, pending)
    for schema in ['', ' WITHOUT ROWID', ' STRICT']:
        statements = [f"CREATE TABLE t(id INT PRIMARY KEY,name TEXT,literal INT AS(name LIKE '%!_%' ESCAPE '!') STORED){schema}",
            "INSERT INTO t(id,name) VALUES(1,'A_one'),(2,'a_two'),(3,'B_three'),(4,'é_été'),(5,NULL)",
            "CREATE VIEW v AS SELECT id,name FROM t WHERE name GLOB '[A-Z]*'",
            "CREATE UNIQUE INDEX ix ON t((name LIKE 'A!_%' ESCAPE '!')) WHERE name GLOB 'A*'",
            "SELECT id,name,literal FROM t ORDER BY id", "SELECT * FROM v ORDER BY id",
            "INSERT INTO t(id,name) VALUES(6,'A_four') ON CONFLICT(like('A!_%',name,'!')) WHERE glob('A*',name) DO UPDATE SET name=excluded.name RETURNING *",
            "BEGIN", "UPDATE t SET name=upper(name) WHERE name LIKE '%!_%' ESCAPE '!' RETURNING *", "ROLLBACK",
            "UPDATE t SET name='bad' WHERE name LIKE '%' ESCAPE ''", "SELECT * FROM t ORDER BY id",
            "CREATE TABLE copies AS SELECT name GLOB '*' AS g FROM t UNION ALL SELECT CAST(1 AS INT)", "PRAGMA table_info(copies)"]
        compare(lib, statements)
        cases += 1
    with tempfile.TemporaryDirectory(prefix='safe-pattern-') as folder:
        folder = Path(folder)
        for n, (size, encoding, mode) in enumerate(itertools.product([512,1024,2048,4096,8192,16384,32768,65536], ['UTF-8','UTF-16le','UTF-16be'], [0,1,2])):
            source, saved, final = [folder / f'{n}-{part}.db' for part in ['source','saved','final']]
            native(source, f"PRAGMA page_size={size};PRAGMA encoding='{encoding}';PRAGMA auto_vacuum={mode};CREATE TABLE t(id INT PRIMARY KEY,name TEXT,literal INT AS(name LIKE '%!_%' ESCAPE '!') STORED);INSERT INTO t(id,name) VALUES(1,'A_one'),(2,'a_two'),(3,'É_été'),(4,'🦀'),(5,NULL);CREATE VIEW v AS SELECT * FROM t WHERE name GLOB '[A-ZÉ]*';CREATE INDEX ix ON t((name LIKE 'A!_%' ESCAPE '!')) WHERE name GLOB '[A-Z]*';")
            blob_exprs = ["glob(x'61',x'61')", "like('A',x'61')", "like('A',x'6100')", "like('é',x'C3A9')", "glob('*',x'FF')", "glob('?',x'00D8')", "glob('?',x'00D86100')"]
            query = 'SELECT ' + ','.join(f'{e} AS c{i}' for i,e in enumerate(blob_exprs))
            expected = native(source,query)[0]
            actual = run_engine(query, '--load', source)[0]['rows'][0]
            assert [v['value'] for v in actual] == [expected[f'c{i}'] for i in range(len(actual))], (encoding,actual,expected)
            sql = "CREATE TABLE copied AS SELECT * FROM v;UPDATE t SET name=lower(name) WHERE name GLOB '[A-Z]*';"
            run_engine(sql, '--load', source, '--save', saved)
            native(source, sql)
            for q in ['SELECT * FROM t ORDER BY id','SELECT * FROM v ORDER BY id','SELECT * FROM copied ORDER BY id','PRAGMA integrity_check']:
                assert native(saved,q) == native(source,q), (n,q)
            native(saved,"INSERT INTO t(id,name) VALUES(6,'R_more')")
            sql = "UPDATE t SET name='B_changed' WHERE name LIKE 'R!_%' ESCAPE '!';"
            run_engine(sql, '--load', saved, '--save', final)
            native(saved,sql)
            assert native(final,'SELECT * FROM t ORDER BY id') == native(saved,'SELECT * FROM t ORDER BY id')
            assert native(final,'PRAGMA integrity_check') == [{'integrity_check':'ok'}]
            cases += 1
    print(f'PASS: {cases} LIKE/ESCAPE/GLOB value/error/schema/image scenarios against SQLite 3.53.4')

if __name__ == '__main__':
    main()
