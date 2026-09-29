#![forbid(unsafe_code)]
use sqlite_safe::{
    sql::{Connection, SqlLimits},
    AutoVacuum, Database, Encoding, Error, ImageBuilder, Text, Value,
};

fn i(n: i64) -> Value {
    Value::Integer(n)
}
fn t(s: &str) -> Value {
    Value::Text(Text::utf8(s))
}

#[test]
fn views_resolve_dependencies_lazily_and_rebind_after_schema_changes() {
    let mut c = Connection::new();
    c.execute("CREATE VIEW v AS SELECT * FROM missing", &[])
        .unwrap();
    assert!(c.execute("SELECT * FROM v", &[]).is_err());
    c.execute_batch(
        "CREATE TABLE missing(x);INSERT INTO missing VALUES(1);CREATE VIEW w AS SELECT * FROM v;",
    )
    .unwrap();
    assert_eq!(
        c.execute("SELECT * FROM w", &[]).unwrap().rows,
        vec![vec![i(1)]]
    );
    c.execute_batch(
        "DROP TABLE missing;CREATE TABLE missing(y,z);INSERT INTO missing VALUES(2,3);",
    )
    .unwrap();
    let result = c.execute("SELECT * FROM w", &[]).unwrap();
    assert_eq!(result.columns, vec!["y", "z"]);
    assert_eq!(result.rows, vec![vec![i(2), i(3)]]);
    c.execute("CREATE VIEW bad(x,y) AS SELECT 1", &[]).unwrap();
    assert_eq!(
        c.execute("PRAGMA table_info(bad)", &[]).unwrap().rows,
        vec![
            vec![i(0), t("x"), t(""), i(0), Value::Null, i(0)],
            vec![i(1), t("y"), t(""), i(0), Value::Null, i(0)],
        ]
    );
    assert!(c.execute("SELECT * FROM bad", &[]).is_err());
    c.execute_batch("CREATE VIEW a AS SELECT * FROM b;CREATE VIEW b AS SELECT * FROM a;")
        .unwrap();
    assert!(c.execute("SELECT * FROM a", &[]).is_err());
}

#[test]
fn views_isolate_names_rows_and_expression_caches() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(x);INSERT INTO t VALUES(1);CREATE VIEW v AS SELECT x,(SELECT 8) a,(SELECT 9) b FROM t;").unwrap();
    assert_eq!(
        c.execute(
            "WITH t(x) AS(VALUES(99)) SELECT (SELECT 7),x,a,b FROM v",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(7), i(1), i(8), i(9)]]
    );
    assert_eq!(
        c.execute(
            "WITH v(x) AS(VALUES(99)) SELECT x,(SELECT x FROM main.v) FROM v",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(99), i(1)]]
    );
    c.execute("CREATE VIEW no_outer AS SELECT t.x", &[])
        .unwrap();
    assert!(c
        .execute("SELECT (SELECT * FROM no_outer) FROM t", &[])
        .is_err());
    assert!(c
        .execute("CREATE VIEW parameter AS SELECT (SELECT ?1)", &[])
        .is_err());
    c.execute("CREATE VIEW constants AS SELECT 1,(SELECT 2)", &[])
        .unwrap();
    assert_eq!(
        c.execute("SELECT * FROM constants", &[]).unwrap().rows,
        vec![vec![i(1), i(2)]]
    );
}

#[test]
fn view_metadata_and_ctas_types_names_rows_and_counters() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a VARCHAR(10) COLLATE NOCASE,b INTEGER PRIMARY KEY,c NUMERIC);INSERT INTO t VALUES('a',90,3),('B',100,4);CREATE VIEW v AS SELECT a,b,(SELECT c FROM t LIMIT 1) s,CAST(a AS INT) casted,b COLLATE NOCASE coll,b+1 expr FROM t;").unwrap();
    let info = c.execute("PRAGMA table_info(v)", &[]).unwrap();
    assert_eq!(
        info.rows.iter().map(|r| r[2].clone()).collect::<Vec<_>>(),
        vec![
            t("VARCHAR(10)"),
            t("INTEGER"),
            t("NUMERIC"),
            t("INT"),
            t("INT"),
            t("")
        ]
    );
    let before = (c.changes(), c.total_changes(), c.last_insert_rowid());
    c.execute("CREATE TABLE z AS SELECT * FROM v ORDER BY b DESC", &[])
        .unwrap();
    assert_eq!(
        (c.changes(), c.total_changes(), c.last_insert_rowid()),
        before
    );
    assert_eq!(
        c.execute("SELECT rowid,b,a='A' FROM z ORDER BY rowid", &[])
            .unwrap()
            .rows,
        vec![vec![i(1), i(100), i(0)], vec![i(2), i(90), i(0)]]
    );
    assert_eq!(
        c.execute("PRAGMA table_info(z)", &[])
            .unwrap()
            .rows
            .iter()
            .map(|r| r[2].clone())
            .collect::<Vec<_>>(),
        vec![t("TEXT"), t("INT"), t("NUM"), t("INT"), t("INT"), t("")]
    );
    c.execute("CREATE VIEW duplicate(x,x) AS SELECT 1,2", &[])
        .unwrap();
    assert_eq!(
        c.execute("SELECT * FROM duplicate", &[]).unwrap().columns,
        vec!["x", "x:1"]
    );
    c.execute(
        "CREATE TABLE names AS SELECT 1 a,2 a,3 \"a:1\",4 AS \"\"",
        &[],
    )
    .unwrap();
    assert_eq!(
        c.execute("SELECT * FROM names", &[]).unwrap().columns,
        vec!["a", "a:1", "a:2", ""]
    );
    let image = c.to_image(512).unwrap();
    Connection::from_image(&image).unwrap();
}

#[test]
fn views_and_ctas_roundtrip_all_encodings_modes_and_page_sizes() {
    for encoding in [Encoding::Utf8, Encoding::Utf16Le, Encoding::Utf16Be] {
        for mode in [AutoVacuum::None, AutoVacuum::Full, AutoVacuum::Incremental] {
            for size in [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536] {
                let empty = ImageBuilder::new(size)
                    .unwrap()
                    .encoding(encoding)
                    .unwrap()
                    .auto_vacuum(mode)
                    .finish()
                    .unwrap();
                let mut c = Connection::from_image(&empty).unwrap();
                c.execute_batch("CREATE TABLE t(x);INSERT INTO t VALUES('🦀');CREATE VIEW v AS SELECT * FROM t;CREATE VIEW w AS SELECT * FROM v;CREATE TABLE z AS SELECT * FROM w;").unwrap();
                let image = c.to_image(size).unwrap();
                let physical = Database::parse(&image).unwrap();
                assert_eq!(physical.header().encoding, encoding);
                assert_eq!(physical.header().auto_vacuum, mode);
                assert_eq!(
                    physical
                        .schema()
                        .unwrap()
                        .iter()
                        .filter(|e| e.kind == "view" && e.root_page == 0)
                        .count(),
                    2
                );
                let mut loaded = Connection::from_image(&image).unwrap();
                assert_eq!(
                    loaded
                        .execute("SELECT x FROM w UNION ALL SELECT x FROM z", &[])
                        .unwrap()
                        .rows,
                    vec![vec![t("🦀")], vec![t("🦀")]]
                );
            }
        }
    }
}

#[test]
fn readonly_views_namespace_rollback_and_failed_ctas_are_atomic() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(x);INSERT INTO t VALUES(1);CREATE VIEW v AS SELECT * FROM t;")
        .unwrap();
    for sql in [
        "INSERT INTO v VALUES(2)",
        "UPDATE v SET x=2",
        "DELETE FROM v",
        "DROP TABLE IF EXISTS v",
        "DROP VIEW IF EXISTS t",
        "CREATE TABLE v(x)",
        "CREATE VIEW t AS SELECT 1",
        "CREATE INDEX idx ON v(x)",
    ] {
        assert!(c.execute(sql, &[]).is_err(), "{sql}");
    }
    c.execute("CREATE TABLE IF NOT EXISTS v AS SELECT * FROM absent", &[])
        .unwrap();
    c.execute_batch(
        "BEGIN;DROP VIEW v;CREATE VIEW v AS SELECT 9 x;CREATE TABLE z AS SELECT * FROM v;ROLLBACK;",
    )
    .unwrap();
    assert_eq!(
        c.execute("SELECT * FROM v", &[]).unwrap().rows,
        vec![vec![i(1)]]
    );
    assert!(c.execute("SELECT * FROM z", &[]).is_err());
    assert!(c
        .execute("CREATE TABLE fail AS SELECT abs(-9223372036854775808)", &[])
        .is_err());
    c.execute("CREATE TABLE fail(x)", &[]).unwrap();
    c.execute_batch("DROP VIEW v;DROP VIEW IF EXISTS v;CREATE TABLE v(x);")
        .unwrap();
}

#[test]
fn view_depth_limits_corrupt_schema_identity_and_malformed_prefixes() {
    let mut c = Connection::new();
    for n in 0..80 {
        c.execute(
            &format!("CREATE VIEW v{n} AS SELECT * FROM v{}", n + 1),
            &[],
        )
        .unwrap();
    }
    c.execute("CREATE VIEW v80 AS SELECT 1", &[]).unwrap();
    assert!(matches!(
        c.execute("SELECT * FROM v0", &[]),
        Err(Error::Limit(_))
    ));
    let mut c = Connection::with_limits(SqlLimits {
        max_rows: 2,
        ..Default::default()
    });
    assert!(matches!(
        c.execute("CREATE TABLE big AS VALUES(1),(2),(3)", &[]),
        Err(Error::Limit(_))
    ));
    c.execute("CREATE TABLE big(x)", &[]).unwrap();
    c.execute("CREATE VIEW v AS SELECT 1", &[]).unwrap();
    let mut image = c.to_image(512).unwrap();
    let marker = b"CREATE VIEW v";
    let position = image
        .windows(marker.len())
        .position(|s| s == marker)
        .unwrap()
        + marker.len()
        - 1;
    image[position] = b'w';
    assert!(matches!(
        Connection::from_image(&image),
        Err(Error::Corrupt(_))
    ));
    for seed in [
        "CREATE VIEW v(a) AS WITH q AS(SELECT 1) SELECT * FROM q",
        "CREATE TABLE z AS SELECT (SELECT 1)",
    ] {
        for end in 0..seed.len() {
            assert!(
                std::panic::catch_unwind(|| Connection::new().execute(&seed[..end], &[])).is_ok()
            );
        }
    }
}
