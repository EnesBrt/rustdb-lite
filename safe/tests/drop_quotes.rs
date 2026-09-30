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
fn schema(c: &Connection, name: &str) -> String {
    let bytes = c.to_image(512).unwrap();
    Database::parse(&bytes)
        .unwrap()
        .schema()
        .unwrap()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap()
        .sql
        .unwrap()
}

#[test]
fn drop_normalizes_whole_schema_and_prevents_later_literal_capture() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a,b,g AS(\"future\"),s AS(\"a'b\") STORED,CHECK(\"constant\">0));CREATE INDEX ix ON t(g) WHERE \"future\">0;CREATE VIEW v AS WITH unused AS(SELECT \"future\") SELECT a,g,\"future\" AS literal FROM t;CREATE TABLE u(x,y AS(\"future\") STORED,z DEFAULT \"default\");INSERT INTO t(a,b) VALUES(1,2);INSERT INTO u(x) VALUES(3);").unwrap();
    let counters = (c.changes(), c.total_changes(), c.last_insert_rowid());
    let prepared = c.prepare("SELECT * FROM v").unwrap();
    c.execute("ALTER TABLE main.t DROP COLUMN b", &[]).unwrap();
    assert_eq!(
        (c.changes(), c.total_changes(), c.last_insert_rowid()),
        counters
    );
    assert_eq!(
        schema(&c, "t"),
        "CREATE TABLE t(a,g AS('future'),s AS('a''b') STORED,CHECK('constant'>0))"
    );
    assert_eq!(schema(&c, "ix"), "CREATE INDEX ix ON t(g) WHERE 'future'>0");
    assert_eq!(
        schema(&c, "u"),
        "CREATE TABLE u(x,y AS('future') STORED,z DEFAULT \"default\")"
    );
    assert!(schema(&c, "v").contains("unused AS(SELECT 'future')"));
    c.execute_batch("ALTER TABLE t ADD future TEXT DEFAULT 'captured';ALTER TABLE u ADD future TEXT DEFAULT 'captured';INSERT INTO t(a) VALUES(4);INSERT INTO u(x) VALUES(5);").unwrap();
    assert_eq!(
        c.execute_prepared(&prepared, &[]).unwrap().rows,
        vec![
            vec![i(1), text("future"), text("future")],
            vec![i(4), text("future"), text("future")]
        ]
    );
    assert_eq!(
        c.execute("SELECT y,z FROM u", &[]).unwrap().rows,
        vec![
            vec![text("future"), text("default")],
            vec![text("future"), text("default")]
        ]
    );
}

#[test]
fn drop_preserves_bound_checks_and_native_quoted_view_behavior() {
    for declaration in [
        "a CHECK(\"b\">0),b",
        "a,b,CHECK(\"b\">0)",
        "a,b,g AS(a+1),CHECK(\"g\">0)",
        "a,b,g AS(\"b\")",
    ] {
        let mut c = Connection::new();
        c.execute(&format!("CREATE TABLE t({declaration})"), &[])
            .unwrap();
        let before = c.to_image(512).unwrap();
        let target = if declaration.contains("CHECK(\"g\"") {
            "g"
        } else {
            "b"
        };
        assert!(c
            .execute(&format!("ALTER TABLE t DROP {target}"), &[])
            .is_err());
        assert_eq!(c.to_image(512).unwrap(), before);
    }
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a,b CHECK(\"b\">0));INSERT INTO t VALUES(1,2);CREATE VIEW v AS SELECT \"b\" FROM t;ALTER TABLE t DROP b;").unwrap();
    assert_eq!(schema(&c, "v"), "CREATE VIEW v AS SELECT \"b\" FROM t");
    assert_eq!(
        c.execute("SELECT * FROM v", &[]).unwrap().rows,
        vec![vec![text("b")]]
    );
    c.execute("ALTER TABLE t ADD b INT DEFAULT 7", &[]).unwrap();
    assert_eq!(
        c.execute("SELECT * FROM v", &[]).unwrap().rows,
        vec![vec![i(7)]]
    );
}

#[test]
fn drop_rebinds_rowid_and_boolean_names_without_changing_stored_values() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a,rowid,CHECK(\"rowid\">0));INSERT INTO t VALUES(5,99);ALTER TABLE t DROP rowid;").unwrap();
    assert_eq!(
        c.execute("SELECT rowid,a FROM t", &[]).unwrap().rows,
        vec![vec![i(1), i(5)]]
    );
    c.execute_batch("CREATE TABLE u(a,true,v AS(true),s AS(true) STORED,CHECK(true));INSERT INTO u(a,true) VALUES(1,7);ALTER TABLE u DROP true;INSERT INTO u(a) VALUES(2);").unwrap();
    assert_eq!(
        c.execute("SELECT * FROM u ORDER BY a", &[]).unwrap().rows,
        vec![vec![i(1), i(1), i(7)], vec![i(2), i(1), i(1)]]
    );
    for sql in [
        "CREATE TABLE bad(a,true,g AS(\"true\"))",
        "CREATE TABLE keyed(a,true UNIQUE)",
    ] {
        c.execute(sql, &[]).unwrap();
    }
    let before = c.to_image(512).unwrap();
    assert!(c.execute("ALTER TABLE bad DROP true", &[]).is_err());
    assert!(c.execute("ALTER TABLE keyed DROP true", &[]).is_err());
    assert_eq!(c.to_image(512).unwrap(), before);
}

#[test]
fn drop_failures_and_savepoints_restore_normalized_unrelated_objects() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a,b);CREATE TABLE u(x,g AS(\"future\"));INSERT INTO t VALUES(1,2);CREATE VIEW v AS SELECT b FROM t;").unwrap();
    let before = c.to_image(512).unwrap();
    assert!(c.execute("ALTER TABLE t DROP b", &[]).is_err());
    assert_eq!(c.to_image(512).unwrap(), before);
    c.execute("DROP VIEW v", &[]).unwrap();
    let before = c.to_image(512).unwrap();
    c.execute_batch(
        "BEGIN;SAVEPOINT s;ALTER TABLE t DROP b;ALTER TABLE u ADD future INT;ROLLBACK TO s;",
    )
    .unwrap();
    assert_eq!(c.to_image(512).unwrap(), before);
    c.execute_batch("ALTER TABLE t DROP b;ROLLBACK;").unwrap();
    assert_eq!(c.to_image(512).unwrap(), before);
    c.execute("CREATE INDEX bad ON u(\"absent\")", &[]).unwrap();
    let before = c.to_image(512).unwrap();
    assert!(c.execute("ALTER TABLE t DROP b", &[]).is_err());
    assert_eq!(c.to_image(512).unwrap(), before);
}

#[test]
fn normalization_work_limits_restore_all_tables() {
    let mut c = Connection::with_limits(SqlLimits {
        max_steps: 150,
        ..SqlLimits::default()
    });
    c.execute_batch("CREATE TABLE t(a,b);CREATE TABLE u(x,g AS(\"future\") STORED);")
        .unwrap();
    for _ in 0..200 {
        c.execute("INSERT INTO u(x) VALUES(1)", &[]).unwrap();
    }
    let before = c.to_image(512).unwrap();
    assert!(matches!(
        c.execute("ALTER TABLE t DROP b", &[]),
        Err(Error::Limit(_))
    ));
    assert_eq!(c.to_image(512).unwrap(), before);
}

#[test]
fn normalized_drop_roundtrips_all_image_configurations() {
    for size in [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536] {
        for encoding in [Encoding::Utf8, Encoding::Utf16Le, Encoding::Utf16Be] {
            for mode in [AutoVacuum::None, AutoVacuum::Full, AutoVacuum::Incremental] {
                let image = ImageBuilder::new(size)
                    .unwrap()
                    .encoding(encoding)
                    .unwrap()
                    .auto_vacuum(mode)
                    .finish()
                    .unwrap();
                let mut c = Connection::from_image(&image).unwrap();
                for (name, suffix) in [("t", ""), ("w", ",WITHOUT ROWID")] {
                    c.execute_batch(&format!("CREATE TABLE {name}(a INT PRIMARY KEY,b INT,g TEXT AS(\"future\"),s TEXT AS(\"é🦀's\") STORED) STRICT{suffix};INSERT INTO {name}(a,b) VALUES(1,2);CREATE INDEX ix_{name} ON {name}(g) WHERE \"constant\">0;CREATE VIEW v_{name} AS SELECT a,g,s,\"future\" AS literal FROM {name};ALTER TABLE {name} DROP b;")).unwrap();
                }
                let bytes = c.to_image(size).unwrap();
                let mut c = Connection::from_image(&bytes).unwrap();
                for name in ["t", "w"] {
                    c.execute(
                        &format!("ALTER TABLE {name} ADD future TEXT DEFAULT 'capture'"),
                        &[],
                    )
                    .unwrap();
                    assert_eq!(
                        c.execute(&format!("SELECT * FROM v_{name}"), &[])
                            .unwrap()
                            .rows,
                        vec![vec![i(1), text("future"), text("é🦀's"), text("future")]]
                    );
                }
                assert_eq!(
                    Database::parse(&c.to_image(size).unwrap())
                        .unwrap()
                        .header()
                        .encoding,
                    encoding
                );
            }
        }
    }
}
