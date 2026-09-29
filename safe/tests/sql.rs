#![forbid(unsafe_code)]
use sqlite_safe::{
    sql::{Connection, SqlLimits},
    Error, Text, Value,
};
fn i(n: i64) -> Value {
    Value::Integer(n)
}
fn t(s: &str) -> Value {
    Value::Text(Text::utf8(s))
}
fn rows(c: &mut Connection, sql: &str) -> Vec<Vec<Value>> {
    c.execute(sql, &[]).unwrap().rows
}
#[test]
fn expressions_parameters_and_null_logic() {
    let mut c = Connection::new();
    assert_eq!(
        rows(
            &mut c,
            "SELECT 2+3*4, 7/2, 7/2.0, NULL AND 0, NULL OR 1, NULL IS NULL, 2 IS TRUE, 2 IS 1"
        ),
        vec![vec![
            i(14),
            i(3),
            Value::Real(3.5),
            i(0),
            i(1),
            i(1),
            i(1),
            i(0)
        ]]
    );
    assert_eq!(
        rows(
            &mut c,
            "SELECT -9223372036854775808, 0xffffffffffffffff, 1_000+2, -1>>65, 1<<-1"
        ),
        vec![vec![i(i64::MIN), i(-1), i(1002), i(-1), i(0)]]
    );
    let p = c.prepare("SELECT :x,?5,:x,?,@z").unwrap();
    assert_eq!(p.parameter_count(), 7);
    assert_eq!(p.parameter_index(":x"), Some(1));
    assert_eq!(
        c.execute_prepared(&p, &[i(4), Value::Null, Value::Null, Value::Null, t("a")])
            .unwrap()
            .rows,
        vec![vec![i(4), t("a"), i(4), Value::Null, Value::Null]]
    );
    assert_eq!(rows(&mut c,"SELECT CASE WHEN 1 THEN 7 ELSE abs(-9223372036854775808) END, coalesce(NULL,9,abs(-9223372036854775808)), NULL IN (), NULL NOT IN (), 2 IN (1,NULL)"),vec![vec![i(7),i(9),i(0),i(1),Value::Null]]);
}
#[test]
fn ddl_dml_constraints_and_affinity() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE COLLATE NOCASE, n NUMERIC DEFAULT 12 CHECK(n>=0)); INSERT INTO t(name) VALUES('one'),('two');").unwrap();
    assert_eq!(
        rows(&mut c, "SELECT id,name,n,typeof(n) FROM t ORDER BY id"),
        vec![
            vec![i(1), t("one"), i(12), t("integer")],
            vec![i(2), t("two"), i(12), t("integer")]
        ]
    );
    assert!(matches!(
        c.execute("INSERT INTO t(name) VALUES('three'),('ONE')", &[]),
        Err(Error::Constraint(_))
    ));
    assert_eq!(rows(&mut c, "SELECT count(*) FROM t"), vec![vec![i(2)]]);
    assert_eq!(c.changes(), 0);
    assert!(c.execute("UPDATE t SET n=-1", &[]).is_err());
    c.execute("UPDATE t SET n=n+1,name=upper(name) WHERE id=2", &[])
        .unwrap();
    assert_eq!(
        rows(&mut c, "SELECT id,name,n FROM t WHERE n='13'"),
        vec![vec![i(2), t("TWO"), i(13)]]
    );
    c.execute("DELETE FROM t WHERE id=1", &[]).unwrap();
    assert_eq!(c.changes(), 1);
    assert_eq!(
        rows(&mut c, "SELECT rowid,oid,_rowid_ FROM t"),
        vec![vec![i(2), i(2), i(2)]]
    );
}
#[test]
fn transactions_savepoints_and_statement_rollback() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(x UNIQUE); BEGIN; INSERT INTO t VALUES(1); SAVEPOINT s; INSERT INTO t VALUES(2); ROLLBACK TO s; INSERT INTO t VALUES(3); RELEASE s;").unwrap();
    assert!(!c.is_autocommit());
    assert_eq!(
        rows(&mut c, "SELECT x FROM t ORDER BY x"),
        vec![vec![i(1)], vec![i(3)]]
    );
    c.execute("ROLLBACK", &[]).unwrap();
    assert!(c.is_autocommit());
    assert!(rows(&mut c, "SELECT * FROM t").is_empty());
    c.execute_batch("SAVEPOINT outer; INSERT INTO t VALUES(7); SAVEPOINT inner; INSERT INTO t VALUES(8); RELEASE outer;").unwrap();
    assert!(c.is_autocommit());
    assert_eq!(
        rows(&mut c, "SELECT sum(x),count(*) FROM t"),
        vec![vec![i(15), i(2)]]
    );
    assert!(c.execute("UPDATE t SET x=9", &[]).is_err());
    assert_eq!(
        rows(&mut c, "SELECT x FROM t ORDER BY x"),
        vec![vec![i(7)], vec![i(8)]]
    );
}
#[test]
fn joins_grouping_having_sorting_and_distinct() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE people(id INTEGER PRIMARY KEY,name TEXT); CREATE TABLE purchases(person INTEGER,amount REAL); INSERT INTO people VALUES(1,'Ada'),(2,'Lin'),(3,'Empty'); INSERT INTO purchases VALUES(1,2.5),(1,3.5),(2,10.0);").unwrap();
    assert_eq!(rows(&mut c,"SELECT p.name,sum(o.amount) total,count(o.amount) n FROM people p LEFT JOIN purchases o ON o.person=p.id GROUP BY p.id HAVING n>0 ORDER BY total DESC LIMIT 2"),vec![vec![t("Lin"),Value::Real(10.0),i(1)],vec![t("Ada"),Value::Real(6.0),i(2)]]);
    assert_eq!(
        rows(
            &mut c,
            "SELECT DISTINCT person FROM purchases ORDER BY 1 DESC"
        ),
        vec![vec![i(2)], vec![i(1)]]
    );
    assert_eq!(rows(&mut c,"SELECT p.name, o.amount FROM people p LEFT JOIN purchases o ON p.id=o.person WHERE o.amount IS NULL"),vec![vec![t("Empty"),Value::Null]]);
    assert!(c
        .execute(
            "SELECT person,sum(amount) FROM purchases WHERE sum(amount)>0",
            &[]
        )
        .is_err());
    assert!(c
        .execute("SELECT nonexistent FROM purchases WHERE 0", &[])
        .is_err());
    assert!(c
        .execute("SELECT sum(count(*)) FROM purchases", &[])
        .is_err());
}
#[test]
fn scalar_functions_unicode_binary_and_nul() {
    let mut c = Connection::new();
    assert_eq!(rows(&mut c,"SELECT lower('ÄBC'), upper('éab'), length('🦀é'), hex(x'00ff'), unhex('00 FF',' '), substr('abc🦀',-2), instr('é🦀a','a'), trim('xyabcxy','xy')"),vec![vec![t("Äbc"),t("éAB"),i(2),t("00FF"),Value::Blob(vec![0,255]),t("c🦀"),i(3),t("abc")]]);
    assert_eq!(rows(&mut c,"SELECT length('a'||char(0)||'b'), octet_length('a'||char(0)||'b'), 'abc' LIKE 'A%', 'æ' LIKE 'Æ', like('a!_%','a_xyz','!')"),vec![vec![i(1),i(3),i(1),i(0),i(1)]]);
    assert_eq!(rows(&mut c,"SELECT CAST('123e5' AS INTEGER), CAST('123e5' AS NUMERIC), '123e5'+0, 'abc'+0, CAST(12 AS TEXT)"),vec![vec![i(123),i(12300000),Value::Real(12300000.0),i(0),t("12")]]);
}
#[test]
fn images_preserve_schema_rowid_aliases_and_defaults() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,n REAL DEFAULT 3.5,name TEXT NOT NULL CHECK(length(name)>0)); INSERT INTO t(name) VALUES('first'),('second'); PRAGMA user_version=42;").unwrap();
    for size in [512, 4096, 65536] {
        let image = c.to_image(size).unwrap();
        let mut loaded = Connection::from_image(&image).unwrap();
        assert_eq!(
            rows(&mut loaded, "SELECT * FROM t"),
            rows(&mut c, "SELECT * FROM t")
        );
        assert_eq!(rows(&mut loaded, "PRAGMA user_version"), vec![vec![i(42)]]);
        loaded
            .execute("INSERT INTO t(name) VALUES('third')", &[])
            .unwrap();
        assert_eq!(loaded.last_insert_rowid(), 3);
    }
    c.execute("CREATE TABLE indexed(x UNIQUE)", &[]).unwrap();
    assert!(Connection::from_image(&c.to_image(512).unwrap()).is_ok());
}
#[test]
fn bounded_parser_and_execution_errors_leave_state_intact() {
    let mut c = Connection::new();
    let aliased = format!(
        "SELECT {}1 AS x WHERE x{}",
        "1+".repeat(40),
        "+1".repeat(40)
    );
    assert!(matches!(c.execute(&aliased, &[]), Err(Error::Limit(_))));
    let nested = format!("SELECT {}1{}", "(".repeat(10000), ")".repeat(10000));
    assert!(c.execute(&nested, &[]).is_err());
    let chain = format!("SELECT {}1", "1+".repeat(10000));
    assert!(c.execute(&chain, &[]).is_err());
    assert!(c.execute("SELECT 1__0", &[]).is_err());
    assert!(c.execute("SELECT ?0", &[]).is_err());
    assert!(c.execute("SELECT x'1'", &[]).is_err());
    assert!(c.execute("SELECT 'abc", &[]).is_err());
    let mut c = Connection::with_limits(SqlLimits {
        max_rows: 2,
        ..SqlLimits::default()
    });
    c.execute("CREATE TABLE t(x)", &[]).unwrap();
    assert!(c.execute("INSERT INTO t VALUES(1),(2),(3)", &[]).is_err());
    assert!(rows(&mut c, "SELECT x FROM t").is_empty());
    c.execute("INSERT INTO t VALUES(1),(2)", &[]).unwrap();
    assert!(c
        .execute("SELECT a.x,b.x FROM t a CROSS JOIN t b", &[])
        .is_err());
}

#[test]
fn defaults_rowid_state_and_empty_limit_regressions() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY DEFAULT 10,x DEFAULT(abs(-9223372036854775808))); INSERT INTO t(x) VALUES(last_insert_rowid()),(last_insert_rowid());").unwrap();
    assert_eq!(
        rows(&mut c, "SELECT id,x FROM t ORDER BY id"),
        vec![vec![i(1), i(0)], vec![i(2), i(1)]]
    );
    assert!(rows(&mut c, "SELECT abs(-9223372036854775808) FROM t LIMIT 0").is_empty());
    assert!(c.execute("SELECT id FROM t ORDER BY -1", &[]).is_err());
    assert_eq!(
        rows(&mut c, "SELECT -id AS id FROM t ORDER BY id"),
        vec![vec![i(-2)], vec![i(-1)]]
    );
    assert_eq!(
        rows(&mut c, "SELECT count(),sum(1.0),total(x),avg(x) FROM t"),
        vec![vec![
            i(2),
            Value::Real(2.0),
            Value::Real(1.0),
            Value::Real(0.5)
        ]]
    );
}

#[test]
fn allocation_budgets_rollback_and_batch_output_limits() {
    let mut c = Connection::with_limits(SqlLimits {
        max_database_bytes: 512,
        ..SqlLimits::default()
    });
    c.execute("CREATE TABLE t(x)", &[]).unwrap();
    c.execute("INSERT INTO t VALUES(1)", &[]).unwrap();
    let before = c.total_changes();
    let mut statement = String::from("INSERT INTO t VALUES");
    statement.push_str(
        &(0..20)
            .map(|_| "('abcdefghijklmnop')")
            .collect::<Vec<_>>()
            .join(","),
    );
    assert!(c.execute(&statement, &[]).is_err());
    assert_eq!(c.total_changes(), before);
    assert_eq!(rows(&mut c, "SELECT x FROM t"), vec![vec![i(1)]]);
    let script = format!(
        "SELECT '{}' AS x; SELECT '{}' AS x;",
        "x".repeat(300),
        "y".repeat(300)
    );
    assert!(matches!(c.execute_batch(&script), Err(Error::Limit(_))));
}

#[test]
fn malformed_sql_mutations_never_panic_or_escape_budgets() {
    let seeds=[
        "SELECT CASE WHEN 1 THEN ('héllo' || char(0)) ELSE abs(-9223372036854775808) END;",
        "CREATE TABLE t(id INTEGER PRIMARY KEY,x TEXT DEFAULT 'abc',n NUMERIC CHECK(n>0)); INSERT INTO t(n) VALUES(1),(2); SELECT x,sum(n) FROM t GROUP BY x ORDER BY 1;",
        "SELECT CAST('123e5' AS NUMERIC),x'00ff',1_000,1 NOT IN(NULL),?1,:arg,typeof(1.5);",
        "CREATE TABLE t(x UNIQUE);BEGIN;INSERT INTO t VALUES(1),(2);SAVEPOINT s;UPDATE t SET x=x+3;ROLLBACK TO s;COMMIT;",
    ];
    let limits = SqlLimits {
        max_sql_bytes: 4096,
        max_tokens: 1000,
        max_expr_depth: 24,
        max_rows: 30,
        max_value_bytes: 4096,
        max_database_bytes: 16384,
        max_steps: 2000,
        ..SqlLimits::default()
    };
    let mut random = 0x5647382910abcdefu64;
    for iteration in 0..5000 {
        let mut bytes = seeds[iteration % seeds.len()].as_bytes().to_vec();
        for _ in 0..1 + iteration % 5 {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            let at = random as usize % bytes.len();
            bytes[at] = (random >> 32) as u8;
        }
        if iteration % 5 == 0 {
            bytes.truncate(random as usize % bytes.len());
        }
        let sql = String::from_utf8_lossy(&bytes);
        let outcome = std::panic::catch_unwind(|| {
            let mut c = Connection::with_limits(limits);
            let _ = c.execute_batch(&sql);
        });
        assert!(outcome.is_ok(), "SQL mutation {iteration}: {sql}");
    }
}

#[test]
fn database_encoding_and_application_metadata_survive_sql_updates() {
    use sqlite_safe::{Database, Encoding, ImageBuilder, Table, Text};
    for (encoding, order) in [
        (Encoding::Utf8, vec!["a", "Ā", "\u{e000}", "🦀"]),
        (Encoding::Utf16Le, vec!["Ā", "\u{e000}", "🦀", "a"]),
        (Encoding::Utf16Be, vec!["a", "Ā", "🦀", "\u{e000}"]),
    ] {
        let mut builder = ImageBuilder::new(512).unwrap().encoding(encoding).unwrap();
        let mut table = Table::new("t", &["v"]);
        for (n, value) in ["a", "Ā", "🦀", "\u{e000}"].iter().enumerate() {
            table.rows.push((
                n as i64,
                vec![Value::Text(Text::utf8(value).transcode(encoding).unwrap())],
            ));
        }
        builder.add_table(table).unwrap();
        let mut c = Connection::from_image(&builder.finish().unwrap()).unwrap();
        assert_eq!(
            rows(&mut c, "SELECT v FROM t ORDER BY v"),
            order.iter().map(|s| vec![t(s)]).collect::<Vec<_>>()
        );
        c.execute_batch("CREATE INDEX idx ON t(v DESC);PRAGMA application_id=1196444487;PRAGMA user_version=42;BEGIN;PRAGMA application_id=5;ROLLBACK;").unwrap();
        assert_eq!(
            rows(&mut c, "PRAGMA application_id"),
            vec![vec![i(1196444487)]]
        );
        let image = c.to_image(512).unwrap();
        let physical = Database::parse(&image).unwrap();
        assert_eq!(physical.header().encoding, encoding);
        assert_eq!(physical.header().application_id, 1196444487);
        assert_eq!(physical.header().user_version, 42);
        c = Connection::from_image(&image).unwrap();
        assert_eq!(
            rows(&mut c, "SELECT CAST(CAST(12 AS BLOB) AS INTEGER)"),
            vec![vec![i(12)]]
        );
        assert_eq!(
            rows(&mut c, "SELECT octet_length('abc')"),
            vec![vec![i(if encoding == Encoding::Utf8 { 3 } else { 6 })]]
        );
        c.execute("PRAGMA user_version=4294967295", &[]).unwrap();
        assert_eq!(rows(&mut c, "PRAGMA user_version"), vec![vec![i(0)]]);
    }
}
