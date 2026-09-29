#!/usr/bin/env python3
"""Link upstream's C regression harness to the Rust engine (no C SQLite core)."""
import subprocess
from config import ROOT, DEFINES

source = ROOT / "build/sqlite-source/sqlite-src-3530400"
assert source.is_dir(), "Run scripts/fetch_test_sources.py first"
files = ["test/fuzzcheck.c", "test/ossfuzz.c", "test/fuzzinvariants.c",
         "ext/recover/dbdata.c", "ext/recover/sqlite3recover.c", "test/vt02.c"]
files += [f"ext/misc/{name}.c" for name in ["base64", "base85", "completion", "decimal", "ieee754", "randomjson", "regexp", "series", "shathree", "sha1", "stmtrand"]]
subprocess.run([
    "clang", "-O2", "-DSQLITE_CORE", "-DSQLITE_OSS_FUZZ", "-DSQLITE_STATIC_RANDOMJSON",
    "-DSQLITE_DECIMAL_MAX_DIGIT=1000", *["-D" + d for d in DEFINES],
    "-I", str(ROOT / "upstream"), "-I", str(source / "ext/recover"), "-I", str(source / "test"),
    *[str(source / f) for f in files], str(ROOT / "target/release/libsqlite_rust.a"),
    "-liconv", "-lm", "-lpthread", "-o", str(ROOT / "build/fuzzcheck-rust"),
], check=True)
print("Built upstream fuzzcheck linked to Rust SQLite")
