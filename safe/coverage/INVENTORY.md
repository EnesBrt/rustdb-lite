# Public upstream coverage inventory

All public extensions and upstream utilities in the pinned SQLite 3.53.4 source archive. Commercial products are excluded by user instruction.

No public SQL extension or upstream utility has been completed in the safe rewrite yet. The legacy unsafe port's coverage is not counted.

Inventoried 71 extension entries, 95 utility/build-tool files, 139 compile switches, 307 compiler target triples, and 840 source files.

## Extensions

| Component | Safe rewrite status |
| --- | --- |
| `expert` | Not started |
| `fts3` | Not started |
| `fts5` | Not started |
| `icu` | Not started |
| `intck` | Not started |
| `jni` | Not started |
| `misc/amatch` | Not started |
| `misc/anycollseq` | Not started |
| `misc/appendvfs` | Not started |
| `misc/base64` | Not started |
| `misc/base85` | Not started |
| `misc/basexx` | Not started |
| `misc/blobio` | Not started |
| `misc/btreeinfo` | Not started |
| `misc/cksumvfs` | Not started |
| `misc/closure` | Not started |
| `misc/completion` | Not started |
| `misc/compress` | Not started |
| `misc/csv` | Not started |
| `misc/dbdump` | Not started |
| `misc/decimal` | Not started |
| `misc/eval` | Not started |
| `misc/explain` | Not started |
| `misc/fileio` | Not started |
| `misc/fossildelta` | Not started |
| `misc/fuzzer` | Not started |
| `misc/ieee754` | Not started |
| `misc/memstat` | Not started |
| `misc/memtrace` | Not started |
| `misc/mmapwarm` | Not started |
| `misc/nextchar` | Not started |
| `misc/noop` | Not started |
| `misc/normalize` | Not started |
| `misc/pcachetrace` | Not started |
| `misc/percentile` | Not started |
| `misc/prefixes` | Not started |
| `misc/qpvtab` | Not started |
| `misc/randomjson` | Not started |
| `misc/regexp` | Not started |
| `misc/remember` | Not started |
| `misc/rot13` | Not started |
| `misc/series` | Not started |
| `misc/sha1` | Not started |
| `misc/shathree` | Not started |
| `misc/showauth` | Not started |
| `misc/spellfix` | Not started |
| `misc/sqlar` | Not started |
| `misc/sqlite3_stdio` | Not started |
| `misc/stmt` | Not started |
| `misc/stmtrand` | Not started |
| `misc/templatevtab` | Not started |
| `misc/tmstmpvfs` | Not started |
| `misc/totype` | Not started |
| `misc/uint` | Not started |
| `misc/unionvtab` | Not started |
| `misc/urifuncs` | Not started |
| `misc/uuid` | Not started |
| `misc/vfslog` | Not started |
| `misc/vfsstat` | Not started |
| `misc/vfstrace` | Not started |
| `misc/vtablog` | Not started |
| `misc/vtshim` | Not started |
| `misc/wholenumber` | Not started |
| `misc/zipfile` | Not started |
| `misc/zorder` | Not started |
| `qrf` | Not started |
| `rbu` | Not started |
| `recover` | Not started |
| `rtree` | Not started |
| `session` | Not started |
| `wasm` | Not started |

## Utilities and build tools

| Upstream file | Safe rewrite status |
| --- | --- |
| `src/shell.c.in` | Not started |
| `tool/GetFile.cs` | Not started |
| `tool/GetTclKit.bat` | Not started |
| `tool/Replace.cs` | Not started |
| `tool/build-all-msvc.bat` | Not started |
| `tool/build-shell.sh` | Not started |
| `tool/buildtclext.tcl` | Not started |
| `tool/cg_anno.tcl` | Not started |
| `tool/checkSpacing.c` | Not started |
| `tool/cktclsh.sh` | Not started |
| `tool/cp.tcl` | Not started |
| `tool/custom.txt` | Not started |
| `tool/dbhash.c` | Not started |
| `tool/dbtotxt.c` | Not started |
| `tool/dbtotxt.md` | Not started |
| `tool/emcc.sh.in` | Not started |
| `tool/enlargedb.c` | Not started |
| `tool/extract-sqlite3h.tcl` | Not started |
| `tool/extract.c` | Not started |
| `tool/fast_vacuum.c` | Not started |
| `tool/fragck.tcl` | Not started |
| `tool/fuzzershell.c` | Not started |
| `tool/genfkey.README` | Not started |
| `tool/genfkey.test` | Not started |
| `tool/getlock.c` | Not started |
| `tool/index_usage.c` | Not started |
| `tool/lemon.c` | Not started |
| `tool/lempar.c` | Not started |
| `tool/libvers.c` | Not started |
| `tool/loadfts.c` | Not started |
| `tool/logest.c` | Not started |
| `tool/max-limits.c` | Not started |
| `tool/merge-test.tcl` | Not started |
| `tool/mkamalzip.tcl` | Not started |
| `tool/mkautoconfamal.sh` | Not started |
| `tool/mkccode.tcl` | Not started |
| `tool/mkcombo.tcl` | Not started |
| `tool/mkctimec.tcl` | Not started |
| `tool/mkfptab.c` | Not started |
| `tool/mkkeywordhash.c` | Not started |
| `tool/mkmsvcmin.tcl` | Not started |
| `tool/mkopcodec.tcl` | Not started |
| `tool/mkopcodeh.tcl` | Not started |
| `tool/mkopts.tcl` | Not started |
| `tool/mkpragmatab.tcl` | Not started |
| `tool/mkshellc.tcl` | Not started |
| `tool/mksourceid.c` | Not started |
| `tool/mksqlite3c-noext.tcl` | Not started |
| `tool/mksqlite3c.tcl` | Not started |
| `tool/mksqlite3h.tcl` | Not started |
| `tool/mksqlite3internalh.tcl` | Not started |
| `tool/mksrczip.tcl` | Not started |
| `tool/mktoolzip.tcl` | Not started |
| `tool/mkvsix.tcl` | Not started |
| `tool/mkwinarm64ec.tcl` | Not started |
| `tool/offsets.c` | Not started |
| `tool/omittest-msvc.tcl` | Not started |
| `tool/omittest.tcl` | Not started |
| `tool/opcodesum.tcl` | Not started |
| `tool/pagesig.c` | Not started |
| `tool/replace.tcl` | Not started |
| `tool/restore_jrnl.tcl` | Not started |
| `tool/rollback-test.c` | Not started |
| `tool/showdb.c` | Not started |
| `tool/showjournal.c` | Not started |
| `tool/showlocks.c` | Not started |
| `tool/showshm.c` | Not started |
| `tool/showstat4.c` | Not started |
| `tool/showtmlog.c` | Not started |
| `tool/showwal.c` | Not started |
| `tool/soak1.tcl` | Not started |
| `tool/spaceanal.tcl` | Not started |
| `tool/spellsift.tcl` | Not started |
| `tool/split-sqlite3c.tcl` | Not started |
| `tool/sqldiff.c` | Not started |
| `tool/sqlite3_analyzer.c.in` | Not started |
| `tool/sqlite3_rsync.c` | Not started |
| `tool/sqltclsh.c.in` | Not started |
| `tool/sqltclsh.tcl` | Not started |
| `tool/src-verify.c` | Not started |
| `tool/srcck1.c` | Not started |
| `tool/srctree-check.tcl` | Not started |
| `tool/stack_usage.tcl` | Not started |
| `tool/stripccomments.c` | Not started |
| `tool/symbols-mingw.sh` | Not started |
| `tool/symbols.sh` | Not started |
| `tool/tclConfigShToMake.sh` | Not started |
| `tool/varint.c` | Not started |
| `tool/vdbe-compress.tcl` | Not started |
| `tool/vdbe_profile.tcl` | Not started |
| `tool/version-info.c` | Not started |
| `tool/warnings-clang.sh` | Not started |
| `tool/warnings.sh` | Not started |
| `tool/win/sqlite.vsix` | Not started |
| `tool/winmain.c` | Not started |

Source hashes, compile switches, and the compiler's complete target list are in [inventory.json](inventory.json). Being listed does not establish runtime support. Third-party extensions outside this upstream archive require their own explicit inventory.
