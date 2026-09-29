use sqlite_rust::{Connection, Value};
use std::sync::atomic::{AtomicU64, Ordering};

struct TestFile(std::path::PathBuf);
impl TestFile {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
            "sqlite-rust-{}-{}.db",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for TestFile {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}
fn scalar(db: &Connection, sql: &str) -> Value {
    db.query(sql, &[]).unwrap().rows[0][0].clone()
}

#[test]
fn typed_parameters_roundtrip_without_losing_nuls_or_empty_blobs() {
    let db = Connection::open_in_memory().unwrap();
    let values = vec![
        Value::Null,
        Value::Integer(i64::MIN),
        Value::Integer(i64::MAX),
        Value::Real(-1234.125),
        Value::Text("Rust\0SQLite 🦀 İstanbul".into()),
        Value::Blob(vec![0, 255, 128, 0]),
        Value::Blob(vec![]),
        Value::Text("".into()),
    ];
    let result = db
        .query("SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8", &values)
        .unwrap();
    assert_eq!(result.rows, vec![values]);
    assert_eq!(
        scalar(&db, "SELECT typeof(x'')"),
        Value::Text("blob".into())
    );
}

#[test]
fn statements_reset_bind_copy_and_report_errors() {
    let db = Connection::open_in_memory().unwrap();
    let mut stmt = db.prepare("SELECT :value; -- trailing comment").unwrap();
    assert_eq!(stmt.parameter_count(), 1);
    stmt.bind(1, &Value::Text("owned copy".into())).unwrap();
    assert_eq!(
        stmt.step().unwrap(),
        Some(vec![Value::Text("owned copy".into())])
    );
    assert_eq!(stmt.step().unwrap(), None);
    assert_eq!(stmt.step().unwrap(), None);
    stmt.reset().unwrap();
    stmt.clear_bindings().unwrap();
    assert_eq!(stmt.step().unwrap(), Some(vec![Value::Null]));
    assert!(stmt.bind(0, &Value::Null).is_err());
    assert!(stmt.bind(usize::MAX, &Value::Null).is_err());
    assert!(db.query("SELECT ?", &[]).is_err());
    assert!(db.prepare("SELECT 1; SELECT 2").is_err());
    assert!(db.prepare(" -- comment only").is_err());
    assert!(db.prepare("SELECT '\0'").is_err());
    assert!(db.query("SELECT CAST(x'FF' AS TEXT)", &[]).is_err());
    assert_eq!(
        scalar(&db, "SELECT CAST(CAST(x'FF' AS TEXT) AS BLOB)"),
        Value::Blob(vec![255])
    );
}

#[test]
fn transactions_commit_rollback_and_rollback_on_drop() {
    let mut db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE t(x UNIQUE)").unwrap();
    {
        let tx = db.transaction().unwrap();
        tx.execute_batch("INSERT INTO t VALUES(1)").unwrap();
    }
    assert_eq!(scalar(&db, "SELECT count(*) FROM t"), Value::Integer(0));
    {
        let tx = db.transaction().unwrap();
        tx.execute_batch("INSERT INTO t VALUES(2)").unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(scalar(&db, "SELECT x FROM t"), Value::Integer(2));
    assert!(db.is_autocommit());
    db.execute_batch("SAVEPOINT s; INSERT INTO t VALUES(3); ROLLBACK TO s; RELEASE s")
        .unwrap();
    assert_eq!(scalar(&db, "SELECT count(*) FROM t"), Value::Integer(1));
    let err = db.execute_batch("INSERT INTO t VALUES(2)").unwrap_err();
    assert_eq!(err.code & 255, 19);
}

#[test]
fn failed_deferred_commit_rolls_back_the_guard() {
    let mut db = Connection::open_in_memory().unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE p(id PRIMARY KEY); CREATE TABLE c(id REFERENCES p DEFERRABLE INITIALLY DEFERRED)").unwrap();
    let tx = db.transaction().unwrap();
    tx.execute_batch("INSERT INTO c VALUES(1)").unwrap();
    assert!(tx.commit().is_err());
    assert!(db.is_autocommit());
    assert_eq!(scalar(&db, "SELECT count(*) FROM c"), Value::Integer(0));
}

#[test]
fn disk_btrees_overflow_pages_indexes_reopen_and_vacuum() {
    let file = TestFile::new();
    {
        let db = Connection::open(&file.0).unwrap();
        db.execute_batch("PRAGMA page_size=512; CREATE TABLE t(id INTEGER PRIMARY KEY, payload BLOB, label TEXT); CREATE INDEX labels ON t(label); BEGIN; WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1500) INSERT INTO t SELECT x, zeroblob(2048+x%20), printf('row-%06d',x) FROM n; COMMIT; DELETE FROM t WHERE id%3=0; VACUUM").unwrap();
        assert_eq!(
            scalar(&db, "PRAGMA integrity_check"),
            Value::Text("ok".into())
        );
    }
    let db = Connection::open(&file.0).unwrap();
    assert_eq!(scalar(&db, "SELECT count(*) FROM t"), Value::Integer(1000));
    assert_eq!(
        scalar(&db, "SELECT id FROM t WHERE label='row-000002'"),
        Value::Integer(2)
    );
    assert_eq!(
        scalar(&db, "SELECT length(payload) FROM t WHERE id=2"),
        Value::Integer(2050)
    );
    assert_eq!(&std::fs::read(&file.0).unwrap()[..16], b"SQLite format 3\0");
}

#[test]
fn wal_snapshot_isolation_busy_writer_and_checkpoint() {
    let file = TestFile::new();
    let a = Connection::open(&file.0).unwrap();
    a.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE t(x); INSERT INTO t VALUES(1)")
        .unwrap();
    let b = Connection::open(&file.0).unwrap();
    a.execute_batch("BEGIN").unwrap();
    assert_eq!(scalar(&a, "SELECT count(*) FROM t"), Value::Integer(1));
    b.execute_batch("INSERT INTO t VALUES(2)").unwrap();
    assert_eq!(scalar(&a, "SELECT count(*) FROM t"), Value::Integer(1));
    a.execute_batch("COMMIT; BEGIN IMMEDIATE").unwrap();
    assert_eq!(
        b.execute_batch("BEGIN IMMEDIATE").unwrap_err().code & 255,
        5
    );
    a.execute_batch("ROLLBACK").unwrap();
    assert_eq!(scalar(&a, "SELECT count(*) FROM t"), Value::Integer(2));
    assert_eq!(
        scalar(&a, "PRAGMA integrity_check"),
        Value::Text("ok".into())
    );
    a.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
}

#[test]
fn independent_connections_write_from_threads() {
    let file = TestFile::new();
    let db = Connection::open(&file.0).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE t(worker, n)")
        .unwrap();
    let threads: Vec<_> = (0..4)
        .map(|worker| {
            let path = file.0.clone();
            std::thread::spawn(move || {
                let db = Connection::open(path).unwrap();
                db.busy_timeout(5000).unwrap();
                for n in 0..40 {
                    db.execute(
                        "INSERT INTO t VALUES(?,?)",
                        &[Value::Integer(worker), Value::Integer(n)],
                    )
                    .unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(scalar(&db, "SELECT count(*) FROM t"), Value::Integer(160));
    assert_eq!(
        scalar(&db, "PRAGMA integrity_check"),
        Value::Text("ok".into())
    );
}

#[test]
fn sql_features_extensions_and_virtual_tables() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE VIRTUAL TABLE docs USING fts5(body); INSERT INTO docs VALUES('the rust database'),('another database'); CREATE VIRTUAL TABLE bounds USING rtree(id,x1,x2,y1,y2); INSERT INTO bounds VALUES(1,0,10,0,10),(2,20,30,20,30)").unwrap();
    assert_eq!(
        scalar(&db, "SELECT count(*) FROM docs WHERE docs MATCH 'rust'"),
        Value::Integer(1)
    );
    assert_eq!(
        scalar(&db, "SELECT id FROM bounds WHERE x1<=5 AND x2>=5"),
        Value::Integer(1)
    );
    assert_eq!(
        scalar(&db, "SELECT json_extract(jsonb('{\"a\":[1,2]}'),'$.a[1]')"),
        Value::Integer(2)
    );
    assert_eq!(scalar(&db, "SELECT sqrt(16)"), Value::Real(4.0));
    assert_eq!(
        scalar(
            &db,
            "WITH t(x) AS (VALUES(1),(5),(9)) SELECT median(x) FROM t"
        ),
        Value::Real(5.0)
    );
    assert_eq!(
        scalar(&db, "SELECT count(*)>0 FROM bytecode('SELECT 42')"),
        Value::Integer(1)
    );
    assert_eq!(
        scalar(&db, "SELECT count(*)>0 FROM dbstat"),
        Value::Integer(1)
    );
    db.execute_batch(
        "CREATE VIRTUAL TABLE legacy USING fts4(body); INSERT INTO legacy VALUES('hello rust')",
    )
    .unwrap();
    assert_eq!(
        scalar(&db, "SELECT count(*) FROM legacy WHERE legacy MATCH 'rust'"),
        Value::Integer(1)
    );
}

#[test]
fn triggers_generated_columns_windows_ctes_and_upsert() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, n INTEGER, doubled INTEGER GENERATED ALWAYS AS(n*2) STORED) STRICT; CREATE TABLE log(x); CREATE TRIGGER audit AFTER INSERT ON t BEGIN INSERT INTO log VALUES(new.id); END; WITH RECURSIVE s(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM s WHERE n<10) INSERT INTO t(id,n) SELECT n,n FROM s; INSERT INTO t(id,n) VALUES(1,20) ON CONFLICT(id) DO UPDATE SET n=excluded.n").unwrap();
    assert_eq!(
        scalar(&db, "SELECT doubled FROM t WHERE id=1"),
        Value::Integer(40)
    );
    assert_eq!(scalar(&db, "SELECT count(*) FROM log"), Value::Integer(10));
    let rows = db.query("SELECT id, sum(n) OVER(ORDER BY id ROWS UNBOUNDED PRECEDING) FROM t ORDER BY id LIMIT 2", &[]).unwrap().rows;
    assert_eq!(
        rows,
        vec![
            vec![Value::Integer(1), Value::Integer(20)],
            vec![Value::Integer(2), Value::Integer(22)]
        ]
    );
    assert!(db
        .execute_batch("INSERT INTO t(n) VALUES('not an integer')")
        .is_err());
}

#[test]
fn attach_and_without_rowid() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("ATTACH ':memory:' AS other; CREATE TABLE other.t(a TEXT,b INTEGER,PRIMARY KEY(a,b)) WITHOUT ROWID; INSERT INTO other.t VALUES('key',1); CREATE TABLE main.t AS SELECT * FROM other.t; DETACH other").unwrap();
    assert_eq!(scalar(&db, "SELECT a FROM t"), Value::Text("key".into()));
}
