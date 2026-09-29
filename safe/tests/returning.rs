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
fn prepared_returning_preserves_column_names_bindings_and_empty_metadata() {
    let mut db = Connection::new();
    db.execute(
        "CREATE TABLE t(ID INTEGER PRIMARY KEY,x NUMERIC DEFAULT 7,y TEXT DEFAULT 'a')",
        &[],
    )
    .unwrap();
    let p = db
        .prepare("INSERT INTO t(x,y) VALUES(?1,?2) RETURNING rowid,t.X,y AS label,x+1 AS next")
        .unwrap();
    for (id, x) in [(1, "12"), (2, "24")] {
        let r = db.execute_prepared(&p, &[text(x), text("🦀")]).unwrap();
        assert_eq!(r.columns, ["ID", "x", "label", "next"]);
        let n = x.parse::<i64>().unwrap();
        assert_eq!(r.rows, vec![vec![i(id), i(n), text("🦀"), i(n + 1)]]);
        assert_eq!(r.changes, 1);
    }
    let r = db
        .execute("INSERT INTO t DEFAULT VALUES RETURNING *", &[])
        .unwrap();
    assert_eq!(r.rows, vec![vec![i(3), i(7), text("a")]]);
    let r = db
        .execute("UPDATE t SET x=0 WHERE 0 RETURNING oid,*,x AS again", &[])
        .unwrap();
    assert_eq!(r.columns, ["ID", "ID", "x", "y", "again"]);
    assert!(r.rows.is_empty());
    assert_eq!(r.changes, 0);
    db.execute("INSERT INTO t(x) VALUES(9)", &[]).unwrap();
    for sql in [
        "DELETE FROM t WHERE 0 RETURNING missing",
        "INSERT INTO t DEFAULT VALUES RETURNING t.*",
        "UPDATE t SET x=0 RETURNING sum(x)",
        "INSERT INTO t AS dst DEFAULT VALUES RETURNING dst.x",
    ] {
        assert!(db.execute(sql, &[]).is_err());
        assert_eq!(db.changes(), 1);
        assert_eq!(db.total_changes(), 4);
    }
}

#[test]
fn returned_rows_follow_successful_changes_and_keep_statement_counter_timing() {
    let mut db = Connection::new();
    db.execute_batch(
        "CREATE TABLE t(id INTEGER PRIMARY KEY,x UNIQUE);INSERT INTO t VALUES(1,10),(2,20);",
    )
    .unwrap();
    let r=db.execute("INSERT OR IGNORE INTO t VALUES(3,10),(4,40),(5,50) RETURNING id,changes(),total_changes(),last_insert_rowid()",&[]).unwrap();
    assert_eq!(
        r.rows,
        vec![vec![i(4), i(2), i(2), i(4)], vec![i(5), i(2), i(2), i(5)]]
    );
    assert_eq!(r.changes, 2);
    let r=db.execute("INSERT INTO t VALUES(1,11),(6,60),(2,22) ON CONFLICT(id) DO UPDATE SET x=excluded.x RETURNING id,x,last_insert_rowid(),(SELECT last_insert_rowid())",&[]).unwrap();
    assert_eq!(
        r.rows,
        vec![
            vec![i(1), i(11), i(5), i(5)],
            vec![i(6), i(60), i(6), i(6)],
            vec![i(2), i(22), i(6), i(5)]
        ]
    );
    assert_eq!(
        (db.changes(), db.total_changes(), db.last_insert_rowid()),
        (3, 7, 6)
    );
    assert!(db
        .execute(
            "INSERT INTO t VALUES(1,99) ON CONFLICT DO NOTHING RETURNING *",
            &[]
        )
        .unwrap()
        .rows
        .is_empty());
    let r = db
        .execute(
            "UPDATE t AS dst SET x=dst.x+100 WHERE dst.id<3 RETURNING t.id,x",
            &[],
        )
        .unwrap();
    assert_eq!(r.rows, vec![vec![i(1), i(111)], vec![i(2), i(122)]]);
    let r = db
        .execute(
            "DELETE FROM t AS dst WHERE dst.id<3 RETURNING t.id,x,(SELECT count(*) FROM t)",
            &[],
        )
        .unwrap();
    assert_eq!(
        r.rows,
        vec![vec![i(1), i(111), i(4)], vec![i(2), i(122), i(3)]]
    );
}

#[test]
fn subquery_caches_distinguish_direct_reads_and_explicit_materialization() {
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,x);INSERT INTO t VALUES(1,1),(2,2),(3,3);CREATE VIEW v AS SELECT * FROM t;").unwrap();
    let r=db.execute("UPDATE t SET x=x+10 RETURNING x,(SELECT sum(x) FROM t),(SELECT sum(x) FROM v),(SELECT (SELECT sum(x) FROM t))",&[]).unwrap();
    assert_eq!(
        r.rows,
        vec![
            vec![i(11), i(16), i(16), i(16)],
            vec![i(12), i(26), i(16), i(16)],
            vec![i(13), i(36), i(16), i(16)]
        ]
    );
    for hint in ["", "MATERIALIZED", "NOT MATERIALIZED"] {
        let mut db = Connection::new();
        db.execute_batch(
            "CREATE TABLE t(id INTEGER PRIMARY KEY,x);INSERT INTO t VALUES(1,1),(2,2),(3,3);",
        )
        .unwrap();
        let r=db.execute(&format!("WITH q AS {hint}(SELECT sum(x) s FROM t) UPDATE t SET x=(SELECT s FROM q) RETURNING x,(SELECT s FROM q)"),&[]).unwrap();
        assert_eq!(
            r.rows,
            vec![vec![i(6), i(if hint == "MATERIALIZED" { 6 } else { 11 })]; 3]
        );
        let mut db = Connection::new();
        db.execute_batch(
            "CREATE TABLE t(id INTEGER PRIMARY KEY,x);INSERT INTO t VALUES(1,1),(2,2),(3,3);",
        )
        .unwrap();
        let r=db.execute(&format!("WITH q AS {hint}(SELECT sum(x) s FROM t) UPDATE t SET x=CASE id WHEN 1 THEN 10 ELSE (SELECT s FROM q) END RETURNING x,(SELECT s FROM q)"),&[]).unwrap();
        assert_eq!(
            r.rows,
            vec![vec![i(10), i(15)], vec![i(15), i(15)], vec![i(15), i(15)]]
        );
        let mut db = Connection::new();
        db.execute_batch(
            "CREATE TABLE t(id INTEGER PRIMARY KEY,x);INSERT INTO t VALUES(1,1),(2,2);",
        )
        .unwrap();
        let r = db.execute(&format!("WITH q AS {hint}(SELECT sum(x) s FROM t) INSERT INTO t VALUES(1,40),(3,3),(1,50),(4,4) ON CONFLICT(id) DO UPDATE SET x=excluded.x RETURNING id,(SELECT s FROM q)"), &[]).unwrap();
        let inserted = if hint == "NOT MATERIALIZED" { 45 } else { 42 };
        assert_eq!(
            r.rows,
            vec![
                vec![i(1), i(42)],
                vec![i(3), i(inserted)],
                vec![i(1), i(42)],
                vec![i(4), i(inserted)]
            ]
        );
    }
}

#[test]
fn errors_discard_results_but_fail_keeps_its_written_prefix() {
    for policy in ["ABORT", "FAIL", "ROLLBACK"] {
        let mut db = Connection::new();
        db.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,x UNIQUE);INSERT INTO t VALUES(1,1);BEGIN;INSERT INTO t VALUES(10,10);SAVEPOINT s;").unwrap();
        let before = db.to_image(512).unwrap();
        assert!(matches!(
            db.execute(
                &format!("INSERT OR {policy} INTO t VALUES(2,2),(3,1),(4,4) RETURNING *"),
                &[]
            ),
            Err(Error::Constraint(_))
        ));
        assert_eq!(db.last_insert_rowid(), 2);
        assert_eq!(db.is_autocommit(), policy == "ROLLBACK");
        assert_eq!(db.changes(), usize::from(policy == "FAIL"));
        if policy == "ABORT" {
            assert_eq!(db.to_image(512).unwrap(), before);
        }
        let rows = db
            .execute("SELECT id FROM t ORDER BY id", &[])
            .unwrap()
            .rows;
        assert_eq!(
            rows,
            match policy {
                "FAIL" => vec![vec![i(1)], vec![i(2)], vec![i(10)]],
                "ROLLBACK" => vec![vec![i(1)]],
                _ => vec![vec![i(1)], vec![i(10)]],
            }
        );
    }
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,x);INSERT INTO t VALUES(1,1),(2,2);")
        .unwrap();
    let before = db.to_image(512).unwrap();
    assert!(db
        .execute(
            "DELETE FROM t RETURNING abs(CASE id WHEN 2 THEN -9223372036854775808 ELSE x END)",
            &[]
        )
        .is_err());
    assert_eq!(db.to_image(512).unwrap(), before);
    assert_eq!((db.changes(), db.total_changes()), (0, 2));
}

#[test]
fn budgets_roll_back_changes_and_malformed_returning_never_panics() {
    let mut db = Connection::with_limits(SqlLimits {
        max_database_bytes: 1024,
        ..Default::default()
    });
    db.execute("CREATE TABLE t(x)", &[]).unwrap();
    let before = db.to_image(512).unwrap();
    assert!(matches!(
        db.execute(
            "INSERT INTO t VALUES(?1) RETURNING x,x,x,x",
            &[text(&"a".repeat(300))]
        ),
        Err(Error::Limit(_))
    ));
    assert_eq!(db.to_image(512).unwrap(), before);
    assert_eq!(
        (db.changes(), db.total_changes(), db.last_insert_rowid()),
        (0, 0, 1)
    );
    for sql in [
        "INSERT INTO t VALUES(1) RETURNING x,(SELECT x FROM t)",
        "UPDATE t SET x=1 RETURNING x AS value,*",
        "DELETE FROM t RETURNING CASE WHEN x THEN x ELSE NULL END",
    ] {
        for end in 0..=sql.len() {
            assert!(std::panic::catch_unwind(|| Connection::new().prepare(&sql[..end])).is_ok());
        }
    }
    let wide = vec!["x"; 2001].join(",");
    assert!(matches!(
        db.execute(&format!("INSERT INTO t VALUES(1) RETURNING {wide}"), &[]),
        Err(Error::Limit(_))
    ));
    assert_eq!(db.to_image(512).unwrap(), before);
}

#[test]
fn returned_values_survive_encoded_and_auto_vacuum_roundtrips() {
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
            db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY,x TEXT UNIQUE)", &[])
                .unwrap();
            let r = db
                .execute("INSERT INTO t(x) VALUES('🦀'),('é') RETURNING *", &[])
                .unwrap();
            let mut db = Connection::from_image(&db.to_image(512).unwrap()).unwrap();
            assert_eq!(
                db.execute("SELECT * FROM t ORDER BY id", &[]).unwrap().rows,
                r.rows
            );
            let r = db.execute("DELETE FROM t RETURNING *", &[]).unwrap();
            assert_eq!(r.rows, vec![vec![i(1), text("🦀")], vec![i(2), text("é")]]);
        }
    }
}
