#!/usr/bin/env python3
"""Verify source provenance and that the build has no SQLite C dependency."""
import hashlib
import json
from config import ROOT

source = json.loads((ROOT / "upstream/source.json").read_text())
for name, digest in source["files_sha256"].items():
    assert hashlib.sha256((ROOT / "upstream" / name).read_bytes()).hexdigest() == digest, name
translation = json.loads((ROOT / "upstream/translation.json").read_text())
engine = ROOT / "src/engine.rs"
assert hashlib.sha256(engine.read_bytes()).hexdigest() == translation["engine_sha256"]
assert not (ROOT / "build.rs").exists(), "Review any new build script for C compilation"
assert not any(marker in engine.read_text() for marker in ["unimplemented!", "todo!", "compile_error!", "panic!"])
print("PASS: upstream and generated source checksums; no translation stubs or Cargo C build script")
