#!/usr/bin/env python3
"""Isolated C-ABI worker for live C/Rust interoperability and crash tests."""
import ctypes as c
import json
import os
import sys

lib = c.CDLL(sys.argv[1])
lib.sqlite3_open.argtypes = [c.c_char_p, c.POINTER(c.c_void_p)]
lib.sqlite3_open.restype = c.c_int
lib.sqlite3_close.argtypes = [c.c_void_p]
lib.sqlite3_close.restype = c.c_int
CALLBACK = c.CFUNCTYPE(c.c_int, c.c_void_p, c.c_int, c.POINTER(c.c_char_p), c.POINTER(c.c_char_p))
lib.sqlite3_exec.argtypes = [c.c_void_p, c.c_char_p, CALLBACK, c.c_void_p, c.POINTER(c.c_char_p)]
lib.sqlite3_exec.restype = c.c_int
lib.sqlite3_free.argtypes = [c.c_void_p]
db = c.c_void_p()
rc = lib.sqlite3_open(os.fsencode(sys.argv[2]), c.byref(db))
print(json.dumps({"open": rc}), flush=True)
if rc:
    sys.exit(1)
try:
    for line in sys.stdin:
        command = json.loads(line)
        if command.get("crash"):
            os._exit(0)  # Deliberately skip connection cleanup in a test-only child.
        rows = []
        @CALLBACK
        def row(_, count, values, names):
            rows.append([values[i].decode("utf-8") if values[i] is not None else None for i in range(count)])
            return 0
        message = c.c_char_p()
        rc = lib.sqlite3_exec(db, command["sql"].encode(), row, None, c.byref(message))
        result = {"rc": rc, "rows": rows, "error": message.value.decode() if message.value else None}
        lib.sqlite3_free(message)
        print(json.dumps(result), flush=True)
finally:
    lib.sqlite3_close(db)
