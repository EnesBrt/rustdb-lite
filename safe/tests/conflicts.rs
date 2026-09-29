#![forbid(unsafe_code)]
use sqlite_safe::{
    sql::{Connection, SqlLimits},
    Error, Text, Value,
};
fn int(n: i64) -> Value {
    Value::Integer(n)
}
fn text(s: &str) -> Value {
    Value::Text(Text::utf8(s))
}

#[test]
fn abort_fail_and_rollback_have_distinct_statement_and_transaction_effects() {
    for (policy, expected, autocommit, changes, total) in [
        ("ABORT", vec![1, 10], false, 0, 2),
        ("FAIL", vec![1, 2, 10], false, 1, 3),
        ("ROLLBACK", vec![1], true, 0, 2),
    ] {
        let mut db = Connection::new();
        db.execute_batch("CREATE TABLE t(x INTEGER UNIQUE);INSERT INTO t VALUES(1);BEGIN;INSERT INTO t VALUES(10);SAVEPOINT s;").unwrap();
        assert!(matches!(
            db.execute(&format!("INSERT OR {policy} INTO t VALUES(2),(1),(3)"), &[]),
            Err(Error::Constraint(_))
        ));
        assert_eq!(
            db.execute("SELECT x FROM t ORDER BY x", &[]).unwrap().rows,
            expected
                .into_iter()
                .map(|n| vec![int(n)])
                .collect::<Vec<_>>()
        );
        assert_eq!(
            (db.is_autocommit(), db.changes(), db.total_changes()),
            (autocommit, changes, total)
        );
        assert_eq!(db.last_insert_rowid(), 3);
        assert_eq!(db.execute("ROLLBACK TO s", &[]).is_ok(), !autocommit);
    }
}

#[test]
fn ignore_and_replace_handle_keys_defaults_and_check_constraints() {
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,a TEXT UNIQUE,b TEXT UNIQUE,n NUMERIC NOT NULL DEFAULT '9' CHECK(n>0));INSERT INTO t VALUES(1,'a','x',1),(2,'b','y',2);INSERT OR IGNORE INTO t VALUES(3,'a','z',3),(4,'c','z',4);REPLACE INTO t VALUES(5,'a','y',NULL);").unwrap();
    assert_eq!(
        db.execute("SELECT id,a,b,n FROM t ORDER BY id", &[])
            .unwrap()
            .rows,
        vec![
            vec![int(4), text("c"), text("z"), int(4)],
            vec![int(5), text("a"), text("y"), int(9)]
        ]
    );
    assert_eq!(
        (db.changes(), db.total_changes(), db.last_insert_rowid()),
        (1, 4, 5)
    );
    assert!(db
        .execute("INSERT OR REPLACE INTO t VALUES(6,'c','y',-1)", &[])
        .is_err());
    assert_eq!(
        db.execute("SELECT count(*) FROM t", &[]).unwrap().rows,
        vec![vec![int(2)]]
    );
    db.execute("INSERT OR IGNORE INTO t VALUES(6,'d','w',-1)", &[])
        .unwrap();
    assert_eq!(db.changes(), 0);
}

#[test]
fn schema_policies_merge_override_and_roundtrip() {
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE t(a TEXT UNIQUE ON CONFLICT IGNORE,b NOT NULL ON CONFLICT REPLACE DEFAULT 3,UNIQUE(a));INSERT INTO t VALUES('a',NULL),('a',4);").unwrap();
    assert_eq!(db.changes(), 1);
    assert!(db
        .execute("INSERT OR ABORT INTO t VALUES('a',5)", &[])
        .is_err());
    db.execute("INSERT OR REPLACE INTO t VALUES('a',6)", &[])
        .unwrap();
    let mut loaded = Connection::from_image(&db.to_image(512).unwrap()).unwrap();
    loaded.execute("INSERT INTO t VALUES('a',7)", &[]).unwrap();
    assert_eq!(loaded.changes(), 0);
    assert_eq!(
        loaded.execute("SELECT a,b FROM t", &[]).unwrap().rows,
        vec![vec![text("a"), int(6)]]
    );
    assert!(db
        .execute(
            "CREATE TABLE bad(a UNIQUE ON CONFLICT IGNORE,UNIQUE(a) ON CONFLICT REPLACE)",
            &[]
        )
        .is_err());
    db.execute("CREATE TABLE bad(a)", &[]).unwrap();
}

#[test]
fn update_replace_does_not_resurrect_deleted_rows_and_fail_keeps_prefix() {
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,x UNIQUE);INSERT INTO t VALUES(1,1),(2,2),(3,3);UPDATE OR REPLACE t SET x=2 WHERE id=1;").unwrap();
    assert_eq!(
        db.execute("SELECT id,x FROM t ORDER BY id", &[])
            .unwrap()
            .rows,
        vec![vec![int(1), int(2)], vec![int(3), int(3)]]
    );
    db.execute("UPDATE OR REPLACE t SET x=3", &[]).unwrap();
    assert_eq!(db.changes(), 1);
    assert_eq!(
        db.execute("SELECT id,x FROM t", &[]).unwrap().rows,
        vec![vec![int(1), int(3)]]
    );
    db.execute_batch("DELETE FROM t;INSERT INTO t VALUES(1,1),(2,2),(3,3);")
        .unwrap();
    assert!(db
        .execute(
            "UPDATE OR FAIL t SET x=CASE id WHEN 1 THEN 10 ELSE 3 END",
            &[]
        )
        .is_err());
    assert_eq!(db.changes(), 1);
    assert_eq!(
        db.execute("SELECT id,x FROM t ORDER BY id", &[])
            .unwrap()
            .rows,
        vec![
            vec![int(1), int(10)],
            vec![int(2), int(2)],
            vec![int(3), int(3)]
        ]
    );
}

#[test]
fn not_null_replacement_second_pass_and_constraint_order() {
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE t(a NOT NULL ON CONFLICT REPLACE DEFAULT NULL,b NOT NULL ON CONFLICT IGNORE);INSERT INTO t VALUES(NULL,NULL);").unwrap();
    assert_eq!(db.changes(), 0);
    assert!(db.execute("INSERT INTO t VALUES(NULL,1)", &[]).is_err());
    db.execute_batch("CREATE TABLE u(id INTEGER PRIMARY KEY ON CONFLICT REPLACE,a UNIQUE ON CONFLICT IGNORE);INSERT INTO u VALUES(1,1),(2,2);INSERT INTO u VALUES(1,2);").unwrap();
    assert_eq!(
        db.execute("SELECT id,a FROM u ORDER BY id", &[])
            .unwrap()
            .rows,
        vec![vec![int(1), int(1)], vec![int(2), int(2)]]
    );
}

#[test]
fn resource_errors_abort_even_with_fail_or_ignore() {
    let mut db = Connection::with_limits(SqlLimits {
        max_rows: 2,
        ..Default::default()
    });
    db.execute_batch("CREATE TABLE t(x UNIQUE);INSERT INTO t VALUES(1),(2);")
        .unwrap();
    db.execute("INSERT OR REPLACE INTO t VALUES(1)", &[])
        .unwrap();
    assert!(matches!(
        db.execute("INSERT OR FAIL INTO t VALUES(3)", &[]),
        Err(Error::Limit(_))
    ));
    assert_eq!(
        db.execute("SELECT count(*) FROM t", &[]).unwrap().rows,
        vec![vec![int(2)]]
    );
    assert!(db
        .execute(
            "INSERT OR IGNORE INTO t VALUES(abs(-9223372036854775808))",
            &[]
        )
        .is_err());
    for seed in [
        "INSERT OR REPLACE INTO t VALUES(1)",
        "CREATE TABLE z(a UNIQUE ON CONFLICT FAIL)",
    ] {
        for end in 0..seed.len() {
            assert!(
                std::panic::catch_unwind(|| Connection::new().execute(&seed[..end], &[])).is_ok()
            );
        }
    }
}
