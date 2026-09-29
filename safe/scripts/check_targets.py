#!/usr/bin/env python3
"""Compile-check portability. This intentionally does not claim runtime testing."""
import json
import subprocess
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
targets=["aarch64-apple-darwin","x86_64-unknown-linux-gnu","i686-unknown-linux-gnu",
         "s390x-unknown-linux-gnu","x86_64-pc-windows-gnu","wasm32-unknown-unknown",
         "aarch64-linux-android","aarch64-apple-ios","thumbv7em-none-eabi"]
results=[]
for target in targets:
    subprocess.run(["cargo","+stable","check","--manifest-path",str(ROOT/"Cargo.toml"),
        "--lib","--no-default-features","--target",target],check=True)
    results.append({"target":target,"no_std_core":"compile_checked","runtime":"not_run_by_this_script"})
(ROOT/"coverage/compile-checks.json").write_text(json.dumps(results,indent=2)+"\n")
print(f"PASS: no_std safe core compile-checked for {len(targets)} targets; this is not evidence of database runtime support on the other targets")
