#!/usr/bin/env python3
"""Inventory the pinned public upstream surface without implying implementation."""
import hashlib
import json
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT.parent / "legacy/build/sqlite-source/sqlite-src-3530400"
assert SOURCE.is_dir(), "Run the retained legacy/scripts/fetch_test_sources.py first"
out = ROOT / "coverage"
out.mkdir(exist_ok=True)
extensions = []
for directory in sorted((SOURCE / "ext").iterdir()):
    if not directory.is_dir(): continue
    if directory.name == "misc":
        for source in sorted(directory.glob("*.c")):
            extensions.append({"name": "misc/" + source.stem, "status": "not_started", "sources": [str(source.relative_to(SOURCE))]})
    else:
        extensions.append({"name": directory.name, "status": "not_started", "sources": [str(p.relative_to(SOURCE)) for p in sorted(directory.rglob("*")) if p.is_file()]})
tools = [{"path": str(p.relative_to(SOURCE)), "status": "not_started"} for p in sorted((SOURCE / "tool").rglob("*")) if p.is_file()]
tools.insert(0,{"path":"src/shell.c.in","status":"not_started"})
manifest = []
for directory in ("src", "ext", "tool"):
    for path in sorted((SOURCE / directory).rglob("*")):
        if path.is_file():
            manifest.append({"path": str(path.relative_to(SOURCE)), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
options = sorted(set(re.findall(r"\bSQLITE_(?:ENABLE|OMIT)_[A-Z0-9_]+\b", (ROOT.parent / "legacy/upstream/sqlite3.c").read_text())))
targets = subprocess.check_output(["rustc", "+stable", "--print", "target-list"], text=True).splitlines()
data = {
    "scope": "All public extensions and upstream utilities in the pinned SQLite 3.53.4 source archive. Commercial products are excluded by user instruction.",
    "source_archive": "https://www.sqlite.org/2026/sqlite-src-3530400.zip",
    "source_sha3_256": "b834d474b9b393d85a9e3ee4cc11f1329e007e9376a424ee740796f5c4bda3a8",
    "extensions": extensions,
    "utilities_and_build_tools": tools,
    "compile_switches_requiring_review": options,
    "rust_compiler_targets": [{"target": target, "status": "not_validated"} for target in targets],
    "source_files": manifest,
    "note": "Inventory membership is not implemented support. Primitive algorithms are not counted as SQL extensions until registered in a functioning safe SQL engine. See STATUS.md for actual validation.",
}
(out / "inventory.json").write_text(json.dumps(data, indent=2) + "\n")
rows = ["# Public upstream coverage inventory", "", data["scope"], "", "No public SQL extension or upstream utility has been completed in the safe rewrite yet. The legacy unsafe port's coverage is not counted.", "", f"Inventoried {len(extensions)} extension entries, {len(tools)} utility/build-tool files, {len(options)} compile switches, {len(targets)} compiler target triples, and {len(manifest)} source files.", "", "## Extensions", "", "| Component | Safe rewrite status |", "| --- | --- |"]
rows += [f"| `{entry['name']}` | Not started |" for entry in extensions]
rows += ["", "## Utilities and build tools", "", "| Upstream file | Safe rewrite status |", "| --- | --- |"]
rows += [f"| `{entry['path']}` | Not started |" for entry in tools]
rows += ["", "Source hashes, compile switches, and the compiler's complete target list are in [inventory.json](inventory.json). Being listed does not establish runtime support. Third-party extensions outside this upstream archive require their own explicit inventory.", ""]
(out / "INVENTORY.md").write_text("\n".join(rows))
print(f"Inventoried {len(extensions)} public extension entries; {len(tools)} utility/build-tool files; {len(options)} compile switches; {len(targets)} compiler targets; {len(manifest)} source files")
