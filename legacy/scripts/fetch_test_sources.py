#!/usr/bin/env python3
"""Download the checksum-pinned upstream source tree for SQLite's test harness."""
import hashlib
import io
import urllib.request
import zipfile
from config import ROOT

url = "https://www.sqlite.org/2026/sqlite-src-3530400.zip"
digest = "b834d474b9b393d85a9e3ee4cc11f1329e007e9376a424ee740796f5c4bda3a8"
archive_path = ROOT / "build/sqlite-src-3530400.zip"
archive_path.parent.mkdir(exist_ok=True)
if not archive_path.exists():
    archive_path.write_bytes(urllib.request.urlopen(url, timeout=60).read())
data = archive_path.read_bytes()
assert hashlib.sha3_256(data).hexdigest() == digest, "Upstream source checksum mismatch"
destination = ROOT / "build/sqlite-source"
with zipfile.ZipFile(io.BytesIO(data)) as archive:
    for member in archive.infolist():
        path = (destination / member.filename).resolve()
        assert path.is_relative_to(destination.resolve()), "Invalid archive path"
    archive.extractall(destination)
print(destination / "sqlite-src-3530400")
