#![forbid(unsafe_code)]
use sqlite_safe::{
    sql::{Connection, SqlLimits},
    AutoVacuum, Encoding, Error, ImageBuilder, Text, Value,
};
fn i(n: i64) -> Value {
    Value::Integer(n)
}
fn t(s: &str) -> Value {
    Value::Text(Text::utf8(s))
}
fn fixture() -> Connection {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,a TEXT COLLATE NOCASE,b INT,c INT AS(b+1) STORED);INSERT INTO t(id,a,b) VALUES(1,'A',1),(2,'b',2),(3,NULL,3);").unwrap();
    c
}
#[test]
fn row_comparisons_preserve_null_logic_and_short_circuiting() {
    let mut c = Connection::new();
    assert_eq!(c.execute("SELECT (1,NULL,3)=(1,NULL,4),(1,NULL,3)=(1,NULL,3),(1,2,NULL)<(1,3,0),(1,NULL)IS(1,NULL),(2,2)IS(TRUE,2)",&[]).unwrap().rows,vec![vec![i(0),Value::Null,i(1),i(1),i(0)]]);
    assert_eq!(
        c.execute(
            "SELECT (1,abs(-9223372036854775808))<(2,0),(1,abs(-9223372036854775808))=(2,0)",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(1), i(0)]]
    );
    for q in [
        "SELECT (1,abs(-9223372036854775808)) BETWEEN (2,0) AND (3,0)",
        "SELECT CASE (1,abs(-9223372036854775808)) WHEN (2,0) THEN 1 ELSE 0 END",
    ] {
        assert!(c.execute(q, &[]).is_err());
    }
    assert_eq!(
        c.execute(
            "SELECT CASE (1,2) WHEN (1,NULL) THEN 3 WHEN (1,2) THEN 4 ELSE 5 END",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(4)]]
    );
}
#[test]
fn multi_column_subqueries_keep_affinities_and_collations_with_bound_parameters() {
    let mut c = fixture();
    let q = c
        .prepare("SELECT id FROM t WHERE (a,b)=(SELECT ?1,?2) ORDER BY id")
        .unwrap();
    assert_eq!(
        c.execute_prepared(&q, &[t("a"), t("1")]).unwrap().rows,
        vec![vec![i(1)]]
    );
    assert_eq!(
        c.execute_prepared(&q, &[t("b"), i(2)]).unwrap().rows,
        vec![vec![i(2)]]
    );
    assert!(c
        .execute_prepared(&q, &[Value::Null, i(3)])
        .unwrap()
        .rows
        .is_empty());
    assert_eq!(c.execute("SELECT (a,b)=(SELECT 'a',1),(SELECT 'a',1)=(a,b),(a,b)=(SELECT 'a' COLLATE BINARY,1) FROM t WHERE id=1",&[]).unwrap().rows,vec![vec![i(1),i(1),i(0)]]);
    assert_eq!(
        c.execute("SELECT (SELECT a,b FROM t WHERE 0) IS (NULL,NULL)", &[])
            .unwrap()
            .rows,
        vec![vec![i(1)]]
    );
}
#[test]
fn membership_handles_row_lists_tables_ctes_and_correlated_scopes() {
    let mut c = fixture();
    assert_eq!(c.execute("SELECT (NULL,2) IN ((1,3)),(NULL,2) IN ((1,2)),(NULL,2) NOT IN (SELECT a,b FROM t WHERE 0),(missing,2) IN ()",&[]).unwrap().rows,vec![vec![i(0),Value::Null,i(1),i(0)]]);
    c.execute_batch(
        "CREATE TABLE q(a TEXT COLLATE NOCASE,b INT);INSERT INTO q VALUES('a',1),('b',2);",
    )
    .unwrap();
    assert_eq!(
        c.execute("SELECT id FROM t WHERE (a,b) IN q ORDER BY id", &[])
            .unwrap()
            .rows,
        vec![vec![i(1)], vec![i(2)]]
    );
    assert_eq!(
        c.execute(
            "WITH q(a,b) AS(VALUES('B',2)) SELECT id FROM t WHERE (a,b) IN q",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(2)]]
    );
    assert_eq!(
        c.execute(
            "SELECT id,(a,b) IN (SELECT x.a,x.b FROM t x WHERE x.id=t.id) FROM t ORDER BY id",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(1), i(1)], vec![i(2), i(1)], vec![i(3), Value::Null]]
    );
}
#[test]
fn tuple_assignments_upserts_duplicates_and_generated_values_are_atomic() {
    let mut c = fixture();
    c.execute(
        "UPDATE t SET a=abs(-9223372036854775808),(a,b)=(SELECT b,a) WHERE id=1",
        &[],
    )
    .unwrap();
    assert_eq!(
        c.execute("SELECT a,b,c FROM t WHERE id=1", &[])
            .unwrap()
            .rows,
        vec![vec![t("1"), t("A"), i(1)]]
    );
    c.execute("INSERT INTO t(id,a,b) VALUES(2,'new',9) ON CONFLICT(id) DO UPDATE SET (a,b)=(SELECT excluded.a,excluded.b)",&[]).unwrap();
    assert_eq!(
        c.execute("SELECT a,b,c FROM t WHERE id=2", &[])
            .unwrap()
            .rows,
        vec![vec![t("new"), i(9), i(10)]]
    );
    let before = c.to_image(512).unwrap();
    c.execute_batch("BEGIN;UPDATE t SET (a,b)=(SELECT 'changed',10);ROLLBACK;")
        .unwrap();
    assert_eq!(c.to_image(512).unwrap(), before);
    assert!(c
        .execute(
            "UPDATE t SET (a,b)=(SELECT 'bad',abs(-9223372036854775808))",
            &[]
        )
        .is_err());
    assert_eq!(c.to_image(512).unwrap(), before);
    c.execute(
        "UPDATE t SET (a,a)=(1,2),rowid=abs(-9223372036854775808),id=11 WHERE id=1",
        &[],
    )
    .unwrap();
    assert_eq!(
        c.execute("SELECT a FROM t WHERE id=11", &[]).unwrap().rows,
        vec![vec![t("2")]]
    );
}
#[test]
fn malformed_shapes_depth_and_row_bytes_fail_without_partial_writes() {
    let mut c = fixture();
    let before = c.to_image(512).unwrap();
    for q in [
        "SELECT (1,2)",
        "SELECT ((1,2),3)=((1,2),3)",
        "SELECT CASE WHEN 1 THEN 1 ELSE (2,3) END",
        "UPDATE t SET (a,b)=(SELECT 1)",
        "UPDATE t SET a=(SELECT 1,2)",
        "SELECT (a,b) IN (SELECT a FROM t)",
    ] {
        assert!(c.execute(q, &[]).is_err(), "{q}");
        assert_eq!(c.to_image(512).unwrap(), before);
    }
    let sql = "UPDATE t SET (a,b)=(SELECT a,b+1) WHERE (a,b) IN (VALUES('A',1))";
    for end in 1..sql.len() {
        let mut attempt = Connection::from_image(&before).unwrap();
        let _ = attempt.execute(&sql[..end], &[]);
        assert_eq!(
            attempt.execute("SELECT count(*) FROM t", &[]).unwrap().rows,
            vec![vec![i(3)]]
        );
    }
    let wide = format!("SELECT ({0})=({0})", vec!["1"; 2001].join(","));
    assert!(matches!(c.execute(&wide, &[]), Err(Error::Limit(_))));
    for (prefix, suffix) in [("(", ")"), ("1 IN (", ")"), ("(1,2)=(1,", ")")] {
        let deep = format!("SELECT {}1{}", prefix.repeat(1000), suffix.repeat(1000));
        assert!(matches!(c.execute(&deep, &[]), Err(Error::Limit(_))));
    }
    let mut limited = Connection::with_limits(SqlLimits {
        max_database_bytes: 8192,
        ..SqlLimits::default()
    });
    assert!(matches!(
        limited.execute(
            "SELECT (?1,?1) BETWEEN (?1,?1) AND (?1,?1)",
            &[t(&"x".repeat(4100))]
        ),
        Err(Error::Limit(_))
    ));
}
#[test]
fn row_predicate_schemas_and_updates_roundtrip_all_image_configurations() {
    for encoding in [Encoding::Utf8, Encoding::Utf16Le, Encoding::Utf16Be] {
        for page_size in [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536] {
            for mode in [AutoVacuum::None, AutoVacuum::Full, AutoVacuum::Incremental] {
                let image = ImageBuilder::new(page_size)
                    .unwrap()
                    .encoding(encoding)
                    .unwrap()
                    .auto_vacuum(mode)
                    .finish()
                    .unwrap();
                let mut c = Connection::from_image(&image).unwrap();
                c.execute_batch("CREATE TABLE t(id INT PRIMARY KEY,a TEXT,b INT);CREATE TABLE q(a TEXT,b INT);INSERT INTO t VALUES(1,'é',1),(2,'Rust',2),(3,'🦀',3);INSERT INTO q VALUES('é',1),('🦀',3);CREATE VIEW v AS SELECT * FROM t WHERE (a,b) IN q;CREATE INDEX idx ON t((a,b)>('',0)) WHERE (id,b)>(0,0);UPDATE t SET (a,b)=(SELECT a,b+10) WHERE (a,b) IN q;").unwrap();
                let mut restored = Connection::from_image(&c.to_image(page_size).unwrap()).unwrap();
                assert_eq!(
                    restored
                        .execute("SELECT b FROM t ORDER BY id", &[])
                        .unwrap()
                        .rows,
                    vec![vec![i(11)], vec![i(2)], vec![i(13)]]
                );
                assert!(restored
                    .execute("SELECT * FROM v", &[])
                    .unwrap()
                    .rows
                    .is_empty());
                restored
                    .execute("UPDATE t SET (a,b)=(SELECT a,b-10) WHERE (id,b)>(1,5)", &[])
                    .unwrap();
                restored.to_image(page_size).unwrap();
            }
        }
    }
}
