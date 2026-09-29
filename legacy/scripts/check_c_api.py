#!/usr/bin/env python3
import re
import subprocess
from config import ROOT, DEFINES

reference = ROOT / "build/libsqlite3_reference.dylib"
rust = ROOT / "target/release/libsqlite_rust.dylib"
subprocess.run(["clang", "-O2", "-dynamiclib", *["-D" + d for d in DEFINES],
    str(ROOT / "upstream/sqlite3.c"), "-o", str(reference)], check=True)
for label, library in [("reference", reference), ("rust", rust)]:
    client = ROOT / f"build/c-api-{label}"
    subprocess.run(["clang", "-O2", *["-D" + d for d in DEFINES],
        "-I", str(ROOT / "upstream"), str(ROOT / "tests/c_api.c"), str(library),
        "-o", str(client)], check=True)
    subprocess.run([str(client)], check=True)

def exports(library):
    output = subprocess.check_output(["nm", "-gU", str(library)], text=True)
    return {line.split()[-1] for line in output.splitlines() if re.search(r"\b_sqlite3\w*$", line)}

expected, actual = exports(reference), exports(rust)
missing = expected - actual
assert not missing, f"Missing C API exports: {sorted(missing)}"
imports = subprocess.check_output(["nm", "-u", str(rust)], text=True)
assert not re.search(r"\b_sqlite3\w*", imports), "Rust library imports C SQLite"
print(f"PASS: all {len(expected)} configured upstream SQLite exports present; no unresolved SQLite imports")
