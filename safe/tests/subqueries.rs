#![forbid(unsafe_code)]
use sqlite_safe::{
    sql::{Connection, SqlLimits},
    Error, Text, Value,
};

fn i(n: i64) -> Value {
    Value::Integer(n)
}
fn setup() -> Connection {
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,x NUMERIC,y TEXT COLLATE NOCASE);INSERT INTO t VALUES(1,1,'a'),(2,2,'B'),(3,NULL,NULL);").unwrap();
    db
}

#[test]
fn scalar_subqueries_preserve_affinity_and_have_scalar_collation_rules() {
    let mut db = setup();
    assert_eq!(
        db.execute(
            "SELECT (SELECT x FROM t WHERE 0),(SELECT x FROM t ORDER BY id DESC LIMIT 2 OFFSET 1)",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![Value::Null, i(2)]]
    );
    assert_eq!(db.execute("SELECT '1'=(SELECT x FROM t WHERE id=1),'A'=(SELECT y FROM t WHERE id=1),'A'=(SELECT y FROM t WHERE id=1) COLLATE NOCASE",&[]).unwrap().rows,vec![vec![i(1),i(0),i(1)]]);
    assert_eq!(db.execute("SELECT (SELECT 1 UNION ALL SELECT abs(-9223372036854775808)),(VALUES(7),(abs(-9223372036854775808)))",&[]).unwrap().rows,vec![vec![i(1),i(7)]]);
    assert!(db.execute("SELECT (SELECT 1,2)", &[]).is_err());
    assert!(db.execute("SELECT (SELECT sum(t.x)) FROM t", &[]).is_err());
}

#[test]
fn correlated_queries_resolve_nearest_rows_and_cte_scopes() {
    let mut db = setup();
    assert_eq!(db.execute("SELECT id,(SELECT a.y FROM t a WHERE a.id=t.id),(SELECT (SELECT t.id)) FROM t ORDER BY id",&[]).unwrap().rows,vec![vec![i(1),Value::Text(Text::utf8("a")),i(1)],vec![i(2),Value::Text(Text::utf8("B")),i(2)],vec![i(3),Value::Null,i(3)]]);
    assert_eq!(
        db.execute(
            "WITH q AS(SELECT t.id v) SELECT (SELECT v FROM q) FROM t ORDER BY id",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(1)], vec![i(2)], vec![i(3)]]
    );
    assert_eq!(
        db.execute(
            "SELECT (WITH q AS(SELECT t.id v) SELECT a.v+b.v FROM q a,q b) FROM t ORDER BY id",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(2)], vec![i(4)], vec![i(6)]]
    );
    assert_eq!(
        db.execute(
            "SELECT sum((SELECT a.x FROM t a WHERE a.id=t.id)) FROM t",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(3)]]
    );
    assert!(db.execute("SELECT * FROM t,(SELECT t.x)", &[]).is_err());
    assert!(db
        .execute("SELECT (SELECT 1 LIMIT t.id) FROM t", &[])
        .is_err());
    assert!(db
        .execute(
            "SELECT (WITH q AS(SELECT t.id) SELECT 1 LIMIT (SELECT * FROM q)) FROM t",
            &[]
        )
        .is_err());
}

#[test]
fn exists_and_in_have_empty_null_and_lazy_expression_semantics() {
    let mut db = setup();
    assert_eq!(db.execute("SELECT EXISTS(SELECT abs(-9223372036854775808) FROM t),EXISTS(SELECT 1 FROM t WHERE 0),EXISTS(SELECT count(*) FROM t WHERE 0)",&[]).unwrap().rows,vec![vec![i(1),i(0),i(1)]]);
    assert!(db
        .execute(
            "SELECT EXISTS(SELECT sum(abs(-9223372036854775808)) FROM t)",
            &[]
        )
        .is_err());
    assert_eq!(db.execute("SELECT 1 IN(SELECT x FROM t),9 IN(SELECT x FROM t),NULL IN(SELECT x FROM t WHERE 0),NULL NOT IN(SELECT x FROM t WHERE 0),'A' IN(SELECT y FROM t)",&[]).unwrap().rows,vec![vec![i(1),Value::Null,i(0),i(1),i(1)]]);
    assert_eq!(db.execute("SELECT CASE WHEN 0 THEN (SELECT abs(-9223372036854775808)) ELSE 9 END,coalesce(2,(SELECT abs(-9223372036854775808)))",&[]).unwrap().rows,vec![vec![i(9),i(2)]]);
    assert!(db
        .execute("SELECT CASE WHEN 0 THEN (SELECT missing) END", &[])
        .is_err());
}

#[test]
fn prepared_cache_is_statement_local_and_self_reading_inserts_materialize() {
    let mut db = setup();
    let p = db
        .prepare("SELECT id,(SELECT ?1+id) FROM t ORDER BY id")
        .unwrap();
    for n in [1, 4] {
        assert_eq!(
            db.execute_prepared(&p, &[i(n)]).unwrap().rows,
            vec![
                vec![i(1), i(n + 1)],
                vec![i(2), i(n + 2)],
                vec![i(3), i(n + 3)]
            ]
        );
    }
    db.execute_batch("CREATE TABLE out(x UNIQUE);INSERT INTO out VALUES((SELECT 1)),((SELECT count(*) FROM out));").unwrap();
    assert_eq!(
        db.execute("SELECT x FROM out ORDER BY rowid", &[])
            .unwrap()
            .rows,
        vec![vec![i(1)], vec![i(0)]]
    );
    assert!(db
        .execute("INSERT INTO out VALUES((SELECT 5)),((SELECT 5))", &[])
        .is_err());
    assert_eq!(
        db.execute("SELECT count(*) FROM out", &[]).unwrap().rows,
        vec![vec![i(2)]]
    );
    db.execute("BEGIN", &[]).unwrap();
    db.execute("UPDATE out SET x=x+(SELECT count(*) FROM out)", &[])
        .unwrap();
    db.execute(
        "DELETE FROM out WHERE EXISTS(SELECT 1 FROM t WHERE t.id=out.x)",
        &[],
    )
    .unwrap();
    db.execute("ROLLBACK", &[]).unwrap();
    assert_eq!(
        db.execute("SELECT x FROM out ORDER BY rowid", &[])
            .unwrap()
            .rows,
        vec![vec![i(1)], vec![i(0)]]
    );
}

#[test]
fn subqueries_obey_fuel_memory_nesting_and_schema_constraints() {
    let mut db = Connection::with_limits(SqlLimits {
        max_steps: 300,
        ..Default::default()
    });
    assert!(matches!(
        db.execute(
            "SELECT 1 IN(WITH q(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM q) SELECT x FROM q)",
            &[]
        ),
        Err(Error::Limit(_))
    ));
    let mut nested = "SELECT 1".to_string();
    for _ in 0..40 {
        nested = format!("SELECT ({nested})");
    }
    assert!(matches!(
        Connection::new().execute(&nested, &[]),
        Err(Error::Limit(_))
    ));
    let mut db = Connection::with_limits(SqlLimits {
        max_database_bytes: 800,
        ..Default::default()
    });
    let expressions = vec!["(SELECT char(65,66,67,68))"; 16].join(",");
    assert!(matches!(
        db.execute(&format!("SELECT {expressions}"), &[]),
        Err(Error::Limit(_))
    ));
    for sql in [
        "CREATE TABLE bad(x DEFAULT (SELECT 1))",
        "CREATE TABLE bad(x CHECK(EXISTS(SELECT 1)))",
    ] {
        assert!(Connection::new().execute(sql, &[]).is_err());
    }
    let seeds = [
        "SELECT (SELECT 1 WHERE EXISTS(SELECT 2))",
        "SELECT 1 NOT IN(WITH q(x) AS(VALUES(NULL)) SELECT x FROM q)",
    ];
    for seed in seeds {
        for end in 0..seed.len() {
            assert!(
                std::panic::catch_unwind(|| Connection::new().execute(&seed[..end], &[])).is_ok()
            );
        }
    }
}
