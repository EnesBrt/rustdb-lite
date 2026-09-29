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
