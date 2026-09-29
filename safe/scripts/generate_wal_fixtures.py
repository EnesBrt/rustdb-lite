#!/usr/bin/env python3
"""Regenerate small, externally validated WAL fixtures; native SQLite is test-only.

Run intentionally when changing fixtures, not as part of normal Rust tests.
SQLite generates random WAL salts, so regeneration changes the captured hashes.
"""
import hashlib
import json
from pathlib import Path
import sqlite3
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
ORACLE = ROOT.parent / "legacy/build/sqlite3-reference"


def rechecksum(wal, *, big_endian=False, uncommitted_tail=False):
    """Transform native bytes, recalculating the documented rolling checksum."""
    data = bytearray(wal)
    struct.pack_into(">I", data, 0, 0x377F0683 if big_endian else 0x377F0682)
    page_size = struct.unpack_from(">I", data, 8)[0]
    frame_size = 24 + page_size
    if uncommitted_tail:
        struct.pack_into(">I", data, len(data) - frame_size + 4, 0)
    a = b = 0

    def accumulate(block):
        nonlocal a, b
        for x, y in struct.iter_unpack(">II" if big_endian else "<II", block):
            a = (a + x + b) & 0xFFFFFFFF
            b = (b + y + a) & 0xFFFFFFFF
        return struct.pack(">II", a, b)

    data[24:32] = accumulate(data[:24])
    for at in range(32, len(data), frame_size):
        accumulate(data[at:at + 8])
        data[at + 16:at + 24] = accumulate(data[at + 24:at + frame_size])
    return bytes(data)


def main():
    assert ORACLE.exists(), "Build the pinned reference with legacy/scripts/build_oracle.py"
    out = ROOT / "tests/fixtures"
    out.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="sqlite-wal-fixture-") as directory:
        directory = Path(directory)
        path = directory / "fixture.db"
        db = sqlite3.connect(path, isolation_level=None)
        try:
            db.execute("PRAGMA page_size=512")
            db.execute("PRAGMA journal_mode=WAL")
            db.execute("PRAGMA wal_autocheckpoint=0")
            db.execute("CREATE TABLE t(x)")
            db.execute("BEGIN")
            db.executemany("INSERT INTO t VALUES(?)", [(n,) for n in range(10)])
            db.execute("COMMIT")
            db.execute("INSERT INTO t VALUES(99)")
            base = path.read_bytes()
            wal = Path(str(path) + "-wal").read_bytes()
        finally:
            db.close()
        snapshots = {
            "wal-base.bin": base,
            "wal-commits.bin": wal,
            "wal-commits-be.bin": rechecksum(wal, big_endian=True),
            "wal-uncommitted.bin": rechecksum(wal, uncommitted_tail=True),
        }
        # Let the pinned reference engine independently open each transformed
        # snapshot. This checks that the fixtures, including their checksums and
        # commit semantics, are accepted by SQLite itself.
        for name in ["wal-commits.bin", "wal-commits-be.bin", "wal-uncommitted.bin"]:
            check = directory / (name + ".db")
            check.write_bytes(base)
            Path(str(check) + "-wal").write_bytes(snapshots[name])
            result = subprocess.run(
                [str(ORACLE), "-json", str(check)],
                input="PRAGMA integrity_check; SELECT count(*) n,sum(x) total FROM t;",
                text=True, capture_output=True, check=True, timeout=30,
            )
            rows = [json.loads(line) for line in result.stdout.splitlines()]
            expected = {"n": 10, "total": 45} if "uncommitted" in name else {"n": 11, "total": 144}
            assert rows == [[{"integrity_check": "ok"}], [expected]], (name, rows)
        for name, data in snapshots.items():
            (out / name).write_bytes(data)
        metadata = {
            "producer": "Python sqlite3 linked to SQLite " + sqlite3.sqlite_version,
            "verified_with": subprocess.check_output([str(ORACLE), "-version"], text=True).strip(),
            "page_size": 512,
            "table_root_page": 2,
            "frame_count": 4,
            "commit_row_counts": [None, 0, 10, 11],
            "uncommitted_variant_row_count": 10,
            "files": {name: {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
                      for name, data in snapshots.items()},
        }
        (out / "wal-provenance.json").write_text(json.dumps(metadata, indent=2) + "\n")
        print("PASS: native and transformed WAL fixtures accepted by the pinned SQLite reference")


if __name__ == "__main__":
    main()
