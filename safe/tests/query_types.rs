#![forbid(unsafe_code)]
use sqlite_safe::{sql::Connection, Text, Value};

fn text(s: &str) -> Value {
    Value::Text(Text::utf8(s))
}
fn types(db: &mut Connection, name: &str) -> Vec<Value> {
    db.execute(&format!("PRAGMA table_info({name})"), &[])
        .unwrap()
        .rows
        .into_iter()
        .map(|r| r[2].clone())
        .collect()
}

#[test]
fn compound_types_are_inferred_from_expressions_not_current_rows() {
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE t(n NUMERIC,s TEXT);INSERT INTO t VALUES(1,'a');CREATE VIEW v AS SELECT n x FROM t UNION ALL SELECT '0123';CREATE TABLE z AS SELECT * FROM v;CREATE VIEW empty AS SELECT s FROM t WHERE 0 UNION ALL SELECT 1 WHERE 0;").unwrap();
    assert_eq!(types(&mut db, "v"), vec![text("BLOB")]);
    assert_eq!(types(&mut db, "z"), vec![text("")]);
    assert_eq!(types(&mut db, "empty"), vec![text("BLOB")]);
    assert_eq!(
        db.execute("SELECT x FROM z ORDER BY rowid", &[])
            .unwrap()
            .rows,
        vec![vec![Value::Integer(1)], vec![text("0123")]]
    );
}

#[test]
fn compound_numeric_casts_preserve_real_values_during_table_creation() {
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE z AS SELECT CAST(1 AS REAL) x UNION ALL SELECT 2;CREATE VIEW v AS SELECT CAST(1 AS INT) x UNION ALL SELECT 2.0;").unwrap();
    assert_eq!(types(&mut db, "z"), vec![text("NUM")]);
    assert_eq!(types(&mut db, "v"), vec![text("NUM")]);
    let expected = vec![vec![Value::Real(1.0)], vec![Value::Integer(2)]];
    assert_eq!(
        db.execute("SELECT x FROM z ORDER BY rowid", &[])
            .unwrap()
            .rows,
        expected
    );
    let mut reopened = Connection::from_image(&db.to_image(512).unwrap()).unwrap();
    assert_eq!(
        reopened
            .execute("SELECT x FROM z ORDER BY rowid", &[])
            .unwrap()
            .rows,
        expected
    );
}

#[test]
fn scalar_compound_affinity_comes_from_the_final_select() {
    let mut db = Connection::new();
    assert_eq!(db.execute("SELECT (SELECT 1 UNION ALL SELECT CAST(2 AS INT))='1',(SELECT CAST(1 AS INT) UNION ALL SELECT 2)='1'", &[]).unwrap().rows, vec![vec![Value::Integer(1), Value::Integer(0)]]);
    db.execute_batch(
        "CREATE TABLE t(a,b BLOB);CREATE VIEW v AS SELECT a,b,CAST(1 AS BLOB) c FROM t;",
    )
    .unwrap();
    assert_eq!(types(&mut db, "v"), vec![text("BLOB"); 3]);
    db.execute_batch("CREATE TABLE typed(a VARCHAR(10),b TEXT);CREATE VIEW first AS SELECT a x FROM typed UNION ALL SELECT b FROM typed;CREATE VIEW second AS SELECT * FROM first;CREATE VIEW third AS WITH q(x) AS(SELECT a FROM typed UNION ALL SELECT b FROM typed) SELECT x FROM q;").unwrap();
    assert_eq!(types(&mut db, "first"), vec![text("VARCHAR(10)")]);
    assert_eq!(types(&mut db, "second"), vec![text("TEXT")]);
    assert_eq!(types(&mut db, "third"), vec![text("TEXT")]);
    db.execute("INSERT INTO typed VALUES('A','B')", &[])
        .unwrap();
    assert_eq!(db.execute("SELECT (SELECT 1 UNION ALL SELECT a FROM typed)='1','1' IN(SELECT 1 UNION ALL SELECT a FROM typed)", &[]).unwrap().rows, vec![vec![Value::Integer(1),Value::Integer(1)]]);
    assert_eq!(db.execute("SELECT (SELECT 1.0 UNION ALL SELECT a FROM typed)=1,1 IN(SELECT 1.0 UNION ALL SELECT a FROM typed),9007199254740993 IN(SELECT CAST(9007199254740993 AS REAL)),9007199254740993=CAST(9007199254740993 AS REAL)", &[]).unwrap().rows, vec![vec![Value::Integer(1),Value::Integer(0),Value::Integer(1),Value::Integer(0)]]);
}

#[test]
fn values_and_recursive_sources_propagate_compound_metadata() {
    let mut db = Connection::new();
    db.execute_batch("CREATE VIEW v AS WITH q(x) AS(VALUES(CAST(1 AS INT)),('2')) SELECT * FROM q;CREATE VIEW r AS WITH RECURSIVE q(x) AS(SELECT CAST(1 AS INT) UNION ALL SELECT 'z' FROM q WHERE x<2) SELECT * FROM q;CREATE TABLE z AS SELECT * FROM r;").unwrap();
    assert_eq!(types(&mut db, "v"), vec![text("BLOB")]);
    assert_eq!(types(&mut db, "r"), vec![text("BLOB")]);
    assert_eq!(
        db.execute("SELECT x FROM z ORDER BY rowid", &[])
            .unwrap()
            .rows,
        vec![vec![Value::Integer(1)], vec![text("z")]]
    );
    db.execute_batch("CREATE TABLE t(n INTEGER);CREATE VIEW declared AS WITH RECURSIVE q(x) AS(SELECT n FROM t UNION ALL SELECT x+1 FROM q WHERE x<3) SELECT * FROM q;").unwrap();
    assert_eq!(types(&mut db, "declared"), vec![text("INT")]);
    db.execute(
        "CREATE VIEW compound_values AS VALUES(CAST(1 AS TEXT)),(2) UNION ALL SELECT NULL",
        &[],
    )
    .unwrap();
    assert_eq!(types(&mut db, "compound_values"), vec![text("BLOB")]);
    assert_eq!(
        db.execute("VALUES('a' COLLATE NOCASE),('A') UNION SELECT 'a'", &[])
            .unwrap()
            .rows,
        vec![vec![text("a")]]
    );
}
