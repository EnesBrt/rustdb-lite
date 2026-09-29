#!/usr/bin/env python3
"""Apply explicit, reproducible compatibility changes to the C2Rust output."""
import hashlib
import json
import re
from config import ROOT, VERSION, DEFINES

source = ROOT / "build/translated/src/sqlite3.rs"
text = source.read_text()
changes = {}
for old, new in (
    ("::core::ffi::VaListImpl", "::core::ffi::VaList"),
    (".as_va_list()", ".clone()"),
    ("::core::intrinsics::atomic_load_relaxed", "crate::compat::atomic_load_relaxed"),
    ("::core::intrinsics::atomic_store_relaxed", "crate::compat::atomic_store_relaxed"),
    ("::core::intrinsics::atomic_fence_seqcst", "crate::compat::atomic_fence_seqcst"),
    ("sqlite3Config.xTestCallback = ::core::mem::transmute(",
     "sqlite3Config.xTestCallback = ::core::mem::transmute::<_, Option<unsafe extern \"C\" fn(::core::ffi::c_int) -> ::core::ffi::c_int>>("),
    ("sqlite3Config.xAltLocaltime = ::core::mem::transmute(",
     "sqlite3Config.xAltLocaltime = ::core::mem::transmute::<_, Option<unsafe extern \"C\" fn(*const ::core::ffi::c_void, *mut ::core::ffi::c_void) -> ::core::ffi::c_int>>("),
):
    changes[old] = text.count(old)
    assert changes[old] > 0, f"Expected translation construct is missing: {old}"
    text = text.replace(old, new)
# C2Rust moves these three constant address calculations into a Mach-O startup
# constructor. They are legal Rust const expressions. Restore static initializers
# to avoid sanitizer redzones in the loader's function-pointer section.
initializer_start = text.index('unsafe extern "C" fn run_static_initializers() {')
initializers = text[initializer_start:]
assert initializers.rstrip().endswith("static INIT_ARRAY: [unsafe extern \"C\" fn(); 1] = [run_static_initializers];")
for name in ("sqlite3aGTb", "sqlite3aEQb", "sqlite3aLTb"):
    expression = re.search(r"    " + name + r" = (.*?);", initializers, re.S).group(1)
    pattern = r"(static mut " + name + r": \*const ::core::ffi::c_uchar = )::core::ptr::null::<\s*::core::ffi::c_uchar,\s*>\(\);"
    text, count = re.subn(pattern, lambda m: m[1] + "unsafe { " + expression + " };", text)
    assert count == 1, f"Expected one static initializer for {name}"
    changes["const initializer " + name] = count
text = text[:text.index('unsafe extern "C" fn run_static_initializers() {')]
for forbidden in ("compile_error!", "todo!", "unimplemented!", "panic!"):
    assert forbidden not in text, f"Translation contains a stub: {forbidden}"
header = "// Generated from SQLite " + VERSION + " using C2Rust 0.22.1.\n"
header += "// See scripts/postprocess.py and PORTING.md; edit the migration pipeline, not this file.\n"
(ROOT / "src/engine.rs").write_text(header + text)
manifest = dict(sqlite_version=VERSION, c2rust_version="0.22.1",
                target="aarch64-apple-darwin", defines=DEFINES,
                patches=changes, source_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
                engine_sha256=hashlib.sha256((header + text).encode()).hexdigest())
(ROOT / "upstream/translation.json").write_text(json.dumps(manifest, indent=2) + "\n")
print(f"Wrote {len(text.splitlines())} lines of Rust; patches: {changes}")
