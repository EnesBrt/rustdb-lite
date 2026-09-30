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
    c.execute_batch("CREATE TABLE t(g INT,x TEXT COLLATE NOCASE,k INT,keep INT);INSERT INTO t VALUES(1,'b',3,1),(1,'A',2,1),(1,'a',1,0),(2,'z',5,0),(2,'y',4,1);").unwrap();
    c
}
#[test]
fn prepared_filters_order_inputs_and_keep_separator_rows_together() {
    let mut c = fixture();
    let stmt=c.prepare("SELECT g,string_agg(x,':'||k ORDER BY k DESC) FILTER(WHERE keep=?1),count(*) FILTER(WHERE keep=?1) FROM t GROUP BY g ORDER BY g").unwrap();
    assert_eq!(
        c.execute_prepared(&stmt, &[i(1)]).unwrap().rows,
        vec![vec![i(1), t("b:2A"), i(2)], vec![i(2), t("y"), i(1)]]
    );
    assert_eq!(
        c.execute_prepared(&stmt, &[i(0)]).unwrap().rows,
        vec![vec![i(1), t("a"), i(1)], vec![i(2), t("z"), i(1)]]
    );
    assert_eq!(
        c.execute_prepared(&stmt, &[]).unwrap().rows,
        vec![vec![i(1), Value::Null, i(0)], vec![i(2), Value::Null, i(0)]]
    );
    assert_eq!(
        c.execute(
            "SELECT group_concat(x ORDER BY keep NULLS FIRST,k) FROM t",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![t("a,z,A,b,y")]]
    );
}
#[test]
fn distinct_input_keys_preserve_native_types_and_collation_representatives() {
    let mut c = fixture();
    assert_eq!(
        c.execute(
            "SELECT group_concat(DISTINCT x ORDER BY x),group_concat(DISTINCT x ORDER BY k) FROM t",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![t("a,b,y,z"), t("A,b,y,z")]]
    );
    assert_eq!(c.execute("SELECT min(x ORDER BY k COLLATE NOCASE)='a',min(x) FILTER(WHERE keep COLLATE NOCASE)='a',min(x COLLATE NOCASE)='a' FROM t",&[]).unwrap().rows,vec![vec![i(0),i(0),i(1)]]);
    c.execute_batch("CREATE TABLE n(x,k);INSERT INTO n VALUES(1,1),(1.0,2),(2,3);")
        .unwrap();
    assert_eq!(
        c.execute(
            "SELECT typeof(sum(DISTINCT x ORDER BY x)),typeof(sum(DISTINCT x ORDER BY k)) FROM n",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![t("real"), t("integer")]]
    );
}
#[test]
fn filtered_arguments_are_lazy_but_names_and_aggregate_contexts_are_checked() {
    let mut c = fixture();
    let before = c.to_image(512).unwrap();
    assert_eq!(c.execute("SELECT sum(abs(-9223372036854775808) ORDER BY abs(-9223372036854775808)) FILTER(WHERE 0) FROM t",&[]).unwrap().rows,vec![vec![Value::Null]]);
    assert_eq!(
        c.execute(
            "SELECT min(k ORDER BY abs(-9223372036854775808)),count(ORDER BY missing) FROM t",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(1), i(5)]]
    );
    for sql in [
        "CREATE TABLE bad AS SELECT sum(k ORDER BY abs(-9223372036854775808)) FROM t",
        "SELECT abs(k) FILTER(WHERE keep) FROM t",
        "SELECT min(k ORDER BY missing) FROM t",
        "SELECT sum(k) FILTER(WHERE sum(keep)) FROM t",
        "SELECT count(ALL *) FROM t",
    ] {
        assert!(c.execute(sql, &[]).is_err(), "{sql}");
        assert_eq!(c.to_image(512).unwrap(), before);
    }
    assert_eq!(
        c.execute(
            "SELECT EXISTS(SELECT sum(abs(-9223372036854775808)) FILTER(WHERE 0) FROM t)",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(1)]]
    );
}
#[test]
fn correlations_and_filtered_extrema_select_the_native_bare_row() {
    let mut c = fixture();
    assert_eq!(
        c.execute(
            "SELECT g,x,min(k) FILTER(WHERE keep),count(*) FROM t GROUP BY g ORDER BY g",
            &[]
        )
        .unwrap()
        .rows,
        vec![
            vec![i(1), t("A"), i(2), i(3)],
            vec![i(2), t("y"), i(4), i(2)]
        ]
    );
    assert_eq!(c.execute("SELECT g,(SELECT group_concat(b.x ORDER BY b.k) FILTER(WHERE b.keep AND b.g=a.g) FROM t b) FROM t a GROUP BY g ORDER BY g",&[]).unwrap().rows,vec![vec![i(1),t("A,b")],vec![i(2),t("y")]]);
    c.execute_batch("CREATE TABLE n(a,b);INSERT INTO n VALUES(1,NULL),(2,NULL),(3,NULL);")
        .unwrap();
    assert_eq!(
        c.execute("SELECT a,min(b) FILTER(WHERE 1) FROM n", &[])
            .unwrap()
            .rows,
        vec![vec![i(3), Value::Null]]
    );
    assert_eq!(
        c.execute("SELECT a,min(DISTINCT b) FILTER(WHERE 1) FROM n", &[])
            .unwrap()
            .rows,
        vec![vec![i(1), Value::Null]]
    );
}
#[test]
fn malformed_modifiers_and_order_memory_limits_preserve_database_state() {
    let c = fixture();
    let before = c.to_image(512).unwrap();
    let sql="CREATE VIEW v AS SELECT group_concat(x ORDER BY k DESC NULLS LAST) FILTER(WHERE keep) FROM t";
    for end in 1..sql.len() {
        let mut attempt = Connection::from_image(&before).unwrap();
        let _ = attempt.execute(&sql[..end], &[]);
        assert_eq!(
            attempt.execute("SELECT count(*) FROM t", &[]).unwrap().rows,
            vec![vec![i(5)]]
        );
    }
    let mut limited = Connection::with_limits(SqlLimits {
        max_database_bytes: 8192,
        ..SqlLimits::default()
    });
    limited
        .execute_batch("CREATE TABLE t(x);INSERT INTO t VALUES(1),(2),(3);")
        .unwrap();
    let before = limited.to_image(512).unwrap();
    assert!(matches!(
        limited.execute(
            "CREATE TABLE bad AS SELECT group_concat(x ORDER BY ?1) FROM t",
            &[t(&"x".repeat(4000))]
        ),
        Err(Error::Limit(_))
    ));
    assert_eq!(limited.to_image(512).unwrap(), before);
    assert_eq!(
        limited
            .execute(
                "SELECT group_concat(x ORDER BY ?1) FILTER(WHERE 0) FROM t",
                &[t(&"x".repeat(4000))]
            )
            .unwrap()
            .rows,
        vec![vec![Value::Null]]
    );
}
#[test]
fn aggregate_views_and_snapshots_roundtrip_all_image_configurations() {
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
                c.execute_batch("CREATE TABLE t(g INT,x TEXT,k INT,keep INT);INSERT INTO t VALUES(1,'é',3,1),(1,'Rust',2,1),(1,'🦀',1,0),(2,'safe',4,1);CREATE VIEW v AS SELECT g,group_concat(x,'|' ORDER BY k) FILTER(WHERE keep) AS label,count(*) FILTER(WHERE keep) AS n FROM t GROUP BY g;CREATE TABLE copied AS SELECT * FROM v;").unwrap();
                let expected = vec![vec![i(1), t("Rust|é"), i(2)], vec![i(2), t("safe"), i(1)]];
                let mut restored = Connection::from_image(&c.to_image(page_size).unwrap()).unwrap();
                assert_eq!(
                    restored
                        .execute("SELECT * FROM v ORDER BY g", &[])
                        .unwrap()
                        .rows,
                    expected
                );
                assert_eq!(
                    restored
                        .execute("SELECT * FROM copied ORDER BY g", &[])
                        .unwrap()
                        .rows,
                    expected
                );
                restored
                    .execute_batch("BEGIN;UPDATE t SET keep=1;ROLLBACK;")
                    .unwrap();
                assert_eq!(
                    restored
                        .execute("SELECT * FROM v ORDER BY g", &[])
                        .unwrap()
                        .rows,
                    expected
                );
            }
        }
    }
}
