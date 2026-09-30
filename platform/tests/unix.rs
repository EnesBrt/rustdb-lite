#![forbid(unsafe_code)]
#![cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
))]
use sqlite_safe::{
    journal::{self, JournalLimits},
    pager::{Pager, Storage},
    sql::JournaledConnection,
    Error, Value,
};
use sqlite_safe_platform::unix::UnixStorage;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "sqlite-safe-file-{}-{nanos}-{id}",
            std::process::id()
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn db(&self) -> PathBuf {
        self.0.join("db.sqlite")
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn open(path: &Path) -> JournaledConnection<UnixStorage> {
    JournaledConnection::open(
        UnixStorage::new(path).unwrap(),
        512,
        JournalLimits::default(),
    )
    .unwrap()
}
fn seed(path: &Path) {
    let mut c = open(path);
    c.execute("CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT UNIQUE)", &[])
        .unwrap();
    c.execute("INSERT INTO t VALUES(1,'old')", &[]).unwrap();
}
#[test]
fn joined_views_and_insert_select_persist_across_reopen_and_rollback() {
    let temp = Temp::new();
    let path = temp.db();
    let mut c = open(&path);
    for sql in [
        "CREATE TABLE a(x INT,y TEXT)",
        "CREATE TABLE b(x INT,z TEXT)",
        "INSERT INTO a VALUES(1,'left'),(2,'both')",
        "INSERT INTO b VALUES(2,'both'),(3,'right')",
        "CREATE VIEW v AS SELECT * FROM a FULL JOIN b USING(x)",
        "CREATE TABLE result AS SELECT * FROM v",
    ] {
        c.execute(sql, &[]).unwrap();
    }
    drop(c);
    let mut c = open(&path);
    assert_eq!(
        c.execute("SELECT x FROM v ORDER BY x", &[]).unwrap().rows,
        vec![
            vec![Value::Integer(1)],
            vec![Value::Integer(2)],
            vec![Value::Integer(3)]
        ]
    );
    let before = fs::read(&path).unwrap();
    c.execute("BEGIN", &[]).unwrap();
    c.execute("INSERT INTO a SELECT x,'new' FROM v WHERE y IS NULL", &[])
        .unwrap();
    c.execute("ROLLBACK", &[]).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(c
        .execute(
            "INSERT INTO a SELECT abs(-9223372036854775808),z FROM v",
            &[]
        )
        .is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    c.execute("INSERT INTO a SELECT x,'new' FROM v WHERE y IS NULL", &[])
        .unwrap();
    drop(c);
    let mut c = open(&path);
    assert_eq!(
        c.execute("SELECT count(*) FROM v WHERE y IS NULL", &[])
            .unwrap()
            .rows,
        vec![vec![Value::Integer(0)]]
    );
    assert_eq!(
        c.execute("SELECT count(*) FROM result WHERE y IS NULL", &[])
            .unwrap()
            .rows,
        vec![vec![Value::Integer(1)]]
    );
}

#[test]
fn expression_partial_indexes_persist_membership_conflicts_and_fail_prefixes() {
    let temp = Temp::new();
    let path = temp.db();
    let mut c = open(&path);
    c.execute(
        "CREATE TABLE t(id INTEGER PRIMARY KEY,x TEXT,active INT)",
        &[],
    )
    .unwrap();
    c.execute("INSERT INTO t VALUES(1,'Rust',1),(2,'RUST',0)", &[])
        .unwrap();
    c.execute("CREATE UNIQUE INDEX ix ON t(lower(x)) WHERE active=1", &[])
        .unwrap();
    drop(c);
    let mut c = open(&path);
    let before = fs::read(&path).unwrap();
    assert!(c.execute("UPDATE t SET active=1 WHERE id=2", &[]).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    c.execute("INSERT INTO t VALUES(3,'RUST',1) ON CONFLICT(lower(x)) WHERE active=1 DO UPDATE SET x='changed'",&[]).unwrap();
    c.execute("BEGIN", &[]).unwrap();
    c.execute("UPDATE t SET active=0", &[]).unwrap();
    c.execute("ROLLBACK", &[]).unwrap();
    assert!(c
        .execute(
            "INSERT OR FAIL INTO t VALUES(3,'fresh',1),(4,'CHANGED',1)",
            &[]
        )
        .is_err());
    drop(c);
    let mut c = open(&path);
    assert_eq!(
        c.execute("SELECT id,active FROM t ORDER BY id", &[])
            .unwrap()
            .rows,
        vec![
            vec![Value::Integer(1), Value::Integer(1)],
            vec![Value::Integer(2), Value::Integer(0)],
            vec![Value::Integer(3), Value::Integer(1)]
        ]
    );
    c.execute("UPDATE t SET active=1 WHERE id=2", &[]).unwrap();
    assert!(c.execute("INSERT INTO t VALUES(4,'rust',1)", &[]).is_err());
}

#[test]
fn generated_values_and_unique_indexes_survive_commit_reopen_and_rollback() {
    let temp = Temp::new();
    let path = temp.db();
    let mut c = open(&path);
    c.execute("CREATE TABLE t(id INTEGER PRIMARY KEY,x INT,g INT AS(x*2) UNIQUE,s TEXT AS(g||'!') STORED)", &[]).unwrap();
    c.execute("INSERT INTO t(x) VALUES(3),(4)", &[]).unwrap();
    drop(c);
    let mut c = open(&path);
    assert_eq!(
        c.execute("SELECT g FROM t ORDER BY id", &[]).unwrap().rows,
        vec![vec![Value::Integer(6)], vec![Value::Integer(8)]]
    );
    c.execute(
        "INSERT INTO t(x) VALUES(3) ON CONFLICT(g) DO UPDATE SET x=excluded.g RETURNING *",
        &[],
    )
    .unwrap();
    let before = fs::read(&path).unwrap();
    assert!(c.execute("INSERT INTO t(x) VALUES(9),(6)", &[]).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    c.execute("BEGIN", &[]).unwrap();
    c.execute("UPDATE t SET x=x+100", &[]).unwrap();
    c.execute("ROLLBACK", &[]).unwrap();
    assert!(c
        .execute("INSERT OR FAIL INTO t(x) VALUES(9),(6)", &[])
        .is_err());
    drop(c);
    let mut c = open(&path);
    assert_eq!(
        c.execute("SELECT g,s FROM t ORDER BY id", &[])
            .unwrap()
            .rows,
        vec![
            vec![
                Value::Integer(12),
                Value::Text(sqlite_safe::Text::utf8("12!"))
            ],
            vec![
                Value::Integer(8),
                Value::Text(sqlite_safe::Text::utf8("8!"))
            ],
            vec![
                Value::Integer(18),
                Value::Text(sqlite_safe::Text::utf8("18!"))
            ],
        ]
    );
}

#[test]
fn without_rowid_primary_updates_persist_and_failures_preserve_files() {
    let temp = Temp::new();
    let path = temp.db();
    let mut c = open(&path);
    c.execute(
        "CREATE TABLE t(a TEXT,b INT,v ANY,PRIMARY KEY(a,b)) WITHOUT ROWID,STRICT",
        &[],
    )
    .unwrap();
    c.execute("CREATE INDEX ix ON t(v)", &[]).unwrap();
    c.execute("INSERT INTO t VALUES('a',1,'001'),('b',2,2)", &[])
        .unwrap();
    assert_eq!(c.last_insert_rowid().unwrap(), 0);
    c.execute("UPDATE t SET b=b+10 RETURNING *", &[]).unwrap();
    drop(c);
    let before = fs::read(&path).unwrap();
    let mut c = open(&path);
    assert_eq!(
        c.execute("SELECT b FROM t ORDER BY a", &[]).unwrap().rows,
        vec![vec![Value::Integer(11)], vec![Value::Integer(12)]]
    );
    assert!(matches!(
        c.execute("INSERT INTO t VALUES('c',3,3),('a',11,4)", &[]),
        Err(Error::Constraint(_))
    ));
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(matches!(
        c.execute("INSERT OR FAIL INTO t VALUES('c',3,3),('a',11,4)", &[]),
        Err(Error::Constraint(_))
    ));
    drop(c);
    let mut c = open(&path);
    assert_eq!(
        c.execute("SELECT count(*) FROM t", &[]).unwrap().rows,
        vec![vec![Value::Integer(3)]]
    );
    assert_eq!(c.last_insert_rowid().unwrap(), 0);
    c.execute("BEGIN", &[]).unwrap();
    c.execute("DELETE FROM t", &[]).unwrap();
    c.execute("ROLLBACK", &[]).unwrap();
    drop(c);
    assert_eq!(
        open(&path)
            .execute("SELECT count(*) FROM t", &[])
            .unwrap()
            .rows,
        vec![vec![Value::Integer(3)]]
    );
}

#[test]
fn strict_datatype_errors_persist_only_retained_transaction_prefixes() {
    let temp = Temp::new();
    let path = temp.db();
    let mut c = open(&path);
    c.execute("CREATE TABLE t(x INT,a ANY) STRICT", &[])
        .unwrap();
    let before = fs::read(&path).unwrap();
    assert!(matches!(
        c.execute("INSERT INTO t VALUES(1,'001'),('bad',2)", &[]),
        Err(Error::Datatype(_))
    ));
    assert_eq!(fs::read(&path).unwrap(), before);
    c.execute("BEGIN", &[]).unwrap();
    assert!(matches!(
        c.execute("INSERT INTO t VALUES(1,'001'),('bad',2)", &[]),
        Err(Error::Datatype(_))
    ));
    assert_eq!(c.changes().unwrap(), 0);
    assert_eq!(c.total_changes().unwrap(), 0);
    assert_eq!(fs::read(&path).unwrap(), before);
    c.execute("COMMIT", &[]).unwrap();
    drop(c);
    let mut c = open(&path);
    let rows = c.execute("SELECT * FROM t", &[]).unwrap().rows;
    assert_eq!(
        rows,
        vec![vec![
            Value::Integer(1),
            Value::Text(sqlite_safe::Text::utf8("001"))
        ]]
    );
    c.execute("BEGIN", &[]).unwrap();
    assert!(matches!(
        c.execute("INSERT INTO t VALUES(2,2),('bad',3)", &[]),
        Err(Error::Datatype(_))
    ));
    c.execute("ROLLBACK", &[]).unwrap();
    drop(c);
    assert_eq!(
        open(&path).execute("SELECT * FROM t", &[]).unwrap().rows,
        rows
    );
}

#[test]
fn autoincrement_persists_high_water_and_rolls_back_full_transactions() {
    let temp = Temp::new();
    let path = temp.db();
    let mut c = open(&path);
    c.execute(
        "CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,x UNIQUE)",
        &[],
    )
    .unwrap();
    c.execute("INSERT INTO t VALUES(100,1)", &[]).unwrap();
    c.execute("DELETE FROM t", &[]).unwrap();
    drop(c);
    let mut c = open(&path);
    assert_eq!(
        c.execute("INSERT INTO t(x) VALUES(1) RETURNING id", &[])
            .unwrap()
            .rows,
        vec![vec![Value::Integer(101)]]
    );
    assert!(matches!(
        c.execute("INSERT OR FAIL INTO t(x) VALUES(2),(1)", &[]),
        Err(Error::Constraint(_))
    ));
    drop(c);
    let mut c = open(&path);
    assert_eq!(
        c.execute("SELECT seq FROM sqlite_sequence", &[])
            .unwrap()
            .rows,
        vec![vec![Value::Integer(101)]]
    );
    assert_eq!(
        c.execute("INSERT INTO t(x) VALUES(3) RETURNING id", &[])
            .unwrap()
            .rows,
        vec![vec![Value::Integer(103)]]
    );
    drop(c);
    let before = fs::read(&path).unwrap();
    let mut c = open(&path);
    c.execute("BEGIN", &[]).unwrap();
    c.execute("INSERT INTO t VALUES(9223372036854775807,9)", &[])
        .unwrap();
    assert_eq!(
        c.execute("INSERT INTO t(x) VALUES(10)", &[]),
        Err(Error::Full)
    );
    assert!(c.is_autocommit().unwrap());
    drop(c);
    assert_eq!(fs::read(&path).unwrap(), before);
    let mut c = open(&path);
    assert_eq!(
        c.execute("INSERT INTO t(x) VALUES(4) RETURNING id", &[])
            .unwrap()
            .rows,
        vec![vec![Value::Integer(104)]]
    );
    c.execute("DROP TABLE t", &[]).unwrap();
    drop(c);
    assert!(open(&path)
        .execute("SELECT * FROM sqlite_sequence", &[])
        .unwrap()
        .rows
        .is_empty());
}
#[test]
fn returning_persists_changes_and_keeps_fail_prefix_without_partial_results() {
    let temp = Temp::new();
    let path = temp.db();
    seed(&path);
    let mut c = open(&path);
    let r = c
        .execute(
            "INSERT INTO t VALUES(2,'two'),(3,'three') RETURNING id",
            &[],
        )
        .unwrap();
    assert_eq!(
        r.rows,
        vec![vec![Value::Integer(2)], vec![Value::Integer(3)]]
    );
    drop(c);
    let before = fs::read(&path).unwrap();
    let mut c = open(&path);
    assert!(c
        .execute(
            "DELETE FROM t RETURNING abs(CASE id WHEN 2 THEN -9223372036854775808 ELSE 1 END)",
            &[]
        )
        .is_err());
    drop(c);
    assert_eq!(fs::read(&path).unwrap(), before);
    let mut c = open(&path);
    assert!(matches!(
        c.execute(
            "INSERT OR FAIL INTO t VALUES(4,'four'),(5,'old') RETURNING *",
            &[]
        ),
        Err(Error::Constraint(_))
    ));
    drop(c);
    let mut c = open(&path);
    assert_eq!(
        c.execute("SELECT id FROM t ORDER BY id", &[]).unwrap().rows,
        vec![
            vec![Value::Integer(1)],
            vec![Value::Integer(2)],
            vec![Value::Integer(3)],
            vec![Value::Integer(4)]
        ]
    );
    c.execute("BEGIN", &[]).unwrap();
    let r = c
        .execute("UPDATE t SET id=id+10 RETURNING id", &[])
        .unwrap();
    assert_eq!(r.rows.len(), 4);
    c.execute("ROLLBACK", &[]).unwrap();
    drop(c);
    assert_eq!(
        open(&path)
            .execute("SELECT min(id) FROM t", &[])
            .unwrap()
            .rows,
        vec![vec![Value::Integer(1)]]
    );
}
#[test]
fn upserts_commit_and_failed_update_preserves_the_persisted_database() {
    let temp = Temp::new();
    let path = temp.db();
    seed(&path);
    let mut c = open(&path);
    c.execute(
        "INSERT INTO t VALUES(2,'old') ON CONFLICT(v) DO UPDATE SET id=excluded.id",
        &[],
    )
    .unwrap();
    c.execute("INSERT INTO t VALUES(3,'new') ON CONFLICT DO NOTHING", &[])
        .unwrap();
    drop(c);
    let before = fs::read(&path).unwrap();
    let mut c = open(&path);
    assert!(c
        .execute(
            "INSERT OR IGNORE INTO t VALUES(4,'four'),(5,'old') ON CONFLICT(v) DO UPDATE SET id=3",
            &[]
        )
        .is_err());
    assert_eq!(
        c.execute("SELECT id FROM t ORDER BY id", &[]).unwrap().rows,
        vec![vec![Value::Integer(2)], vec![Value::Integer(3)]]
    );
    drop(c);
    assert_eq!(fs::read(&path).unwrap(), before);
    let mut c = open(&path);
    c.execute("BEGIN", &[]).unwrap();
    c.execute(
        "INSERT INTO t VALUES(2,'changed') ON CONFLICT(id) DO UPDATE SET v=excluded.v",
        &[],
    )
    .unwrap();
    c.execute("COMMIT", &[]).unwrap();
    drop(c);
    let mut c = open(&path);
    assert_eq!(
        c.execute("SELECT v FROM t WHERE id=2", &[]).unwrap().rows,
        vec![vec![Value::Text(sqlite_safe::Text::utf8("changed"))]]
    );
}
#[test]
fn conflict_policies_persist_prefixes_and_discard_rolled_back_transactions() {
    for transaction in [false, true] {
        for policy in ["ABORT", "FAIL", "ROLLBACK", "IGNORE", "REPLACE"] {
            let temp = Temp::new();
            let path = temp.db();
            seed(&path);
            let before = fs::read(&path).unwrap();
            let mut c = open(&path);
            if transaction {
                c.execute("SAVEPOINT outer", &[]).unwrap();
                c.execute("INSERT INTO t VALUES(10,'prior')", &[]).unwrap();
            }
            let result = c.execute(
                &format!("INSERT OR {policy} INTO t VALUES(2,'two'),(3,'old'),(4,'four')"),
                &[],
            );
            assert_eq!(result.is_ok(), matches!(policy, "IGNORE" | "REPLACE"));
            if transaction {
                assert_eq!(fs::read(&path).unwrap(), before);
                if policy == "ROLLBACK" {
                    assert!(c.is_autocommit().unwrap());
                    assert!(c.execute("RELEASE outer", &[]).is_err());
                } else {
                    assert!(!c.is_autocommit().unwrap());
                    c.execute("RELEASE outer", &[]).unwrap();
                }
            }
            drop(c);
            let mut c = open(&path);
            let mut ids = match policy {
                "FAIL" => vec![1, 2],
                "IGNORE" => vec![1, 2, 4],
                "REPLACE" => vec![2, 3, 4],
                _ => vec![1],
            };
            if transaction && policy != "ROLLBACK" {
                ids.push(10);
            }
            assert_eq!(
                c.execute("SELECT id FROM t ORDER BY id", &[]).unwrap().rows,
                ids.into_iter()
                    .map(|id| vec![Value::Integer(id)])
                    .collect::<Vec<_>>()
            );
            if policy == "ROLLBACK" || policy == "ABORT" && !transaction {
                assert_eq!(fs::read(&path).unwrap(), before);
            }
        }
    }
}
#[test]
fn view_schema_and_create_table_as_select_persist_atomically() {
    let temp = Temp::new();
    let path = temp.db();
    let mut c = open(&path);
    for sql in [
        "CREATE TABLE t(x INTEGER)",
        "INSERT INTO t VALUES(1),(2)",
        "CREATE VIEW v AS SELECT x+10 y FROM t",
        "CREATE TABLE snapshot AS SELECT * FROM v",
        "UPDATE t SET x=x+1",
    ] {
        c.execute(sql, &[]).unwrap();
    }
    drop(c);
    let mut c = open(&path);
    assert_eq!(
        c.execute("SELECT y FROM v ORDER BY y", &[]).unwrap().rows,
        vec![vec![Value::Integer(12)], vec![Value::Integer(13)]]
    );
    assert_eq!(
        c.execute("SELECT y FROM snapshot ORDER BY y", &[])
            .unwrap()
            .rows,
        vec![vec![Value::Integer(11)], vec![Value::Integer(12)]]
    );
    for sql in [
        "BEGIN",
        "DROP VIEW v",
        "CREATE VIEW v AS SELECT 99 y",
        "ROLLBACK",
    ] {
        c.execute(sql, &[]).unwrap();
    }
    drop(c);
    let before = fs::read(&path).unwrap();
    let mut c = open(&path);
    assert!(c
        .execute("CREATE TABLE failed AS SELECT * FROM missing", &[])
        .is_err());
    assert!(c.execute("INSERT INTO v VALUES(99)", &[]).is_err());
    assert_eq!(
        c.execute("SELECT sum(y) FROM v", &[]).unwrap().rows,
        vec![vec![Value::Integer(25)]]
    );
    drop(c);
    assert_eq!(fs::read(&path).unwrap(), before);
}
#[test]
fn cte_data_changes_commit_and_reopen() {
    let temp = Temp::new();
    let path = temp.db();
    let mut c = open(&path);
    c.execute("CREATE TABLE t(x UNIQUE)", &[]).unwrap();
    c.execute("WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM q WHERE x<5) INSERT INTO t SELECT x FROM q", &[]).unwrap();
    c.execute("BEGIN", &[]).unwrap();
    c.execute("WITH q AS(SELECT 1) DELETE FROM t", &[]).unwrap();
    c.execute("ROLLBACK", &[]).unwrap();
    drop(c);
    let mut c = open(&path);
    assert_eq!(
        c.execute("SELECT count(*),sum(x) FROM t", &[])
            .unwrap()
            .rows,
        vec![vec![Value::Integer(5), Value::Integer(15)]]
    );
}
#[test]
fn subquery_updates_persist_and_failed_insert_preserves_file() {
    let temp = Temp::new();
    let path = temp.db();
    let mut c = open(&path);
    for sql in [
        "CREATE TABLE out(x INTEGER UNIQUE)",
        "CREATE TABLE pairs(old,new)",
        "INSERT INTO out VALUES(1),(2),(3)",
        "INSERT INTO pairs VALUES(1,11),(2,22),(3,33)",
        "UPDATE out SET x=(SELECT new FROM pairs WHERE old=out.x)",
    ] {
        c.execute(sql, &[]).unwrap();
    }
    drop(c);
    let mut c = open(&path);
    assert_eq!(
        c.execute("SELECT x FROM out ORDER BY x", &[]).unwrap().rows,
        vec![
            vec![Value::Integer(11)],
            vec![Value::Integer(22)],
            vec![Value::Integer(33)]
        ]
    );
    c.execute("BEGIN", &[]).unwrap();
    c.execute(
        "DELETE FROM out WHERE EXISTS(SELECT 1 FROM pairs WHERE new=out.x AND old>1)",
        &[],
    )
    .unwrap();
    c.execute("ROLLBACK", &[]).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(c
        .execute("INSERT INTO out VALUES((SELECT 44)),((SELECT 11))", &[])
        .is_err());
    assert_eq!(
        c.execute("SELECT count(*) FROM out", &[]).unwrap().rows,
        vec![vec![Value::Integer(3)]]
    );
    drop(c);
    assert_eq!(fs::read(&path).unwrap(), before);
}
#[test]
fn real_file_sql_commit_rollback_and_reopen() {
    let temp = Temp::new();
    let path = temp.db();
    seed(&path);
    let before = fs::read(&path).unwrap();
    let mut c = open(&path);
    for sql in [
        "BEGIN",
        "INSERT INTO t VALUES(2,'new')",
        "SAVEPOINT s",
        "DELETE FROM t",
        "ROLLBACK TO s",
        "RELEASE s",
        "COMMIT",
    ] {
        c.execute(sql, &[]).unwrap();
    }
    assert!(c.execute("INSERT INTO t VALUES(3,'new')", &[]).is_err());
    c.execute("BEGIN", &[]).unwrap();
    c.execute("DELETE FROM t", &[]).unwrap();
    drop(c);
    assert_ne!(fs::read(&path).unwrap(), before);
    let mut c = open(&path);
    assert_eq!(
        c.execute("SELECT count(*) FROM t", &[]).unwrap().rows,
        vec![vec![Value::Integer(2)]]
    );
    drop(c);
    assert!(!temp.0.join("db.sqlite-journal").exists());
}
#[test]
fn own_connections_and_aliases_do_not_bypass_exclusive_locks() {
    let temp = Temp::new();
    let path = temp.db();
    seed(&path);
    let first = open(&path);
    for path in [&path, &temp.0.join(".").join("db.sqlite")] {
        assert!(matches!(
            Pager::open(UnixStorage::new(path).unwrap(), JournalLimits::default()),
            Err(Error::Busy(_))
        ));
    }
    // A different database is independent of the first inode's lock.
    let second = open(&temp.0.join("second.sqlite"));
    drop(first);
    drop(second);
    assert!(Pager::open(UnixStorage::new(&path).unwrap(), JournalLimits::default()).is_ok());
}
#[test]
fn real_journal_recovery_and_corrupt_journal_retention() {
    let temp = Temp::new();
    let path = temp.db();
    seed(&path);
    let original = fs::read(&path).unwrap();
    let journal = journal::encode(&original, 512, 65536, 123, JournalLimits::default()).unwrap();
    let journal_path = temp.0.join("db.sqlite-journal");
    fs::write(&journal_path, &journal).unwrap();
    fs::write(&path, [0u8; 512]).unwrap();
    drop(open(&path));
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(!journal_path.exists());
    let mut damaged = journal;
    damaged[65536 + 4 + 312] ^= 1;
    fs::write(&journal_path, &damaged).unwrap();
    assert!(Pager::open(UnixStorage::new(&path).unwrap(), JournalLimits::default()).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(fs::read(&journal_path).unwrap(), damaged);
}
#[test]
fn sidecar_path_and_file_replacement_guards() {
    let temp = Temp::new();
    let path = temp.db();
    seed(&path);
    let original = fs::read(&path).unwrap();
    fs::write(temp.0.join("db.sqlite-wal"), b"unfinished WAL").unwrap();
    assert!(Pager::open(UnixStorage::new(&path).unwrap(), JournalLimits::default()).is_err());
    fs::remove_file(temp.0.join("db.sqlite-wal")).unwrap();
    std::os::unix::fs::symlink(&path, temp.0.join("alias.sqlite")).unwrap();
    assert!(Pager::open(
        UnixStorage::new(temp.0.join("alias.sqlite")).unwrap(),
        JournalLimits::default()
    )
    .is_err());
    fs::hard_link(&path, temp.0.join("hard.sqlite")).unwrap();
    assert!(Pager::open(UnixStorage::new(&path).unwrap(), JournalLimits::default()).is_err());
    fs::remove_file(temp.0.join("hard.sqlite")).unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    let mut c = open(&path);
    fs::rename(&path, temp.0.join("renamed.sqlite")).unwrap();
    fs::write(&path, b"replacement").unwrap();
    assert!(c.execute("INSERT INTO t VALUES(2,'never')", &[]).is_err());
    drop(c);
    assert_eq!(fs::read(&path).unwrap(), b"replacement");
    assert_eq!(fs::read(temp.0.join("renamed.sqlite")).unwrap(), original);
}
#[test]
fn bounded_reads_and_storage_lifetime() {
    let temp = Temp::new();
    let path = temp.db();
    seed(&path);
    let mut s = UnixStorage::new(&path).unwrap();
    assert!(s.read_database(8192).is_err());
    s.lock_exclusive().unwrap();
    assert!(matches!(s.read_database(100), Err(Error::Limit(_))));
    assert!(s.lock_exclusive().is_err());
    s.unlock();
    s.unlock();
    assert!(s.read_database(8192).is_err());
    s.lock_exclusive().unwrap();
    s.unlock();
}

#[test]
fn racing_threads_get_only_one_connection_for_an_inode() {
    let temp = Temp::new();
    let path = temp.db();
    seed(&path);
    let start = std::sync::Arc::new(std::sync::Barrier::new(8));
    let attempted = std::sync::Arc::new(std::sync::Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let path = path.clone();
            let start = start.clone();
            let attempted = attempted.clone();
            std::thread::spawn(move || {
                let storage = UnixStorage::new(path).unwrap();
                start.wait();
                let connection = Pager::open(storage, JournalLimits::default());
                let won = connection.is_ok();
                if !won {
                    assert!(matches!(connection, Err(Error::Busy(_))));
                }
                attempted.wait();
                drop(connection);
                won
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .map(|t| usize::from(t.join().unwrap()))
            .sum::<usize>(),
        1
    );
}
