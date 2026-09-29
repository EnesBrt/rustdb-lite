#!/usr/bin/env python3
"""Regenerate the Rust engine; normal Cargo builds never execute this script."""
import os
import platform
import subprocess
import sys
from config import ROOT

assert platform.system() == "Darwin" and platform.machine() == "arm64", "This migration is validated for Apple Silicon macOS"
tool = ROOT / ".tools/bin/c2rust"
if not tool.exists():
    environment = os.environ.copy()
    environment.setdefault("LLVM_CONFIG_PATH", "/opt/homebrew/opt/llvm@18/bin/llvm-config")
    environment.setdefault("LIBCLANG_PATH", "/opt/homebrew/opt/llvm@18/lib")
    environment.setdefault("CLANG_PATH", "/opt/homebrew/opt/llvm@18/bin/clang")
    subprocess.run(["cargo", "+stable", "install", "--locked", "--version", "0.22.1", "c2rust", "--root", str(ROOT / ".tools")], env=environment, check=True)
assert "0.22.1" in subprocess.check_output([str(tool), "--version"], text=True)
subprocess.run([sys.executable, str(ROOT / "scripts/prepare.py")], check=True)
with (ROOT / "build/transpile.log").open("w") as log:
    subprocess.run([str(tool), "transpile", "--emit-build-files", "--fail-on-error",
        "--disable-rustfmt", "--overwrite-existing", "--output-dir", str(ROOT / "build/translated"),
        str(ROOT / "upstream/compile_commands.json")], stdout=log, stderr=subprocess.STDOUT, check=True)
subprocess.run([sys.executable, str(ROOT / "scripts/postprocess.py")], check=True)
print("Regenerated engine; run the validation commands in TESTING.md before accepting changes")
