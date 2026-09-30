#![forbid(unsafe_code)]
use sqlite_safe::{
    sql::{Connection, SqlLimits},
    AutoVacuum, Database, Encoding, Error, ImageBuilder, Text, Value,
};
fn i(n: i64) -> Value {
    Value::Integer(n)
}
fn sql(c: &Connection, name: &str) -> String {
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
fn rename_preserves_values_indexes_counters_and_sequence() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,a INT UNIQUE CHECK(t.a>0),s AS(a*2) STORED);INSERT INTO t(a) VALUES(2),(3);CREATE INDEX ix ON t((s+1)) WHERE t.a>0;CREATE VIEW v AS SELECT t.* FROM t;").unwrap();
    let before = (c.changes(), c.total_changes(), c.last_insert_rowid());
    let view = c.prepare("SELECT * FROM v ORDER BY id").unwrap();
    let old = c.prepare("SELECT * FROM t").unwrap();
    c.execute("ALTER TABLE main.t RENAME TO \"é new🦀\"", &[])
        .unwrap();
    assert_eq!(
        (c.changes(), c.total_changes(), c.last_insert_rowid()),
        before
    );
    assert!(c.execute_prepared(&old, &[]).is_err());
    assert_eq!(
        c.execute_prepared(&view, &[]).unwrap().rows,
        vec![vec![i(1), i(2), i(4)], vec![i(2), i(3), i(6)]]
    );
    assert!(sql(&c, "é new🦀").contains("CHECK(\"é new🦀\".a>0)"));
    assert_eq!(
        sql(&c, "ix"),
        "CREATE INDEX ix ON \"é new🦀\"((s+1)) WHERE \"é new🦀\".a>0"
    );
    c.execute("INSERT INTO \"é new🦀\"(a) VALUES(4)", &[])
        .unwrap();
    assert_eq!(c.last_insert_rowid(), 3);
    assert_eq!(
        c.execute("SELECT * FROM sqlite_sequence", &[])
            .unwrap()
            .rows,
        vec![vec![Value::Text(Text::utf8("é new🦀")), i(3)]]
    );
    assert!(c
        .execute("INSERT INTO \"é new🦀\"(a) VALUES(4)", &[])
        .is_err());
    let bytes = c.to_image(512).unwrap();
    assert!(Database::parse(&bytes)
        .unwrap()
        .schema()
        .unwrap()
        .iter()
        .any(|s| s.name == "sqlite_autoindex_é new🦀_1"));
}

#[test]
fn rename_uses_binding_for_aliases_ctes_outer_columns_and_unused_queries() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a);CREATE TABLE u(b);INSERT INTO t VALUES(7);INSERT INTO u VALUES(9);CREATE VIEW v AS SELECT t.a,(SELECT t.a FROM u AS q) AS outer_a FROM t;CREATE VIEW w AS WITH unused AS (SELECT t.missing FROM t),t AS (SELECT 3 AS a) SELECT t.a FROM t;CREATE VIEW q AS SELECT t.a FROM t AS t;").unwrap();
    c.execute(
        "CREATE VIEW shadowed AS SELECT (SELECT t.a FROM u AS t) FROM t",
        &[],
    )
    .unwrap();
    let before = c.to_image(512).unwrap();
    assert!(c.execute("ALTER TABLE t RENAME TO renamed", &[]).is_err());
    assert_eq!(c.to_image(512).unwrap(), before);
    c.execute("DROP VIEW shadowed", &[]).unwrap();
    c.execute("ALTER TABLE t RENAME TO renamed", &[]).unwrap();
    assert_eq!(sql(&c,"v"),"CREATE VIEW v AS SELECT \"renamed\".a,(SELECT \"renamed\".a FROM u AS q) AS outer_a FROM \"renamed\"");
    assert_eq!(
        c.execute("SELECT * FROM v", &[]).unwrap().rows,
        vec![vec![i(7), i(7)]]
    );
    assert_eq!(
        sql(&c, "q"),
        "CREATE VIEW q AS SELECT t.a FROM \"renamed\" AS t"
    );
    assert_eq!(
        c.execute("SELECT * FROM w", &[]).unwrap().rows,
        vec![vec![i(3)]]
    );
    assert!(sql(&c, "w").contains("SELECT t.missing FROM t"));
    c.execute(
        "CREATE VIEW unused AS WITH q AS (SELECT renamed.missing FROM renamed) SELECT 1",
        &[],
    )
    .unwrap();
    c.execute("ALTER TABLE renamed RENAME TO again", &[])
        .unwrap();
    assert!(sql(&c, "unused").contains("SELECT renamed.missing FROM \"again\""));
}

#[test]
fn rename_preserves_comments_quotes_literals_and_in_membership() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a);INSERT INTO t VALUES(1);CREATE VIEW v AS SELECT /* t.a */ \"t\".a, 't.a FROM t' AS literal FROM [t] WHERE a IN main.t;").unwrap();
    c.execute("ALTER TABLE t RENAME TO \"a\"\"b\"", &[])
        .unwrap();
    assert_eq!(sql(&c,"v"),"CREATE VIEW v AS SELECT /* t.a */ \"a\"\"b\".a, 't.a FROM t' AS literal FROM \"a\"\"b\" WHERE a IN main.\"a\"\"b\"");
    assert_eq!(
        c.execute("SELECT a FROM v", &[]).unwrap().rows,
        vec![vec![i(1)]]
    );
}

#[test]
fn rename_errors_and_savepoints_restore_schema_and_rows() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a);CREATE TABLE u(a);CREATE VIEW v AS SELECT * FROM t;INSERT INTO t VALUES(1);CREATE INDEX ix ON t(a);").unwrap();
    let before = c.to_image(512).unwrap();
    for statement in [
        "ALTER TABLE t RENAME TO t",
        "ALTER TABLE t RENAME TO T",
        "ALTER TABLE t RENAME TO u",
        "ALTER TABLE t RENAME TO v",
        "ALTER TABLE t RENAME TO ix",
        "ALTER TABLE t RENAME TO sqlite_x",
        "ALTER TABLE v RENAME TO x",
        "ALTER TABLE absent RENAME TO x",
    ] {
        assert!(c.execute(statement, &[]).is_err(), "{statement}");
        assert_eq!(c.to_image(512).unwrap(), before);
    }
    c.execute_batch("BEGIN;ALTER TABLE t RENAME TO x;SAVEPOINT s;ALTER TABLE x RENAME TO y;INSERT INTO y VALUES(2);ROLLBACK TO s;").unwrap();
    assert_eq!(
        c.execute("SELECT * FROM x", &[]).unwrap().rows,
        vec![vec![i(1)]]
    );
    c.execute("ROLLBACK", &[]).unwrap();
    assert_eq!(c.to_image(512).unwrap(), before);
    c.execute("CREATE VIEW broken AS SELECT * FROM absent", &[])
        .unwrap();
    let before = c.to_image(512).unwrap();
    assert!(c.execute("ALTER TABLE t RENAME TO x", &[]).is_err());
    assert_eq!(c.to_image(512).unwrap(), before);
}

#[test]
fn rename_limits_and_native_nested_join_rejection_are_atomic() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a);CREATE TABLE u(a);CREATE VIEW v AS SELECT t.a FROM t JOIN (u JOIN u AS x USING(a)) ON u.a=t.a;").unwrap();
    let before = c.to_image(512).unwrap();
    assert!(c.execute("ALTER TABLE t RENAME TO renamed", &[]).is_err());
    assert_eq!(c.to_image(512).unwrap(), before);
    for end in 1.."ALTER TABLE t RENAME TO renamed".len() {
        let _ = c.prepare(&"ALTER TABLE t RENAME TO renamed"[..end]);
    }
    let mut c = Connection::with_limits(SqlLimits {
        max_steps: 150,
        ..SqlLimits::default()
    });
    c.execute("CREATE TABLE t(a)", &[]).unwrap();
    for _ in 0..200 {
        c.execute("INSERT INTO t VALUES(1)", &[]).unwrap();
    }
    let before = c.to_image(512).unwrap();
    assert!(matches!(
        c.execute("ALTER TABLE t RENAME TO renamed", &[]),
        Err(Error::Limit(_))
    ));
    assert_eq!(c.to_image(512).unwrap(), before);
}

#[test]
fn rename_roundtrips_encoded_images_with_both_table_kinds() {
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
                    c.execute_batch(&format!("CREATE TABLE {name}(id INT PRIMARY KEY,a INT,s INT AS(a*2) STORED,g INT AS(s+1)) STRICT{suffix};INSERT INTO {name}(id,a) VALUES(1,2);CREATE INDEX ix_{name} ON {name}((s+g)) WHERE {name}.a>0;CREATE VIEW v_{name} AS SELECT {name}.* FROM {name};ALTER TABLE {name} RENAME TO renamed_{name};")).unwrap();
                }
                let bytes = c.to_image(size).unwrap();
                let mut c = Connection::from_image(&bytes).unwrap();
                for name in ["t", "w"] {
                    assert_eq!(
                        c.execute(&format!("SELECT * FROM v_{name}"), &[])
                            .unwrap()
                            .rows,
                        vec![vec![i(1), i(2), i(4), i(5)]]
                    );
                    c.execute(
                        &format!("ALTER TABLE renamed_{name} RENAME TO final_{name}"),
                        &[],
                    )
                    .unwrap();
                    c.execute(&format!("UPDATE final_{name} SET a=3"), &[])
                        .unwrap();
                }
                let bytes = c.to_image(size).unwrap();
                let db = Database::parse(&bytes).unwrap();
                assert_eq!(db.header().encoding, encoding);
            }
        }
    }
}
