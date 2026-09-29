#!/usr/bin/env python3
"""Build and run all eight pinned public SQLite fuzz regression corpora."""
import subprocess
import sys
from config import ROOT

source = ROOT / "build/sqlite-source/sqlite-src-3530400"
if not source.exists():
    subprocess.run([sys.executable, str(ROOT / "scripts/fetch_test_sources.py")], check=True)
subprocess.run([sys.executable, str(ROOT / "scripts/build_fuzzcheck.py")], check=True)
subprocess.run([str(ROOT / "build/fuzzcheck-rust"), "--native-malloc", "--brief", "--timeout", "10000",
    *[str(source / f"test/fuzzdata{i}.db") for i in range(1, 9)]], check=True, timeout=600)
