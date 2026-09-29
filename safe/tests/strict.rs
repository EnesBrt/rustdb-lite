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
fn rows(db: &mut Connection) -> Vec<Vec<Value>> {
    db.execute("SELECT * FROM t ORDER BY rowid", &[])
        .unwrap()
        .rows
}

#[test]
fn prepared_strict_values_convert_and_any_preserves_storage_classes() {
    let mut db = Connection::new();
    db.execute(
        "CREATE TABLE t(n INT,r REAL,s TEXT,b BLOB,a ANY) STRICT",
        &[],
    )
    .unwrap();
    let insert = db
        .prepare("INSERT INTO t VALUES(?1,?2,?3,?4,?5) RETURNING *")
        .unwrap();
    for any in [
        Value::Null,
        i(12),
        Value::Real(12.0),
        text("0012"),
        text("12\0tail"),
        Value::Blob(vec![0, 255]),
    ] {
        let result = db
            .execute_prepared(
                &insert,
                &[
                    text("12\0tail"),
                    text("1.5"),
                    i(42),
                    Value::Blob(vec![1, 2]),
                    any.clone(),
                ],
            )
            .unwrap();
        assert_eq!(
            result.rows,
            vec![vec![
                i(12),
                Value::Real(1.5),
                text("42"),
                Value::Blob(vec![1, 2]),
                any
            ]]
        );
    }
    let before = rows(&mut db);
    for bad in [
        vec![Value::Real(1.5), i(1), i(1), Value::Null, Value::Null],
        vec![i(1), text("abc"), i(1), Value::Null, Value::Null],
        vec![i(1), i(1), Value::Blob(vec![1]), Value::Null, Value::Null],
        vec![i(1), i(1), i(1), text("12"), Value::Null],
    ] {
        assert!(matches!(
            db.execute_prepared(&insert, &bad),
            Err(Error::Datatype(_))
        ));
        assert_eq!(rows(&mut db), before);
        assert_eq!(db.changes(), 0);
        assert_eq!(db.total_changes(), 6);
    }
}

#[test]
fn strict_declarations_and_primary_key_nullability_are_atomic() {
    for schema in [
        "x",
        "x VARCHAR(10)",
        "x NUMERIC",
        "x INTEGER(3)",
        "x DOUBLE PRECISION",
        "x \"INT\"(3)",
        "x \"INT\" extra",
    ] {
        let mut db = Connection::new();
        let sql = format!("CREATE TABLE t({schema}) STRICT");
        assert!(db.execute(&sql, &[]).is_err());
        assert!(db.execute("SELECT * FROM t", &[]).is_err());
        for end in 0..=sql.len() {
            assert!(std::panic::catch_unwind(|| Connection::new().prepare(&sql[..end])).is_ok());
        }
    }
    let mut db = Connection::new();
    db.execute(
        "CREATE TABLE t(a INT,b ANY,PRIMARY KEY(a,b)) STRICT,STRICT",
        &[],
    )
    .unwrap();
    for sql in [
        "INSERT INTO t VALUES(NULL,1)",
        "INSERT INTO t VALUES(1,NULL)",
    ] {
        assert!(matches!(db.execute(sql, &[]), Err(Error::Constraint(_))));
    }
    assert!(db
        .execute("PRAGMA table_info(t)", &[])
        .unwrap()
        .rows
        .iter()
        .all(|r| r[3] == i(1)));
    db.execute_batch(
        "DROP TABLE t;CREATE TABLE t(x INTEGER PRIMARY KEY) STRICT;INSERT INTO t VALUES(NULL);",
    )
    .unwrap();
    assert_eq!(rows(&mut db), vec![vec![i(1)]]);
    assert_eq!(
        db.execute("PRAGMA table_info(t)", &[]).unwrap().rows[0][3],
        i(0)
    );
}

#[test]
fn datatype_prefix_retention_matches_statement_journaling_and_savepoints() {
    for explicit_id in [false, true] {
        for policy in [
            "",
            " OR ABORT",
            " OR IGNORE",
            " OR FAIL",
            " OR ROLLBACK",
            " OR REPLACE",
        ] {
            for transaction in [false, true] {
                let mut db = Connection::new();
                db.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,x INT) STRICT;INSERT INTO t VALUES(1,1);").unwrap();
                if transaction {
                    db.execute("SAVEPOINT s", &[]).unwrap();
                }
                let input = if explicit_id {
                    "VALUES(2,2),(3,'bad')"
                } else {
                    "(x) VALUES(2),('bad')"
                };
                assert!(matches!(
                    db.execute(&format!("INSERT{policy} INTO t {input} RETURNING *"), &[]),
                    Err(Error::Datatype(_))
                ));
                let retained =
                    transaction && (!explicit_id || !["", " OR ABORT"].contains(&policy));
                let expected = if retained {
                    vec![vec![i(1), i(1)], vec![i(2), i(2)]]
                } else {
                    vec![vec![i(1), i(1)]]
                };
                assert_eq!(
                    rows(&mut db),
                    expected,
                    "{explicit_id} {policy} {transaction}"
                );
                assert_eq!(
                    (db.changes(), db.total_changes(), db.last_insert_rowid()),
                    (0, 1, 2)
                );
                assert_eq!(db.is_autocommit(), !transaction);
                if transaction {
                    db.execute("ROLLBACK TO s", &[]).unwrap();
                    assert_eq!(rows(&mut db), vec![vec![i(1), i(1)]]);
                    db.execute("RELEASE s", &[]).unwrap();
                }
            }
        }
    }
}

#[test]
fn rowid_type_errors_use_statement_journaling_in_ordinary_tables_too() {
    for suffix in ["", " STRICT"] {
        let mut db = Connection::new();
        db.execute(
            &format!("CREATE TABLE t(id INTEGER PRIMARY KEY,x INT){suffix}"),
            &[],
        )
        .unwrap();
        db.execute("BEGIN", &[]).unwrap();
        assert!(matches!(
            db.execute("INSERT OR IGNORE INTO t VALUES(1,1),('bad',2)", &[]),
            Err(Error::Datatype(_))
        ));
        assert_eq!(rows(&mut db), vec![vec![i(1), i(1)]]);
        assert_eq!((db.changes(), db.total_changes()), (0, 0));
        db.execute("ROLLBACK", &[]).unwrap();
        assert!(rows(&mut db).is_empty());
    }
}

#[test]
fn upsert_and_not_null_short_circuit_before_later_type_checks() {
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,x INT,y TEXT NOT NULL ON CONFLICT IGNORE) STRICT;INSERT INTO t VALUES(1,1,'one');").unwrap();
    assert_eq!(
        db.execute("INSERT INTO t VALUES(2,'bad',NULL)", &[])
            .unwrap()
            .changes,
        0
    );
    assert_eq!(
        db.execute(
            "INSERT INTO t VALUES(1,'bad','two') ON CONFLICT(id) DO NOTHING",
            &[]
        )
        .unwrap()
        .changes,
        0
    );
    db.execute("CREATE INDEX ix ON t(x)", &[]).unwrap();
    assert!(matches!(
        db.execute("INSERT INTO t VALUES(2,'bad','two')", &[]),
        Err(Error::Datatype(_))
    ));
    db.execute("BEGIN", &[]).unwrap();
    assert!(matches!(
        db.execute(
            "INSERT INTO t VALUES(2,2,'two'),(1,1,'one') ON CONFLICT(id) DO UPDATE SET x='bad'",
            &[]
        ),
        Err(Error::Datatype(_))
    ));
    // y uses IGNORE, and DO UPDATE changes no unique/NOT NULL key, so none
    // of the emitted checks request a statement journal.
    assert_eq!(rows(&mut db).len(), 2);
    db.execute("ROLLBACK", &[]).unwrap();
    assert_eq!(rows(&mut db), vec![vec![i(1), i(1), text("one")]]);
}

#[test]
fn resource_errors_always_restore_the_statement() {
    let mut db = Connection::with_limits(SqlLimits {
        max_rows: 2,
        ..Default::default()
    });
    db.execute_batch("CREATE TABLE t(x INT) STRICT;BEGIN;")
        .unwrap();
    assert!(matches!(
        db.execute("INSERT INTO t VALUES(1),(2),(3)", &[]),
        Err(Error::Limit(_))
    ));
    assert!(rows(&mut db).is_empty());
    assert!(!db.is_autocommit());
    assert_eq!(db.total_changes(), 0);
    db.execute("INSERT INTO t VALUES(1)", &[]).unwrap();
    db.execute("COMMIT", &[]).unwrap();
    assert_eq!(rows(&mut db), vec![vec![i(1)]]);
}

#[test]
fn strict_schema_and_any_values_survive_encoded_images() {
    for encoding in [Encoding::Utf8, Encoding::Utf16Le, Encoding::Utf16Be] {
        for mode in [AutoVacuum::None, AutoVacuum::Full, AutoVacuum::Incremental] {
            for page_size in [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536] {
                let image = ImageBuilder::new(page_size)
                    .unwrap()
                    .encoding(encoding)
                    .unwrap()
                    .auto_vacuum(mode)
                    .finish()
                    .unwrap();
                let mut db = Connection::from_image(&image).unwrap();
                db.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,a ANY UNIQUE,r REAL,b BLOB) STRICT;INSERT INTO t(a,r,b) VALUES('001',1,x'00FF'),(1,2.5,x''),('é🦀',3,NULL);").unwrap();
                let expected = rows(&mut db);
                let mut copy = Connection::from_image(&db.to_image(page_size).unwrap()).unwrap();
                assert_eq!(rows(&mut copy), expected);
                assert_eq!(
                    copy.execute("PRAGMA table_list(t)", &[]).unwrap().rows[0][5],
                    i(1)
                );
                assert!(matches!(
                    copy.execute("UPDATE t SET r='invalid'", &[]),
                    Err(Error::Datatype(_))
                ));
                assert_eq!(rows(&mut copy), expected);
                copy.execute("INSERT INTO t(a) VALUES('native-compatible')", &[])
                    .unwrap();
                assert_eq!(copy.last_insert_rowid(), 4);
            }
        }
    }
}

#[test]
fn catalog_flags_cover_views_system_tables_and_filters() {
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,x ANY) STRICT;CREATE VIEW v AS SELECT x FROM t;CREATE VIEW broken AS SELECT * FROM missing;").unwrap();
    for (name, kind, columns, strict) in [
        ("t", "table", 2, 1),
        ("v", "view", 1, 0),
        ("broken", "view", 0, 0),
        ("sqlite_sequence", "table", 2, 0),
    ] {
        assert_eq!(
            db.execute(&format!("PRAGMA main.table_list('{name}')"), &[])
                .unwrap()
                .rows,
            vec![vec![
                text("main"),
                text(name),
                text(kind),
                i(columns),
                i(0),
                i(strict)
            ]]
        );
    }
    assert_eq!(
        db.execute("PRAGMA temp.table_list", &[]).unwrap().rows,
        vec![vec![
            text("temp"),
            text("sqlite_temp_schema"),
            text("table"),
            i(5),
            i(0),
            i(0)
        ]]
    );
    assert!(db
        .execute("PRAGMA table_list(sqlite_schema)", &[])
        .unwrap()
        .rows
        .is_empty());
    assert_eq!(
        db.execute("PRAGMA table_list(sqlite_master)", &[])
            .unwrap()
            .rows
            .len(),
        1
    );
    assert_eq!(db.execute("PRAGMA table_list", &[]).unwrap().rows.len(), 6);
}
