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

#[test]
fn using_preserves_qualified_values_and_native_wildcard_expansion() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE a(x TEXT,y INT);CREATE TABLE b(x INT,z INT);INSERT INTO a VALUES('1',10),('2',20);INSERT INTO b VALUES(1,100),(3,300);").unwrap();
    let r = c
        .execute(
            "SELECT x,a.x,b.x,a.* FROM a RIGHT JOIN b USING(x) ORDER BY b.x",
            &[],
        )
        .unwrap();
    assert_eq!(r.columns, vec!["x", "x", "x", "x", "y"]);
    assert_eq!(
        r.rows,
        vec![
            vec![i(1), t("1"), i(1), i(1), i(10)],
            vec![i(3), Value::Null, i(3), i(3), Value::Null]
        ]
    );
    assert_eq!(
        c.execute(
            "SELECT x,a.x,b.x FROM a FULL JOIN b USING(x) ORDER BY coalesce(a.y,b.z)",
            &[]
        )
        .unwrap()
        .rows,
        vec![
            vec![t("1"), t("1"), i(1)],
            vec![t("2"), t("2"), Value::Null],
            vec![i(3), Value::Null, i(3)]
        ]
    );
    c.execute(
        "CREATE TABLE snapshot AS SELECT * FROM a FULL JOIN b USING(x)",
        &[],
    )
    .unwrap();
    let info = c.execute("PRAGMA table_info(snapshot)", &[]).unwrap();
    assert_eq!(info.rows[0][2], t("TEXT"));
    assert_eq!(
        c.execute("SELECT typeof(x) FROM snapshot", &[])
            .unwrap()
            .rows,
        vec![vec![t("text")]; 3]
    );
}

#[test]
fn unmatched_rows_null_extend_generated_columns_and_empty_sides() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE a(x INT,g INT AS(coalesce(x,99)));CREATE TABLE b(x INT,h INT AS(coalesce(x,88)));INSERT INTO b(x) VALUES(2),(NULL);").unwrap();
    let sql = "SELECT a.x,a.g,b.x,b.h FROM a FULL JOIN b USING(x) ORDER BY b.rowid";
    assert_eq!(
        c.execute(sql, &[]).unwrap().rows,
        vec![
            vec![Value::Null, Value::Null, i(2), i(2)],
            vec![Value::Null, Value::Null, Value::Null, i(88)]
        ]
    );
    c.execute("INSERT INTO a(x) VALUES(NULL)", &[]).unwrap();
    assert_eq!(
        c.execute(
            "SELECT count(*),count(a.g),count(b.h) FROM a FULL JOIN b USING(x)",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(3), i(1), i(2)]]
    );
    c.execute("DELETE FROM b", &[]).unwrap();
    assert!(c
        .execute("SELECT * FROM a RIGHT JOIN b ON 1", &[])
        .unwrap()
        .rows
        .is_empty());
    assert_eq!(
        c.execute("SELECT g,h FROM a FULL JOIN b ON 1", &[])
            .unwrap()
            .rows,
        vec![vec![i(99), Value::Null]]
    );
}

#[test]
fn chained_merges_bind_nearest_outer_scopes_and_reuse_parameters() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE a(x INT);CREATE TABLE b(x INT);CREATE TABLE c(x INT);INSERT INTO a VALUES(1),(2);INSERT INTO b VALUES(2),(3);INSERT INTO c VALUES(3),(4);").unwrap();
    let query=c.prepare("SELECT x,(SELECT (SELECT x+?1)),a.x,b.x,c.x FROM a FULL JOIN b USING(x) FULL JOIN c USING(x) ORDER BY x").unwrap();
    for n in [10, 20] {
        assert_eq!(
            c.execute_prepared(&query, &[i(n)]).unwrap().rows,
            vec![
                vec![i(1), i(1 + n), i(1), Value::Null, Value::Null],
                vec![i(2), i(2 + n), i(2), i(2), Value::Null],
                vec![i(3), i(3 + n), Value::Null, i(3), i(3)],
                vec![i(4), i(4 + n), Value::Null, Value::Null, i(4)]
            ]
        );
    }
    assert_eq!(
        c.execute(
            "SELECT x FROM a FULL JOIN b USING(x) RIGHT JOIN c USING(x) ORDER BY x",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(3)], vec![i(4)]]
    );
    assert_eq!(
        c.execute(
            "WITH q AS(SELECT * FROM a NATURAL FULL JOIN b) SELECT x FROM q ORDER BY x",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(1)], vec![i(2)], vec![i(3)]]
    );
}

#[test]
fn on_filters_matches_before_null_extension_and_where_filters_after_it() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE a(x INT);CREATE TABLE b(x INT);INSERT INTO a VALUES(1),(2);INSERT INTO b VALUES(2),(3);").unwrap();
    assert_eq!(
        c.execute(
            "SELECT a.x,b.x FROM a FULL JOIN b ON a.x=b.x AND a.x=1 ORDER BY a.x,b.x",
            &[]
        )
        .unwrap()
        .rows,
        vec![
            vec![Value::Null, i(2)],
            vec![Value::Null, i(3)],
            vec![i(1), Value::Null],
            vec![i(2), Value::Null]
        ]
    );
    assert!(c
        .execute(
            "SELECT a.x,b.x FROM a FULL JOIN b ON a.x=b.x WHERE a.x=1 AND b.x=1",
            &[]
        )
        .unwrap()
        .rows
        .is_empty());
    c.execute(
        "CREATE VIEW v AS SELECT * FROM a NATURAL FULL OUTER JOIN b",
        &[],
    )
    .unwrap();
    assert_eq!(
        c.execute("SELECT x,count(*) FROM v GROUP BY x ORDER BY x", &[])
            .unwrap()
            .rows,
        vec![vec![i(1), i(1)], vec![i(2), i(1)], vec![i(3), i(1)]]
    );
    c.execute("CREATE TABLE result(x INT)", &[]).unwrap();
    assert_eq!(
        c.execute(
            "INSERT INTO result SELECT x FROM v ORDER BY x RETURNING x",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(1)], vec![i(2)], vec![i(3)]]
    );
}

#[test]
fn invalid_joins_and_bounded_execution_leave_database_unchanged() {
    let mut c = Connection::with_limits(SqlLimits {
        max_rows: 6,
        ..SqlLimits::default()
    });
    c.execute_batch("CREATE TABLE a(x);CREATE TABLE b(x);INSERT INTO a VALUES(1),(2),(3);INSERT INTO b VALUES(4),(5),(6);").unwrap();
    let before = c.to_image(512).unwrap();
    for sql in [
        "SELECT * FROM a NATURAL JOIN b ON 1",
        "SELECT * FROM a LEFT INNER JOIN b",
        "SELECT * FROM a JOIN b USING(missing)",
        "SELECT * FROM a JOIN b USING(rowid)",
        "SELECT x FROM a JOIN b ON 1",
    ] {
        assert!(c.execute(sql, &[]).is_err(), "{sql}");
    }
    assert!(matches!(
        c.execute(
            "CREATE TABLE failed AS SELECT * FROM a FULL JOIN b ON 1",
            &[]
        ),
        Err(Error::Limit(_))
    ));
    assert_eq!(c.to_image(512).unwrap(), before);
    let sql = "SELECT * FROM a NATURAL FULL OUTER JOIN b WHERE x IS NOT NULL";
    for end in 1..sql.len() {
        let _ = c.execute(&sql[..end], &[]);
    }
    assert_eq!(c.to_image(512).unwrap(), before);
}

#[test]
fn joined_views_and_snapshots_roundtrip_encoding_page_and_vacuum_modes() {
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
                c.execute_batch("CREATE TABLE a(x TEXT PRIMARY KEY,y INT) WITHOUT ROWID;CREATE TABLE b(x TEXT,z INT);CREATE VIEW v AS SELECT * FROM a FULL JOIN b USING(x);INSERT INTO a VALUES('é',1),('Rust',2);INSERT INTO b VALUES('Rust',3),('🦀',4);CREATE TABLE copy AS SELECT * FROM v;").unwrap();
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
                        .execute("SELECT * FROM copy ORDER BY x", &[])
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
