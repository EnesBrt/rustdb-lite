#!/usr/bin/env python3
"""Compare the implemented SQL surface to pinned SQLite 3.53.4, byte-exactly.

Native SQLite runs only as a separate test oracle. SQL queries use stable aliases
so the wrapper can record runtime types and exact IEEE-754/text/BLOB bytes.
"""
import json
from pathlib import Path
import random
import math
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
ORACLE = ROOT / 'legacy/build/sqlite3-reference'
ENGINE = ROOT / 'target/debug/sqlite-safe-sql'


def run_engine(script, *args, fail=False):
    result = subprocess.run([str(ENGINE), *map(str,args)], input=script, text=True, capture_output=True, timeout=30)
    if fail:
        assert result.returncode != 0, ('expected error',script,result.stdout)
        return
    assert result.returncode == 0, (script,result.returncode,result.stderr)
    return [json.loads(line) for line in result.stdout.splitlines()]


def oracle_query(setup, query, n):
    fields = []
    for i in range(n):
        col=f'c{i}'
        fields += [f'typeof({col}) AS t{i}', f"CASE typeof({col}) WHEN 'text' THEN hex({col}) WHEN 'blob' THEN hex({col}) WHEN 'real' THEN hex(ieee754_to_blob({col})) ELSE {col} END AS v{i}"]
    wrapped=f'SELECT {",".join(fields)} FROM ({query});'
    result=subprocess.run([str(ORACLE),'-json',':memory:'],input=setup+'\n'+wrapped,text=True,capture_output=True,timeout=30)
    assert result.returncode == 0,(setup,query,result.stderr)
    raw=json.loads(result.stdout) if result.stdout.strip() else []
    rows=[]
    for row in raw:
        values=[]
        for i in range(n):
            kind=row[f't{i}'];value=row[f'v{i}'];item={'type':kind}
            if kind=='integer':item['value']=value
            elif kind=='real':item['bits']=value
            elif kind in ('text','blob'):item['hex']=value
            values.append(item)
        rows.append(values)
    return rows


def check(setup,query,n):
    actual=run_engine(setup+'\n'+query+';')[-1]['rows']
    expected=oracle_query(setup,query,n)
    assert actual==expected, {'query':query,'setup':setup,'rust':actual,'sqlite':expected}


def select(expressions):
    return 'SELECT '+','.join(f'({expr}) AS c{i}' for i,expr in enumerate(expressions))


def main():
    subprocess.run(['cargo','build','--bin','sqlite-safe-sql'],cwd=ROOT,check=True)
    cases=0
    expressions=[
        '2+3*4','7/2','7/2.0','-7/2','1/0','1%0','-9223372036854775808/-1',
        '9223372036854775807+1','9223372036854775807*2','0xffffffffffffffff',
        '0x8000000000000000','-(9223372036854775808)','-+9223372036854775808','-(9223372036854775808 COLLATE BINARY)','1_000+2','1.2_5e1_0','1<<65','-1>>65','1<<-2','8>>-2',
        "'123e5'+0","CAST('123e5' AS INTEGER)","CAST('123e5' AS NUMERIC)",
        "CAST('9007199254740993' AS NUMERIC)","CAST('123abc' AS REAL)",
        "CAST('1.5abc' AS NUMERIC)","CAST('  -12xy' AS INTEGER)","CAST('0x10' AS INTEGER)",
        'NULL AND 0','NULL AND 1','NULL OR 1','NULL OR 0','NOT NULL',
        '2 IS TRUE','2 IS FALSE','NULL IS FALSE','2 IS NOT TRUE','2 IS 1','NULL IS NULL',
        'NULL IS DISTINCT FROM 1','NULL IS NOT DISTINCT FROM NULL','1 ISNULL','NULL NOTNULL',
        '2 BETWEEN 1 AND 3','2 NOT BETWEEN 1 AND 3','NULL IN ()','NULL NOT IN ()',
        '2 IN (1,NULL)','1 IN (1,NULL)','2 NOT IN (1,NULL)','2 NOT IN (1,3)',
        "1='1'","CAST(1 AS TEXT)='1'","'a'='A' COLLATE NOCASE","'a '= 'a' COLLATE RTRIM",
        '9223372036854775807>9223372036854775806.0','9007199254740993>9007199254740992.0',
        "CASE 2 WHEN 1 THEN 'a' WHEN 2 THEN 'b' ELSE 'c' END",
        'CASE WHEN 1 THEN 7 ELSE abs(-9223372036854775808) END',
        'coalesce(NULL,3,abs(-9223372036854775808))','iif(0,1,0,2,3)',
        "typeof('1'+0)","typeof('1e0'+0)","CAST(1.1 AS TEXT)","CAST(1.2345678901234567 AS TEXT)",
        "CAST(1e16 AS TEXT)","CAST(1e17 AS TEXT)","CAST(1e-5 AS TEXT)","CAST(-0.0 AS TEXT)",
        "lower('ÄBC')","upper('éabc')","hex(x'00ff')","unhex('01-FF','-')", "unhex('xyz')",
        "length('é🦀')","length('x'||char(0)||'y')","octet_length('x'||char(0)||'y')",
        "instr('é🦀a','a')","instr(x'00ff00',x'ff')","instr('abc','')",
        "replace('ababa','a','xy')","trim('abcxy','xy')","'Abc' LIKE 'a%'","'æ' LIKE 'Æ'",
        "like('a!_%','a_xyz','!')","nullif('A' COLLATE NOCASE,'a')","min(4,2,3)","max('a','B' COLLATE NOCASE)",
        'min(1,NULL)','char(65,0,128)', "CAST('9007199254740992.0' AS NUMERIC)",
    ]
    for start in range(0,len(expressions),12):
        batch=expressions[start:start+12];check('',select(batch),len(batch));cases+=len(batch)
    float_rng=random.Random(173534)
    for _ in range(15):
        exprs=[]
        while len(exprs)<12:
            n=struct.unpack('>d',float_rng.getrandbits(64).to_bytes(8,'big'))[0]
            if math.isfinite(n):exprs.append(f'CAST({repr(n)} AS TEXT)')
        check('',select(exprs),len(exprs));cases+=len(exprs)
    for start in range(-8,9):
        exprs=[f"substr('abc🦀', {start}, {n})" for n in range(-7,8)]
        check('',select(exprs),len(exprs));cases+=len(exprs)
    rng=random.Random(75304)
    literals=['NULL','0','1','-1','2.5',"'12'","'2.5x'","'bad'","x'3132'",'9223372036854775807','-9223372036854775808']
    ops=['+','-','*','/','%','&','|','<<','>>','=','!=','<','<=','>','>=','IS','IS NOT','AND','OR','||']
    for _ in range(40):
        exprs=[f'({rng.choice(literals)}) {rng.choice(ops)} ({rng.choice(literals)})' for _ in range(10)]
        check('',select(exprs),len(exprs));cases+=len(exprs)
    setup="""CREATE TABLE t(id INTEGER PRIMARY KEY,txt TEXT COLLATE NOCASE,n NUMERIC,r REAL,b BLOB);
    INSERT INTO t VALUES(1,'B','10',2,x'00'),(2,'a','20.5',3.5,x'FF'),(3,'A',NULL,NULL,NULL),(4,'c',-5,1.25,x'00FF');"""
    queries=[
        ('SELECT id c0,txt c1,n c2,r c3,b c4 FROM t ORDER BY id',5),
        ("SELECT id c0 FROM t WHERE n>'9' ORDER BY id",1),
        ("SELECT id c0 FROM t WHERE txt='A' ORDER BY id",1),
        ('SELECT DISTINCT txt c0 FROM t ORDER BY 1 DESC',1),
        ('SELECT txt c0,count(*) c1,sum(n) c2,avg(r) c3,total(n) c4 FROM t GROUP BY txt ORDER BY txt',5),
        ('SELECT min(n) c0, max(n) c1, count(n) c2,count(DISTINCT txt) c3,group_concat(txt) c4 FROM t',5),
        ('SELECT id c0,max(n) c1 FROM t',2),
        ('SELECT id c0,n c1 FROM t ORDER BY n DESC NULLS FIRST LIMIT 2 OFFSET 1',2),
        ('SELECT sum(n) c0,total(n) c1,count(*) c2,group_concat(txt) c3 FROM t WHERE 0',4),
        ('SELECT a.txt c0,b.id c1 FROM t a LEFT JOIN t b ON a.id=b.id+1 WHERE a.id>1 ORDER BY a.id',2),
    ]
    for query,n in queries:check(setup,query,n);cases+=1
    check('CREATE TABLE t(r REAL,n); INSERT INTO t VALUES(-0.0,-0.0);','SELECT r c0,n c1 FROM t',2);cases+=1
    check('CREATE TABLE t(id INTEGER PRIMARY KEY DEFAULT 10,x DEFAULT(abs(-9223372036854775808)));INSERT INTO t(x) VALUES(last_insert_rowid()),(last_insert_rowid());','SELECT id c0,x c1 FROM t ORDER BY id',2);cases+=1
    check(setup,'SELECT -id AS c0 FROM t ORDER BY c0',1);cases+=1
    for values in ['(1.0),(2.0)', "('1.0'),('2e0')", '(1),(2),(3)','(9007199254740993),(1),(-9007199254740992)','(9223372036854775807),(1),(0.0)',"('1'),('2'),('x')"]:
        check('CREATE TABLE t(x);INSERT INTO t VALUES'+values+';', 'SELECT sum(x) c0,total(x) c1,avg(x) c2 FROM t',3);cases+=1
    check(setup+' UPDATE t SET n=n+1,txt=upper(txt) WHERE id>=2; DELETE FROM t WHERE id=4;', 'SELECT id c0,txt c1,n c2 FROM t ORDER BY id',3);cases+=1
    check(setup+' BEGIN; INSERT INTO t(txt) VALUES(\'new\'); SAVEPOINT s; DELETE FROM t; ROLLBACK TO s; RELEASE s; COMMIT;', 'SELECT id c0,txt c1 FROM t ORDER BY id',2);cases+=1
    for script in ["SELECT unknown(1);","SELECT 1__2;","SELECT abs(-9223372036854775808);",'CREATE TABLE t(x UNIQUE);INSERT INTO t VALUES(1),(1);','SELECT ?0;', 'CREATE TABLE t(x);SELECT nope FROM t;']:
        run_engine(script,fail=True)
        rejected=subprocess.run([str(ORACLE),':memory:'],input=script,text=True,capture_output=True,timeout=30)
        assert rejected.returncode!=0,('reference accepted supposed error',script,rejected.stdout)
        cases+=1
    with tempfile.TemporaryDirectory(prefix='safe-sql-') as directory:
        path=Path(directory)/'created.db'
        script="CREATE TABLE t(id INTEGER PRIMARY KEY,name TEXT NOT NULL,n REAL DEFAULT 2.5 CHECK(n>0)); INSERT INTO t(name) VALUES('one'),('🦀'); PRAGMA user_version=42;"
        run_engine(script,'--save',path)
        result=subprocess.run([str(ORACLE),'-json',str(path)],input='PRAGMA integrity_check; SELECT id,name,n FROM t ORDER BY id; PRAGMA user_version;',text=True,capture_output=True,check=True)
        decoder=json.JSONDecoder(); outputs=[]; remaining=result.stdout.strip()
        while remaining:
            item,end=decoder.raw_decode(remaining);outputs.append(item);remaining=remaining[end:].lstrip()
        assert outputs==[[{'integrity_check':'ok'}],[{'id':1,'name':'one','n':2.5},{'id':2,'name':'🦀','n':2.5}],[{'user_version':42}]]
        loaded=run_engine('SELECT id,name,n FROM t ORDER BY id;','--load',path)[-1]['rows']
        assert loaded==run_engine(script+'SELECT id,name,n FROM t ORDER BY id;')[-1]['rows']
        run_engine('SELECT 1;','--save',path,fail=True)
        cases+=3
    print(f'PASS: {cases} SQL expression/query/error/snapshot cases against SQLite 3.53.4')

if __name__=='__main__':main()
