#!/usr/bin/env python3
"""Fetch verified upstream inputs and prepare the native compilation database."""
import hashlib
import io
import json
import os
import shutil
import subprocess
import urllib.request
import zipfile
from pathlib import Path
from config import ROOT, ARCHIVE_URL, ARCHIVE_SHA3, VERSION, DEFINES

upstream = ROOT / "upstream"
upstream.mkdir(exist_ok=True)
data = urllib.request.urlopen(ARCHIVE_URL, timeout=60).read()
assert hashlib.sha3_256(data).hexdigest() == ARCHIVE_SHA3, "Upstream checksum mismatch"
with zipfile.ZipFile(io.BytesIO(data)) as archive:
    for name in ("sqlite3.c", "sqlite3.h", "sqlite3ext.h", "shell.c"):
        member = next(n for n in archive.namelist() if n.endswith("/" + name))
        (upstream / name).write_bytes(archive.read(member))
metadata = dict(version=VERSION, url=ARCHIVE_URL, archive_sha3_256=ARCHIVE_SHA3)
metadata["files_sha256"] = {
    name: hashlib.sha256((upstream / name).read_bytes()).hexdigest()
    for name in ("sqlite3.c", "sqlite3.h", "sqlite3ext.h", "shell.c")
}
(upstream / "source.json").write_text(json.dumps(metadata, indent=2) + "\n")
clang = os.environ.get("CLANG_PATH", "/opt/homebrew/opt/llvm@18/bin/clang")
if not Path(clang).is_file():
    clang = shutil.which("clang")
args = [clang, "-std=gnu11", "-c", *["-D" + d for d in DEFINES]]
if os.uname().sysname == "Darwin":
    sdk = subprocess.check_output(["xcrun", "--show-sdk-path"], text=True).strip()
    args += ["-isysroot", sdk]
args += [str(upstream / "sqlite3.c"), "-o", str(ROOT / "build/sqlite3.o")]
(ROOT / "build").mkdir(exist_ok=True)
(upstream / "compile_commands.json").write_text(json.dumps([
    dict(directory=str(upstream), file=str(upstream / "sqlite3.c"), arguments=args)
], indent=2) + "\n")
print(f"Verified SQLite {VERSION}; wrote compilation database with {len(DEFINES)} options")
