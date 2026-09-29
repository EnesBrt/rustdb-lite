#!/usr/bin/env python3
"""Validate safe storage against the pinned C SQLite and native WAL snapshots."""
import json
import shutil
import sqlite3
import struct
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ORACLE = ROOT.parent / "legacy/build/sqlite3-reference"
INSPECT = ROOT.parent / "target/debug/sqlite-safe-inspect"
CREATE = ROOT.parent / "target/debug/examples/create_fixture"

def sql(path, source):
    result = subprocess.run([str(ORACLE), "-json", str(path)], input=source, text=True, capture_output=True, timeout=30)
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout) if result.stdout.strip() else []

def inspect(path, command, *args, ok=True):
    result = subprocess.run([str(INSPECT), str(path), command, *map(str,args)], text=True, capture_output=True, timeout=30)
    if not ok:
        assert result.returncode == 1, (result.stdout,result.stderr)
        return []
    assert result.returncode == 0, result.stderr
    return [json.loads(line) for line in result.stdout.splitlines()]

def value(value, blob=False):
    if value is None: return {"type":"null"}
    if blob: return {"type":"blob","hex":value.upper()}
    if isinstance(value,int): return {"type":"integer","value":value}
    if isinstance(value,float): return {"type":"real","bits":struct.pack(">d",value).hex().upper()}
    return {"type":"text","value":value}

def main():
    subprocess.run(["cargo", "+stable", "build", "--manifest-path", str(ROOT / "Cargo.toml"), "--bins", "--examples"], check=True)
    assert ORACLE.exists(), "Run the legacy/scripts/build_oracle.py first"
    scenarios = 0
    with tempfile.TemporaryDirectory(prefix="sqlite-safe-") as tmp:
        tmp=Path(tmp)
        for size in [512,1024,2048,4096,8192,16384,32768,65536]:
            path=tmp/f"written-{size}.db"
            subprocess.run([str(CREATE),str(path),str(size)],check=True)
            assert sql(path,"PRAGMA integrity_check;")==[{"integrity_check":"ok"}]
            assert sql(path,"SELECT count(*) n,sum(number) total,sum(length(bytes)) bytes FROM items;")==[{"n":2000,"total":1999000,"bytes":1195056}]
            result=sql(path,'SELECT rowid,number,hex(text) text_hex,hex(bytes) blob,float,"nothing" FROM items ORDER BY rowid;')
            schema=inspect(path,"schema")
            root=next(entry["root_page"] for entry in schema if entry["name"]=="items")
            expected=[{"rowid":row["rowid"],"values":[value(row["number"]),value(bytes.fromhex(row["text_hex"]).decode("utf-8")),value(row["blob"],True),value(row["float"]),value(row["nothing"])]} for row in result]
            assert inspect(path,"rows",root)==expected, ("Rust/C writer mismatch",size)
            assert len(schema)==151
            scenarios+=1

        for size in [512,4096,65536]:
            for encoding in ["UTF-8","UTF-16le","UTF-16be"]:
                for auto in [0,1]:
                    path=tmp/f"read-{size}-{encoding}-{auto}.db"
                    sql(path,f"""PRAGMA page_size={size}; PRAGMA encoding='{encoding}'; PRAGMA auto_vacuum={auto};
                    CREATE TABLE t(n,t,b,r,z); CREATE INDEX ix ON t(t,n);
                    WITH RECURSIVE s(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM s WHERE x<1200)
                    INSERT INTO t SELECT x,printf('héllo-%06d',x)||char(0)||'🦀',zeroblob(CASE WHEN x%29=0 THEN 9000 ELSE 5 END),x+0.25,NULL FROM s;
                    DELETE FROM t WHERE n%7=0;
                    CREATE TABLE wr(k TEXT,n,b BLOB,PRIMARY KEY(k,n)) WITHOUT ROWID;
                    INSERT INTO wr SELECT t,n,b FROM t;
                    """)
                    schema=inspect(path,"schema")
                    roots={s["name"]:s["root_page"] for s in schema}
                    actual=inspect(path,"rows",roots["t"])
                    expected=[{"rowid":row["rowid"],"values":[value(row["n"]),value(bytes.fromhex(row["text_hex"]).decode(encoding)),value(row["blob"],True),value(row["r"]),value(row["z"])]} for row in sql(path,"SELECT rowid,n,hex(t) text_hex,hex(b) blob,r,z FROM t ORDER BY rowid;")]
                    assert actual==expected,("table",size,encoding,auto)
                    actual=inspect(path,"rows",roots["wr"])
                    expected=[{"rowid":None,"values":[value(bytes.fromhex(row["text_hex"]).decode(encoding)),value(row["n"]),value(row["blob"],True)]} for row in sql(path,"SELECT hex(k) text_hex,n,hex(b) blob FROM wr ORDER BY k,n;")]
                    assert actual==expected,("WITHOUT ROWID",size,encoding,auto)
                    actual=inspect(path,"rows",roots["ix"])
                    expected=[{"rowid":None,"values":[value(bytes.fromhex(row["text_hex"]).decode(encoding)),value(row["n"]),value(row["rowid"])]} for row in sql(path,"SELECT hex(t) text_hex,n,rowid FROM t INDEXED BY ix ORDER BY t,n,rowid;")]
                    assert actual==expected,("index",size,encoding,auto)
                    scenarios+=1

        live=tmp/"live.db"
        connection=sqlite3.connect(live,isolation_level=None)
        try:
            connection.execute("PRAGMA journal_mode=WAL")
            connection.execute("PRAGMA wal_autocheckpoint=0")
            connection.execute("CREATE TABLE t(x)")
            connection.execute("BEGIN")
            connection.executemany("INSERT INTO t VALUES(?)",[(n,) for n in range(10)])
            connection.execute("COMMIT")
            connection.execute("INSERT INTO t VALUES(99)")
            snapshot=tmp/"snapshot.db"
            shutil.copyfile(live,snapshot)
            wal=Path(str(live)+"-wal").read_bytes()
            size=int.from_bytes(wal[8:12],"big")
            frame=size+24
            variants=[("full",wal,11),("torn",wal[:-1],10),("missing",wal[:-frame],10),
                ("garbage",wal+b"incomplete frame",11)]
            bad=bytearray(wal); bad[-size-8]^=1
            variants.append(("bad-checksum",bytes(bad),10))
            for name,data,count in variants:
                walpath=tmp/f"{name}.wal";walpath.write_bytes(data)
                schema=inspect(snapshot,"schema","--wal",walpath)
                root=next(s["root_page"] for s in schema if s["name"]=="t")
                rows=inspect(snapshot,"rows",root,"--wal",walpath)
                assert len(rows)==count,(name,len(rows))
                assert [row["values"][0]["value"] for row in rows]==list(range(10))+([99] if count==11 else [])
                scenarios+=1
            inspect(live,"schema",ok=False) # Avoid silently inspecting a live WAL main file.
            scenarios+=1
        finally: connection.close()
    print(f"PASS: {scenarios} safe-storage differential scenarios; 8 SQLite-validated writer page sizes; 18 reader cases with table/index/WITHOUT ROWID, encodings, auto-vacuum and overflow; 6 WAL/sidecar cases")
    print(f"WAL producer: Python's native SQLite {sqlite3.sqlite_version}; main oracle: SQLite 3.53.4")

if __name__=="__main__":main()
