#![forbid(unsafe_code)]
use sqlite_safe::{
    sql::{Connection, SqlLimits},
    AutoVacuum, Encoding, Error, ImageBuilder, Table, Text, Value,
};
fn i(n: i64) -> Value {
    Value::Integer(n)
}
fn text(s: &str) -> Value {
    Value::Text(Text::utf8(s))
}
fn seq(db: &mut Connection) -> Vec<Vec<Value>> {
    db.execute(
        "SELECT rowid AS rid,name,seq FROM sqlite_sequence ORDER BY rowid",
        &[],
    )
    .unwrap()
    .rows
}

#[test]
fn prepared_allocation_keeps_high_water_and_consumes_ignored_attempts() {
    let mut db = Connection::new();
    db.execute(
        "CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,x UNIQUE)",
        &[],
    )
    .unwrap();
    assert!(seq(&mut db).is_empty());
    let p = db
        .prepare("INSERT OR IGNORE INTO t(x) VALUES(?1) RETURNING id")
        .unwrap();
    assert_eq!(
        db.execute_prepared(&p, &[text("a")]).unwrap().rows,
        vec![vec![i(1)]]
    );
    assert!(db
        .execute_prepared(&p, &[text("a")])
        .unwrap()
        .rows
        .is_empty());
    assert_eq!(seq(&mut db), vec![vec![i(1), text("t"), i(2)]]);
    db.execute("DELETE FROM t", &[]).unwrap();
    assert_eq!(
        db.execute_prepared(&p, &[text("b")]).unwrap().rows,
        vec![vec![i(3)]]
    );
    assert_eq!(
        (db.changes(), db.total_changes(), db.last_insert_rowid()),
        (1, 3, 3)
    );
    db.execute("UPDATE t SET id=100", &[]).unwrap();
    assert_eq!(seq(&mut db)[0][2], i(3));
    db.execute("INSERT INTO t VALUES(4,'c')", &[]).unwrap();
    assert_eq!(seq(&mut db)[0][2], i(4));
    assert_eq!(
        db.execute_prepared(&p, &[text("d")]).unwrap().rows,
        vec![vec![i(101)]]
    );
}

#[test]
fn sequence_writes_happen_after_returning_and_only_on_statement_success() {
    let mut db = Connection::new();
    db.execute_batch(
        "CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,x UNIQUE);INSERT INTO t(x) VALUES(1);",
    )
    .unwrap();
    let r=db.execute("INSERT INTO t(x) VALUES(2),(3) RETURNING id,(SELECT seq FROM sqlite_sequence WHERE name='t')",&[]).unwrap();
    assert_eq!(r.rows, vec![vec![i(2), i(1)], vec![i(3), i(1)]]);
    assert_eq!(seq(&mut db)[0][2], i(3));
    assert!(matches!(
        db.execute("INSERT OR FAIL INTO t(x) VALUES(4),(1),(5)", &[]),
        Err(Error::Constraint(_))
    ));
    assert_eq!(
        (db.changes(), db.total_changes(), db.last_insert_rowid()),
        (1, 4, 4)
    );
    assert_eq!(seq(&mut db)[0][2], i(3));
    db.execute("INSERT INTO t(x) VALUES(1) ON CONFLICT(x) DO NOTHING", &[])
        .unwrap();
    assert_eq!(seq(&mut db)[0][2], i(5));
    db.execute(
        "INSERT INTO t(x) VALUES(1) ON CONFLICT(x) DO UPDATE SET id=50",
        &[],
    )
    .unwrap();
    assert_eq!(seq(&mut db)[0][2], i(6));
    assert_eq!(db.last_insert_rowid(), 4);
    db.execute("INSERT INTO t(x) SELECT 5 WHERE 0", &[])
        .unwrap();
    assert_eq!(seq(&mut db)[0][2], i(6));
}

#[test]
fn savepoints_and_full_rowid_exhaustion_restore_the_right_transaction_state() {
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,x);INSERT INTO t VALUES(1,1);BEGIN;INSERT INTO t VALUES(10,10);SAVEPOINT s;INSERT INTO t VALUES(20,20);ROLLBACK TO s;").unwrap();
    assert_eq!(seq(&mut db)[0][2], i(10));
    db.execute("INSERT INTO t DEFAULT VALUES", &[]).unwrap();
    assert_eq!(db.last_insert_rowid(), 11);
    db.execute("ROLLBACK", &[]).unwrap();
    assert_eq!(seq(&mut db)[0][2], i(1));
    db.execute_batch("BEGIN;INSERT INTO t VALUES(9223372036854775807,99);")
        .unwrap();
    assert_eq!(
        db.execute("INSERT OR IGNORE INTO t DEFAULT VALUES", &[]),
        Err(Error::Full)
    );
    assert!(db.is_autocommit());
    assert_eq!(db.changes(), 0);
    assert_eq!(db.last_insert_rowid(), i64::MAX);
    assert_eq!(seq(&mut db)[0][2], i(1));
    assert_eq!(
        db.execute("SELECT * FROM t", &[]).unwrap().rows,
        vec![vec![i(1), i(1)]]
    );
    assert!(db.execute("ROLLBACK TO s", &[]).is_err());
}

#[test]
fn editable_sequence_rows_preserve_types_duplicates_and_table_lifecycle() {
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE t(id INTEGER,x,PRIMARY KEY(id DESC AUTOINCREMENT));INSERT INTO sqlite_sequence VALUES('t','50x'),('t',100),('T',200),(x'74',300);INSERT INTO t VALUES(-1,1);").unwrap();
    assert_eq!(seq(&mut db)[0][2], text("50x"));
    db.execute("INSERT INTO t DEFAULT VALUES", &[]).unwrap();
    assert_eq!(db.last_insert_rowid(), 51);
    assert_eq!(seq(&mut db)[0][2], i(51));
    assert_eq!(seq(&mut db)[1][2], i(100));
    let before = db.to_image(512).unwrap();
    for sql in [
        "DROP TABLE sqlite_sequence",
        "CREATE INDEX idx ON sqlite_sequence(name)",
        "CREATE TABLE IF NOT EXISTS sqlite_sequence(name,seq)",
    ] {
        assert!(db.execute(sql, &[]).is_err());
        assert_eq!(db.to_image(512).unwrap(), before);
    }
    db.execute("DROP TABLE t", &[]).unwrap();
    assert_eq!(
        seq(&mut db),
        vec![
            vec![i(3), text("T"), i(200)],
            vec![i(4), Value::Blob(vec![b't']), i(300)]
        ]
    );
    db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT)", &[])
        .unwrap();
    db.execute("INSERT INTO t SELECT 1 WHERE 0", &[]).unwrap();
    assert_eq!(seq(&mut db)[2], vec![i(5), text("t"), i(0)]);
}

#[test]
fn sequence_updates_obey_budgets_and_schema_validation_is_atomic() {
    let mut db = Connection::with_limits(SqlLimits {
        max_rows: 1,
        ..Default::default()
    });
    db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT)", &[])
        .unwrap();
    let before = db.to_image(512).unwrap();
    assert!(matches!(
        db.execute("INSERT INTO t DEFAULT VALUES", &[]),
        Err(Error::Limit(_))
    ));
    assert_eq!(db.to_image(512).unwrap(), before);
    assert_eq!(
        (db.changes(), db.total_changes(), db.last_insert_rowid()),
        (0, 0, 1)
    );
    let mut db = Connection::with_limits(SqlLimits {
        max_steps: 64,
        ..Default::default()
    });
    db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT)", &[])
        .unwrap();
    for n in 0..80 {
        db.execute(
            "INSERT INTO sqlite_sequence VALUES(?1,0)",
            &[text(&format!("other{n}"))],
        )
        .unwrap();
    }
    let before = db.to_image(512).unwrap();
    assert!(matches!(
        db.execute("INSERT INTO t DEFAULT VALUES", &[]),
        Err(Error::Limit(_))
    ));
    assert_eq!(db.to_image(512).unwrap(), before);
    assert!(matches!(
        db.execute("DROP TABLE t", &[]),
        Err(Error::Limit(_))
    ));
    assert_eq!(db.to_image(512).unwrap(), before);
    for sql in [
        "CREATE TABLE t(id INT PRIMARY KEY AUTOINCREMENT)",
        "CREATE TABLE t(id INTEGER PRIMARY KEY DESC AUTOINCREMENT)",
        "CREATE TABLE t(id INTEGER,x,PRIMARY KEY(id,x AUTOINCREMENT))",
        "CREATE TABLE t(id INTEGER AUTOINCREMENT)",
    ] {
        let mut db = Connection::new();
        assert!(db.execute(sql, &[]).is_err());
        assert!(db.execute("SELECT * FROM sqlite_sequence", &[]).is_err());
        for end in 0..=sql.len() {
            assert!(std::panic::catch_unwind(|| Connection::new().prepare(&sql[..end])).is_ok());
        }
    }
    assert!(ImageBuilder::new(512)
        .unwrap()
        .add_table(Table::new("sqlite_sequence", &["name", "seq"]))
        .is_err());
}

#[test]
fn sequence_and_its_empty_schema_survive_encoded_snapshots() {
    for encoding in [Encoding::Utf8, Encoding::Utf16Le, Encoding::Utf16Be] {
        for mode in [AutoVacuum::None, AutoVacuum::Full, AutoVacuum::Incremental] {
            let image = ImageBuilder::new(512)
                .unwrap()
                .encoding(encoding)
                .unwrap()
                .auto_vacuum(mode)
                .finish()
                .unwrap();
            let mut db = Connection::from_image(&image).unwrap();
            db.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,x TEXT UNIQUE);INSERT INTO t VALUES(100,'🦀');DELETE FROM t;").unwrap();
            let mut db = Connection::from_image(&db.to_image(512).unwrap()).unwrap();
            db.execute("INSERT INTO t(x) VALUES('é')", &[]).unwrap();
            assert_eq!(db.last_insert_rowid(), 101);
            db.execute("DROP TABLE t", &[]).unwrap();
            let mut db = Connection::from_image(&db.to_image(512).unwrap()).unwrap();
            assert!(seq(&mut db).is_empty());
            db.execute_batch("CREATE TABLE u(id INTEGER PRIMARY KEY AUTOINCREMENT);INSERT INTO u DEFAULT VALUES;").unwrap();
            assert_eq!(seq(&mut db), vec![vec![i(1), text("u"), i(1)]]);
        }
    }
}
