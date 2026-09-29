#![forbid(unsafe_code)]
use sqlite_safe::{
    sql::{Connection, SqlLimits},
    AutoVacuum, Encoding, Error, ImageBuilder, Text, Value,
};
fn i(n: i64) -> Value {
    Value::Integer(n)
}
fn text(s: &str) -> Value {
    Value::Text(Text::utf8(s))
}

#[test]
fn prepared_upsert_binds_excluded_and_correlated_values_on_each_execution() {
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,x TEXT COLLATE NOCASE UNIQUE,v NUMERIC);INSERT INTO t VALUES(1,'A',10);").unwrap();
    let p = db.prepare("INSERT INTO t(x,v) VALUES(?1,?2) ON CONFLICT(x) DO UPDATE SET v=(SELECT t.v+excluded.v) WHERE excluded.v>0").unwrap();
    for (name, value, changes) in [("a", 2, 1), ("B", 5, 1), ("b", -3, 0)] {
        assert_eq!(
            db.execute_prepared(&p, &[text(name), i(value)])
                .unwrap()
                .changes,
            changes
        );
    }
    assert_eq!(
        db.execute("SELECT * FROM t ORDER BY id", &[]).unwrap().rows,
        vec![vec![i(1), text("A"), i(12)], vec![i(2), text("B"), i(5)]]
    );
    assert_eq!(
        (db.changes(), db.total_changes(), db.last_insert_rowid()),
        (0, 3, 2)
    );
    db.execute(
        "INSERT INTO t AS excluded VALUES(1,'z',999) ON CONFLICT(id) DO UPDATE SET v=excluded.v",
        &[],
    )
    .unwrap();
    assert_eq!(
        db.execute("SELECT v FROM t WHERE id=1", &[]).unwrap().rows,
        vec![vec![i(12)]]
    );
}

#[test]
fn ordered_targets_choose_one_row_and_invalid_targets_preserve_counters() {
    let mut db = Connection::new();
    db.execute_batch(
        "CREATE TABLE t(id INTEGER PRIMARY KEY,x UNIQUE,v);INSERT INTO t VALUES(1,10,1),(2,20,2);",
    )
    .unwrap();
    db.execute("INSERT INTO t VALUES(1,20,99) ON CONFLICT(x) DO UPDATE SET v=excluded.v ON CONFLICT(id) DO UPDATE SET v=-1", &[]).unwrap();
    assert_eq!(
        db.execute("SELECT * FROM t ORDER BY id", &[]).unwrap().rows,
        vec![vec![i(1), i(10), i(1)], vec![i(2), i(20), i(99)]]
    );
    db.execute("INSERT INTO t VALUES(1,20,99) ON CONFLICT(id) DO UPDATE SET id=3 ON CONFLICT(x) DO NOTHING", &[]).unwrap();
    assert_eq!(
        db.execute("SELECT id FROM t ORDER BY id", &[])
            .unwrap()
            .rows,
        vec![vec![i(2)], vec![i(3)]]
    );
    assert_eq!(db.last_insert_rowid(), 2);
    let before = db.to_image(512).unwrap();
    for sql in [
        "INSERT INTO t VALUES(9,9,9) ON CONFLICT(v) DO NOTHING",
        "INSERT INTO t VALUES(9,9,9) ON CONFLICT(id COLLATE BINARY) DO NOTHING",
        "INSERT INTO t VALUES(9,9,9) ON CONFLICT(id) DO UPDATE SET v=missing",
    ] {
        assert!(db.execute(sql, &[]).is_err());
        assert_eq!(db.changes(), 1);
        assert_eq!(db.to_image(512).unwrap(), before);
    }
    db.execute("INSERT INTO t VALUES(2,20,88) ON CONFLICT(id) DO NOTHING ON CONFLICT(id) DO UPDATE SET v=missing", &[]).unwrap();
    assert_eq!(db.changes(), 0);
}

#[test]
fn update_failure_always_aborts_statement_without_rolling_back_transaction() {
    for policy in ["IGNORE", "FAIL", "ROLLBACK", "REPLACE", "ABORT"] {
        let mut db = Connection::new();
        db.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,x UNIQUE,v NOT NULL ON CONFLICT REPLACE DEFAULT 7 CHECK(v>0));INSERT INTO t VALUES(1,10,1);BEGIN;INSERT INTO t VALUES(10,100,10);SAVEPOINT s;").unwrap();
        let before = db.to_image(512).unwrap();
        assert!(matches!(db.execute(&format!("INSERT OR {policy} INTO t VALUES(2,20,2),(3,10,3) ON CONFLICT(x) DO UPDATE SET v=NULL"), &[]),Err(Error::Constraint(_))));
        assert!(!db.is_autocommit());
        assert_eq!(db.to_image(512).unwrap(), before);
        assert_eq!(
            (db.changes(), db.total_changes(), db.last_insert_rowid()),
            (0, 2, 2)
        );
        db.execute("ROLLBACK TO s", &[]).unwrap();
        db.execute("COMMIT", &[]).unwrap();
    }
}

#[test]
fn upsert_preserves_encoded_schema_and_indexes_across_snapshot_roundtrips() {
    for encoding in [Encoding::Utf8, Encoding::Utf16Le, Encoding::Utf16Be] {
        for mode in [AutoVacuum::None, AutoVacuum::Full, AutoVacuum::Incremental] {
            let seed = ImageBuilder::new(512)
                .unwrap()
                .encoding(encoding)
                .unwrap()
                .auto_vacuum(mode)
                .finish()
                .unwrap();
            let mut db = Connection::from_image(&seed).unwrap();
            db.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,x TEXT UNIQUE,v INTEGER);INSERT INTO t VALUES(1,'🦀',1),(2,'é',2);CREATE VIEW v AS SELECT x,v FROM t;").unwrap();
            let mut db = Connection::from_image(&db.to_image(512).unwrap()).unwrap();
            db.execute("WITH q(x,v) AS(VALUES('🦀',8),('new',10)) INSERT INTO t(x,v) SELECT * FROM q WHERE 1 ON CONFLICT(x) DO UPDATE SET v=v+excluded.v", &[]).unwrap();
            let image = db.to_image(512).unwrap();
            let header = sqlite_safe::Database::parse(&image)
                .unwrap()
                .header()
                .clone();
            assert_eq!(header.encoding, encoding);
            assert_eq!(header.auto_vacuum, mode);
            let mut reopened = Connection::from_image(&image).unwrap();
            assert_eq!(
                reopened
                    .execute("SELECT v FROM v ORDER BY v", &[])
                    .unwrap()
                    .rows,
                vec![vec![i(2)], vec![i(9)], vec![i(10)]]
            );
        }
    }
}

#[test]
fn resource_failures_restore_prior_rows_and_malformed_clauses_do_not_panic() {
    // Resolving a wide target is work even when the SELECT inserts no rows.
    let mut limited = Connection::with_limits(SqlLimits {
        max_steps: 64,
        ..Default::default()
    });
    let columns = (0..20)
        .map(|n| format!("c{n}"))
        .collect::<Vec<_>>()
        .join(",");
    let reverse = (0..20)
        .rev()
        .map(|n| format!("c{n}"))
        .collect::<Vec<_>>()
        .join(",");
    limited
        .execute(
            &format!("CREATE TABLE wide({columns},UNIQUE({columns}))"),
            &[],
        )
        .unwrap();
    let source = vec!["0"; 20].join(",");
    assert!(matches!(
        limited.execute(
            &format!("INSERT INTO wide SELECT {source} WHERE 0 ON CONFLICT({reverse}) DO NOTHING"),
            &[]
        ),
        Err(Error::Limit(_))
    ));
    let mut db = Connection::with_limits(SqlLimits {
        max_rows: 2,
        ..Default::default()
    });
    db.execute_batch("CREATE TABLE t(x UNIQUE,v);INSERT INTO t VALUES(1,10),(2,20);")
        .unwrap();
    db.execute(
        "INSERT INTO t VALUES(1,30) ON CONFLICT(x) DO UPDATE SET v=excluded.v",
        &[],
    )
    .unwrap();
    let before = db.to_image(512).unwrap();
    assert!(matches!(
        db.execute(
            "INSERT OR FAIL INTO t VALUES(1,40),(3,30) ON CONFLICT(x) DO UPDATE SET v=excluded.v",
            &[]
        ),
        Err(Error::Limit(_))
    ));
    assert_eq!(db.to_image(512).unwrap(), before);
    let sql="INSERT INTO t AS dst VALUES(1,2),(2,3) ON CONFLICT(x) DO UPDATE SET v=(SELECT excluded.v+dst.v) WHERE excluded.v>0 ON CONFLICT DO NOTHING";
    for length in 0..=sql.len() {
        assert!(std::panic::catch_unwind(|| {
            let mut c = Connection::new();
            c.execute("CREATE TABLE t(x UNIQUE,v)", &[]).unwrap();
            let _ = c.execute(&sql[..length], &[]);
        })
        .is_ok());
    }
}
