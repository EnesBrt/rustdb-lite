#!/usr/bin/env python3
"""Exercise native locking, WAL sharing, and hot-journal recovery across engines."""
import json
import select
import subprocess
import sys
import tempfile
from pathlib import Path
from config import ROOT

class Worker:
    def __init__(self, library, database):
        self.process = subprocess.Popen([sys.executable, str(ROOT / "scripts/ffi_worker.py"), str(library), str(database)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        assert self.read() == {"open": 0}
    def read(self):
        assert select.select([self.process.stdout], [], [], 20)[0], "worker timed out"
        line = self.process.stdout.readline()
        assert line, f"worker exited: {self.process.stderr.read()}"
        return json.loads(line)
    def sql(self, sql, code=0):
        self.process.stdin.write(json.dumps({"sql": sql}) + "\n")
        self.process.stdin.flush()
        result = self.read()
        assert result["rc"] == code, (sql, result)
        return result["rows"]
    def crash(self):
        self.process.stdin.write('{"crash":true}\n')
        self.process.stdin.flush()
        assert self.process.wait(timeout=10) == 0
    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()
            self.process.wait(timeout=10)
        assert self.process.returncode == 0, self.process.stderr.read()
    def __enter__(self): return self
    def __exit__(self, *_): self.close()

def main():
    reference = ROOT / "build/libsqlite3_reference.dylib"
    rust = ROOT / "target/release/libsqlite_rust.dylib"
    checks = 0
    with tempfile.TemporaryDirectory(prefix="sqlite-rust-process-") as tmp:
        for direction, (first, second) in enumerate([(reference, rust), (rust, reference)]):
            for mode in ["DELETE", "WAL"]:
                path = Path(tmp) / f"shared-{direction}-{mode}.db"
                with Worker(first, path) as a, Worker(second, path) as b:
                    a.sql(f"PRAGMA journal_mode={mode}; CREATE TABLE t(x); INSERT INTO t VALUES(1)")
                    a.sql("BEGIN IMMEDIATE; INSERT INTO t VALUES(2)")
                    assert b.sql("SELECT count(*) FROM t") == [["1"]]
                    b.sql("BEGIN IMMEDIATE", code=5)
                    a.sql("COMMIT")
                    assert b.sql("SELECT count(*) FROM t") == [["2"]]
                    if mode == "WAL":
                        b.sql("BEGIN")
                        assert b.sql("SELECT count(*) FROM t") == [["2"]]
                        a.sql("INSERT INTO t VALUES(3)")
                        assert b.sql("SELECT count(*) FROM t") == [["2"]]
                        b.sql("COMMIT")
                        assert b.sql("SELECT count(*) FROM t") == [["3"]]
                    b.sql("BEGIN IMMEDIATE; INSERT INTO t VALUES(4)")
                    a.sql("BEGIN IMMEDIATE", code=5)
                    b.sql("COMMIT")
                    assert a.sql("PRAGMA integrity_check") == [["ok"]]
                checks += 1

                path = Path(tmp) / f"crash-{direction}-{mode}.db"
                with Worker(first, path) as writer:
                    writer.sql(f"PRAGMA journal_mode={mode}; PRAGMA synchronous=FULL; PRAGMA cache_size=5; CREATE TABLE t(id INTEGER PRIMARY KEY,payload BLOB); WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100) INSERT INTO t SELECT x,zeroblob(4096) FROM n;")
                    writer.sql("BEGIN IMMEDIATE; UPDATE t SET payload=zeroblob(6000); INSERT INTO t VALUES(101,zeroblob(6000))")
                    artifact = Path(str(path) + ("-journal" if mode == "DELETE" else "-wal"))
                    assert artifact.exists() and artifact.stat().st_size > 512, "test did not spill to disk"
                    writer.crash()
                with Worker(second, path) as reader:
                    assert reader.sql("SELECT count(*),sum(length(payload)) FROM t") == [["100", "409600"]]
                    assert reader.sql("PRAGMA integrity_check") == [["ok"]]
                checks += 1
    print(f"PASS: {checks} bidirectional, separate-process rollback/WAL locking and abrupt-exit recovery scenarios")

if __name__ == "__main__": main()
