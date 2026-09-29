# WAL fixtures

These small binary snapshots were produced by native SQLite, not the Rust writer.
`wal-provenance.json` records the producer, reference version, sizes, and SHA-256
hashes. The database has 512-byte pages and one table, `t(x)`, rooted at page 2.

The captured WAL has four frames. Frame 2 commits the empty table; frame 3 commits
integers 0 through 9; frame 4 commits the additional value 99. The main file is a
one-page snapshot taken before checkpointing. The `be` variant uses big-endian
checksum words. The `uncommitted` variant removes the last commit marker and
recalculates checksums; recovery must retain only the first ten rows.

The generator independently opens all three WAL variants with the pinned C SQLite
reference and checks integrity and query results before saving them. Regenerate
from the repository root with `python3 safe/scripts/generate_wal_fixtures.py` after
building that reference. Random salts change fixture hashes on regeneration.
These fixtures contain synthetic data only. Rust tests embed their bytes and do
not link or execute native SQLite.
