#!/usr/bin/env python3
"""Compile implemented Unix adapter configurations. This does not run target code."""
import json
from pathlib import Path
import subprocess
ROOT=Path(__file__).resolve().parents[2]
TARGETS=['aarch64-apple-darwin','x86_64-unknown-linux-gnu','i686-unknown-linux-gnu',
         's390x-unknown-linux-gnu','aarch64-linux-android','aarch64-apple-ios']
results=[]
for target in TARGETS:
    subprocess.run(['cargo','check','-p','sqlite-safe-platform','--lib','--target',target],cwd=ROOT,check=True)
    results.append({'target':target,'unix_adapter':'compile_checked','runtime':'not_run_by_this_script'})
out=ROOT/'platform/coverage/compile-checks.json';out.parent.mkdir(exist_ok=True)
out.write_text(json.dumps(results,indent=2)+'\n')
print(f'PASS: Unix adapter compile-checked for {len(TARGETS)} targets; runtime and durability must be validated separately')
