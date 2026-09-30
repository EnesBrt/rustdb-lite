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
    c.execute_batch("CREATE TABLE a(x INT,y TEXT);CREATE TABLE b(x INT,z TEXT);CREATE TABLE c(w INT);INSERT INTO a VALUES(1,'a'),(2,'b');INSERT INTO b VALUES(2,'c'),(3,'d');INSERT INTO c VALUES(1);").unwrap();
    c
}
#[test]
fn parentheses_change_outer_join_association() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE a(x INT);CREATE TABLE b(x INT);CREATE TABLE c(x INT);INSERT INTO a VALUES(1),(2);INSERT INTO b VALUES(1),(2);INSERT INTO c VALUES(2);").unwrap();
    assert_eq!(
        c.execute(
            "SELECT a.x,b.x,c.x FROM a LEFT JOIN (b JOIN c ON b.x=c.x) ON a.x=b.x ORDER BY a.x",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(1), Value::Null, Value::Null], vec![i(2), i(2), i(2)]]
    );
    assert_eq!(
        c.execute(
            "SELECT a.x,b.x,c.x FROM a LEFT JOIN b ON a.x=b.x JOIN c ON b.x=c.x ORDER BY a.x",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(2), i(2), i(2)]]
    );
}
#[test]
fn nested_names_distinguish_aliases_original_columns_merged_values_and_rowids() {
    let mut c = fixture();
    let result = c
        .execute(
            "SELECT g.x,a.x,b.x,a.rowid,b.rowid FROM c,(a FULL JOIN b USING(x)) g ORDER BY g.x",
            &[],
        )
        .unwrap();
    assert_eq!(
        result.columns,
        vec!["x", "x:1", "x:2", "_ROWID_", "_ROWID_:1"]
    );
    assert_eq!(
        result.rows,
        vec![
            vec![i(1), i(1), Value::Null, i(1), Value::Null],
            vec![i(2), i(2), i(2), i(2), i(1)],
            vec![i(3), Value::Null, i(3), Value::Null, i(2)]
        ]
    );
    assert_eq!(
        c.execute(
            "SELECT a.*,b.* FROM c,(a FULL JOIN b USING(x)) g ORDER BY g.x",
            &[]
        )
        .unwrap()
        .columns,
        vec!["x:1", "y", "x:2", "z"]
    );
    assert_eq!(
        c.execute(
            "SELECT g.\"x:1\",g._ROWID_ FROM c,(a FULL JOIN b USING(x)) g ORDER BY g.x",
            &[]
        )
        .unwrap()
        .rows,
        vec![
            vec![i(1), i(1)],
            vec![i(2), i(2)],
            vec![Value::Null, Value::Null]
        ]
    );
    for sql in [
        "SELECT g.* FROM c,(a JOIN b USING(x)) g",
        "SELECT a.* FROM (a JOIN b USING(x)) g",
        "SELECT x FROM c,(a JOIN b ON 1) g",
    ] {
        assert!(c.execute(sql, &[]).is_err(), "{sql}");
    }
}
#[test]
fn on_dependencies_include_nested_correlations_aliases_and_future_sources() {
    let mut c = fixture();
    let q=c.prepare("SELECT a.x,b.x,c.w FROM a JOIN b ON (SELECT (SELECT c.w))=a.x JOIN c ON c.w=?1 ORDER BY b.x").unwrap();
    assert_eq!(
        c.execute_prepared(&q, &[i(1)]).unwrap().rows,
        vec![vec![i(1), i(2), i(1)], vec![i(1), i(3), i(1)]]
    );
    assert!(c.execute_prepared(&q, &[i(2)]).unwrap().rows.is_empty());
    assert_eq!(
        c.execute(
            "SELECT a.x+1 AS ax,b.x FROM a JOIN b ON ax=b.x ORDER BY a.x",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(2), i(2)], vec![i(3), i(3)]]
    );
    assert!(c
        .execute("SELECT * FROM a JOIN b ON c.w=a.x LEFT JOIN c ON 0", &[])
        .unwrap()
        .rows
        .is_empty());
    for sql in [
        "SELECT * FROM a LEFT JOIN b ON (SELECT c.w)=a.x JOIN c ON 1",
        "SELECT * FROM a JOIN b ON (SELECT (SELECT c.w))=a.x RIGHT JOIN c ON 1",
    ] {
        assert!(c.execute(sql, &[]).is_err(), "{sql}");
    }
    assert!(c
        .execute(
            "SELECT * FROM a LEFT JOIN b ON EXISTS(SELECT 1 FROM c WHERE c.w=a.x) JOIN c ON 1",
            &[]
        )
        .is_ok());
}
#[test]
fn singleton_groups_flatten_without_losing_recursive_cte_bindings() {
    let mut c = fixture();
    assert_eq!(
        c.execute("SELECT aa.x FROM ((a aa)) ORDER BY aa.x", &[])
            .unwrap()
            .rows,
        vec![vec![i(1)], vec![i(2)]]
    );
    assert_eq!(
        c.execute("SELECT bb.x FROM c,(a aa) bb ORDER BY bb.x", &[])
            .unwrap()
            .rows,
        vec![vec![i(1)], vec![i(2)]]
    );
    assert!(c.execute("SELECT aa.x FROM c,(a aa)", &[]).is_err());
    assert_eq!(c.execute("WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM ((q)) WHERE x<3) SELECT * FROM q",&[]).unwrap().rows,vec![vec![i(1)],vec![i(2)],vec![i(3)]]);
}
#[test]
fn group_depth_fuel_and_malformed_prefixes_fail_without_mutation() {
    let mut c = fixture();
    let before = c.to_image(512).unwrap();
    let deep = format!("SELECT * FROM {}a{}", "(".repeat(40), ")".repeat(40));
    assert!(matches!(c.execute(&deep, &[]), Err(Error::Limit(_))));
    let sql = "CREATE TABLE result AS SELECT * FROM c,(a FULL JOIN b USING(x)) g";
    for end in 1..sql.len() {
        let mut attempt = Connection::from_image(&before).unwrap();
        let _ = attempt.execute(&sql[..end], &[]);
        assert_eq!(
            attempt
                .execute("SELECT * FROM a ORDER BY x", &[])
                .unwrap()
                .rows,
            vec![vec![i(1), t("a")], vec![i(2), t("b")]]
        );
    }
    let mut limited = Connection::with_limits(SqlLimits {
        max_steps: 40,
        ..SqlLimits::default()
    });
    limited.execute_batch("CREATE TABLE a(x);CREATE TABLE b(x);INSERT INTO a VALUES(1),(2);INSERT INTO b VALUES(1),(2);").unwrap();
    assert!(matches!(
        limited.execute(
            "CREATE TABLE result AS SELECT * FROM a aa,(a FULL JOIN b USING(x)) g",
            &[]
        ),
        Err(Error::Limit(_))
    ));
    assert_eq!(c.to_image(512).unwrap(), before);
}
#[test]
fn grouped_views_roundtrip_all_page_encoding_and_vacuum_combinations() {
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
                c.execute_batch("CREATE TABLE a(x TEXT,y INT);CREATE TABLE b(x TEXT,z INT);CREATE TABLE c(w INT);CREATE VIEW v AS SELECT w,g.x,a.y,b.z FROM c,(a FULL JOIN b USING(x)) g;INSERT INTO a VALUES('é',1),('Rust',2);INSERT INTO b VALUES('Rust',3),('🦀',4);INSERT INTO c VALUES(1);CREATE TABLE copied AS SELECT * FROM v;").unwrap();
                let expected = c.execute("SELECT * FROM v ORDER BY x", &[]).unwrap().rows;
                let mut restored = Connection::from_image(&c.to_image(size).unwrap()).unwrap();
                assert_eq!(
                    restored
                        .execute("SELECT * FROM v ORDER BY x", &[])
                        .unwrap()
                        .rows,
                    expected
                );
                assert_eq!(
                    restored
                        .execute("SELECT * FROM copied ORDER BY x", &[])
                        .unwrap()
                        .rows,
                    expected
                );
                restored
                    .execute("INSERT INTO a SELECT x,10 FROM v WHERE y IS NULL", &[])
                    .unwrap();
                restored.to_image(size).unwrap();
            }
        }
    }
}
