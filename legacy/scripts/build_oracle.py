#!/usr/bin/env python3
"""Build C SQLite exclusively as a subprocess test reference, never a dependency."""
import os
import subprocess
from config import ROOT, DEFINES

out = ROOT / "build"
out.mkdir(exist_ok=True)
subprocess.run([
    os.environ.get("CC", "clang"), "-O2", *["-D" + d for d in DEFINES],
    str(ROOT / "upstream/sqlite3.c"), str(ROOT / "upstream/shell.c"),
    "-lm", "-lpthread", "-o", str(out / "sqlite3-reference"),
], check=True)
subprocess.run([str(out / "sqlite3-reference"), "--version"], check=True)
