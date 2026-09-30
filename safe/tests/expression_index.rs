#![forbid(unsafe_code)]
use sqlite_safe::{
    sql::{Connection, SqlLimits},
    AutoVacuum, Database, Encoding, Error, ImageBuilder, Text, Value,
};
fn i(n: i64) -> Value {
    Value::Integer(n)
}
fn text(s: &str) -> Value {
    Value::Text(Text::utf8(s))
}

#[test]
fn membership_changes_enforce_uniqueness_even_when_the_key_is_unchanged() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,x TEXT,active INT);CREATE UNIQUE INDEX ix ON t(lower(x)) WHERE active;INSERT INTO t VALUES(1,'Rust',1),(2,'RUST',0),(3,NULL,1);").unwrap();
    assert!(matches!(
        c.execute("UPDATE t SET active=1 WHERE id=2", &[]),
        Err(Error::Constraint(_))
    ));
    assert_eq!(
        c.execute("UPDATE OR IGNORE t SET active=1 WHERE id=2", &[])
            .unwrap()
            .changes,
        0
    );
    assert_eq!(
        c.execute(
            "UPDATE OR REPLACE t SET active=1 WHERE id=2 RETURNING id",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(2)]]
    );
    assert_eq!(
        c.execute("SELECT id FROM t ORDER BY id", &[]).unwrap().rows,
        vec![vec![i(2)], vec![i(3)]]
    );
    c.execute("UPDATE t SET active=0 WHERE id=2", &[]).unwrap();
    c.execute("INSERT INTO t VALUES(4,'rust',1)", &[]).unwrap();
    let image = c.to_image(512).unwrap();
    let mut restored = Connection::from_image(&image).unwrap();
    assert!(restored
        .execute("UPDATE t SET active=1 WHERE id=2", &[])
        .is_err());
}

#[test]
fn excluded_rows_do_not_evaluate_keys_and_failed_builds_are_atomic() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(x INT,y INT);CREATE INDEX ix ON t(abs(x)) WHERE y>0;INSERT INTO t VALUES(-9223372036854775808,0),(2,1);").unwrap();
    let before = c.to_image(512).unwrap();
    assert!(c.execute("UPDATE t SET y=1", &[]).is_err());
    assert_eq!(c.to_image(512).unwrap(), before);
    assert!(c.execute("CREATE INDEX bad ON t(abs(x))", &[]).is_err());
    assert_eq!(c.to_image(512).unwrap(), before);
    assert_eq!(
        c.execute("PRAGMA index_list(t)", &[]).unwrap().rows.len(),
        1
    );
    c.execute("CREATE UNIQUE INDEX empty ON t(abs(x)) WHERE NULL", &[])
        .unwrap();
    c.execute("DELETE FROM t WHERE y=0", &[]).unwrap();
    c.execute("UPDATE t SET y=0", &[]).unwrap();
}

#[test]
fn upsert_matches_expression_structure_tokens_collations_and_predicates() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,x INT,y INT);CREATE UNIQUE INDEX ix ON t(CAST(x AS INT)+1.0) WHERE y>0;INSERT INTO t VALUES(1,4,1);").unwrap();
    let before = c.to_image(512).unwrap();
    for target in [
        "CAST(x AS INTEGER)+1.0) WHERE y>0",
        "CAST(x AS int)+1.0) WHERE y>0",
        "CAST(x AS INT)+1.00) WHERE y>0",
        "CAST(x AS INT)+1.0) WHERE 0<y",
        "CAST(x AS INT)+1.0)",
    ] {
        assert!(c
            .execute(
                &format!("INSERT INTO t VALUES(2,4,1) ON CONFLICT({target} DO NOTHING"),
                &[]
            )
            .is_err());
        assert_eq!(c.to_image(512).unwrap(), before);
    }
    assert_eq!(c.execute("INSERT INTO t VALUES(2,4,1) ON CONFLICT(CAST(x AS INT)+1.0) WHERE y>00 DO UPDATE SET x=excluded.x+10 RETURNING x",&[]).unwrap().rows,vec![vec![i(14)]]);
    c.execute("CREATE UNIQUE INDEX single ON t(1)", &[])
        .unwrap();
    assert!(c
        .execute("INSERT INTO t VALUES(3,5,0) ON CONFLICT(1) DO NOTHING", &[])
        .is_err());
    assert_eq!(
        c.execute(
            "INSERT INTO t VALUES(3,5,0) ON CONFLICT(1 COLLATE BINARY) DO NOTHING",
            &[]
        )
        .unwrap()
        .changes,
        0
    );
}

#[test]
fn metadata_distinguishes_expressions_columns_partial_flags_and_primary_suffixes() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(id INT PRIMARY KEY,x TEXT COLLATE RTRIM,y INT) WITHOUT ROWID;CREATE INDEX ix ON t((x||y) COLLATE NoCaSe DESC,id+1) WHERE y>0;").unwrap();
    let list = c.execute("PRAGMA index_list(t)", &[]).unwrap().rows;
    assert_eq!(list[0], vec![i(0), text("ix"), i(0), text("c"), i(1)]);
    assert_eq!(
        c.execute("PRAGMA index_xinfo(ix)", &[]).unwrap().rows,
        vec![
            vec![i(0), i(-2), Value::Null, i(1), text("NoCaSe"), i(1)],
            vec![i(1), i(-2), Value::Null, i(0), text("BINARY"), i(1)],
            vec![i(2), i(0), text("id"), i(0), text("BINARY"), i(0)],
        ]
    );
    c.execute("CREATE INDEX direct ON t('x' COLLATE nocase)", &[])
        .unwrap();
    assert_eq!(
        c.execute("PRAGMA index_info(direct)", &[]).unwrap().rows,
        vec![vec![i(0), i(1), text("x")]]
    );
}

#[test]
fn index_function_opcodes_change_strict_statement_rollback_requirements() {
    for (key, expected) in [("x", 1), ("abs(x)", 0), ("coalesce(x,1)", 1)] {
        let mut c = Connection::new();
        c.execute("CREATE TABLE t(x INT) STRICT", &[]).unwrap();
        c.execute(&format!("CREATE INDEX ix ON t({key})"), &[])
            .unwrap();
        c.execute("BEGIN", &[]).unwrap();
        assert!(matches!(
            c.execute("INSERT INTO t VALUES(1),('bad')", &[]),
            Err(Error::Datatype(_))
        ));
        assert_eq!(c.changes(), 0);
        assert_eq!(
            c.execute("SELECT count(*) FROM t", &[]).unwrap().rows,
            vec![vec![i(expected)]]
        );
        c.execute("COMMIT", &[]).unwrap();
    }
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(x INT,g INT AS(abs(x))) STRICT;BEGIN;")
        .unwrap();
    assert!(matches!(
        c.execute("INSERT INTO t VALUES(1),('bad')", &[]),
        Err(Error::Datatype(_))
    ));
    assert!(c.execute("SELECT * FROM t", &[]).unwrap().rows.is_empty());
}

#[test]
fn expression_index_images_roundtrip_all_encoding_page_and_vacuum_layouts() {
    for encoding in [Encoding::Utf8, Encoding::Utf16Le, Encoding::Utf16Be] {
        for size in [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536] {
            for mode in [AutoVacuum::None, AutoVacuum::Full, AutoVacuum::Incremental] {
                let image = ImageBuilder::new(size)
                    .unwrap()
                    .encoding(encoding)
                    .unwrap()
                    .auto_vacuum(mode)
                    .finish()
                    .unwrap();
                let mut c = Connection::from_image(&image).unwrap();
                c.execute_batch("CREATE TABLE t(id INT PRIMARY KEY,x TEXT,y INT) WITHOUT ROWID;CREATE UNIQUE INDEX ix ON t(lower(x),id+1 DESC) WHERE y>0;INSERT INTO t VALUES(1,'É',1),(2,'🦀',0),(3,'Rust',NULL),(4,'SQL',1);").unwrap();
                let image = c.to_image(size).unwrap();
                let db = Database::parse(&image).unwrap();
                let entry = db
                    .schema()
                    .unwrap()
                    .into_iter()
                    .find(|e| e.name == "ix")
                    .unwrap();
                let mut count = 0;
                db.visit_rows(entry.root_page, |row| {
                    assert!(row.rowid.is_none());
                    assert_eq!(row.values.len(), 3);
                    count += 1;
                    Ok(())
                })
                .unwrap();
                assert_eq!(count, 2);
                let mut restored = Connection::from_image(&image).unwrap();
                assert_eq!(
                    restored
                        .execute("SELECT * FROM t ORDER BY id", &[])
                        .unwrap()
                        .rows,
                    c.execute("SELECT * FROM t ORDER BY id", &[]).unwrap().rows
                );
                restored
                    .execute("UPDATE t SET y=1 WHERE id=2", &[])
                    .unwrap();
                restored.to_image(size).unwrap();
            }
        }
    }
}

#[test]
fn invalid_and_over_budget_indexes_leave_schema_and_rows_unchanged() {
    let mut c = Connection::with_limits(SqlLimits {
        max_value_bytes: 64,
        ..SqlLimits::default()
    });
    c.execute("CREATE TABLE t(x TEXT,y INT)", &[]).unwrap();
    c.execute("INSERT INTO t VALUES(?1,1)", &[text(&"x".repeat(40))])
        .unwrap();
    let before = c.to_image(512).unwrap();
    for ddl in [
        "CREATE INDEX ix ON t(x||x)",
        "CREATE INDEX ix ON t(?1)",
        "CREATE INDEX ix ON t(sum(y))",
        "CREATE INDEX ix ON t(t.y)",
        "CREATE INDEX ix ON t(rowid)",
        "CREATE INDEX ix ON t(changes())",
        "CREATE INDEX ix ON t(y) WHERE ?1",
        "CREATE INDEX ix ON t(y) WHERE (SELECT 1)",
    ] {
        assert!(c.execute(ddl, &[]).is_err(), "{ddl}");
        assert_eq!(c.to_image(512).unwrap(), before);
    }
    let sql = "CREATE INDEX ix ON t(length(x) DESC) WHERE y>0";
    for end in 1..sql.len() {
        // Some prefixes are complete valid indexes; isolate each parse so those
        // cannot mask an error or alter the fixture of the next iteration.
        let mut attempt = Connection::from_image(&before).unwrap();
        let _ = attempt.execute(&sql[..end], &[]);
        assert_eq!(
            attempt.execute("SELECT * FROM t", &[]).unwrap().rows,
            vec![vec![text(&"x".repeat(40)), i(1)]]
        );
    }
}
