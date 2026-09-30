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
fn column_rename_preserves_keys_generated_values_indexes_and_counters() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,a INT UNIQUE CHECK(t.a>0),s AS(a*2) STORED,g AS(s+1));INSERT INTO t(a) VALUES(2),(3);CREATE INDEX ix ON t((a+s+g)) WHERE t.a>0;CREATE VIEW v AS SELECT id,a AS value,s,g FROM t;").unwrap();
    let before = (c.changes(), c.total_changes(), c.last_insert_rowid());
    let view = c.prepare("SELECT * FROM v ORDER BY id").unwrap();
    let stale = c.prepare("UPDATE t SET a=?1 WHERE id=?2").unwrap();
    c.execute("ALTER TABLE main.t RENAME COLUMN a TO \"é new🦀\"", &[])
        .unwrap();
    assert_eq!(
        (c.changes(), c.total_changes(), c.last_insert_rowid()),
        before
    );
    assert!(c.execute_prepared(&stale, &[i(4), i(1)]).is_err());
    assert_eq!(
        c.execute_prepared(&view, &[]).unwrap().rows,
        vec![vec![i(1), i(2), i(4), i(5)], vec![i(2), i(3), i(6), i(7)]]
    );
    assert!(sql(&c, "t").contains("CHECK(t.\"é new🦀\">0)"));
    assert!(sql(&c, "ix").contains("((\"é new🦀\"+s+g))"));
    c.execute("INSERT INTO t(\"é new🦀\") VALUES(4)", &[])
        .unwrap();
    assert_eq!(c.last_insert_rowid(), 3);
    assert!(c
        .execute("INSERT INTO t(\"é new🦀\") VALUES(4)", &[])
        .is_err());
    c.execute("ALTER TABLE t RENAME id TO key", &[]).unwrap();
    assert_eq!(
        c.execute("SELECT rowid,oid,_rowid_ FROM t ORDER BY key", &[])
            .unwrap()
            .columns,
        vec!["key", "key", "key"]
    );
    assert_eq!(
        c.execute("SELECT * FROM sqlite_sequence", &[])
            .unwrap()
            .rows,
        vec![vec![Value::Text(Text::utf8("t")), i(3)]]
    );
}

#[test]
fn column_rename_normalizes_literals_and_preserves_tokens_comments_and_defaults() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t([a] INT PRIMARY KEY,b AS(\"future\"),CHECK(a>0));CREATE INDEX ix ON t('a');CREATE VIEW v AS SELECT /* a */ \"a\",'a',\"future\",(\"a'b\") FROM t;CREATE TABLE u(x CHECK(\"literal\"),y AS(\"future\"),z DEFAULT \"default\");INSERT INTO t(a) VALUES(1);").unwrap();
    c.execute("ALTER TABLE t RENAME a TO future", &[]).unwrap();
    assert_eq!(
        sql(&c, "t"),
        "CREATE TABLE t(\"future\" INT PRIMARY KEY,b AS('future'),CHECK(future>0))"
    );
    assert_eq!(sql(&c, "ix"), "CREATE INDEX ix ON t(\"future\")");
    assert_eq!(
        sql(&c, "v"),
        "CREATE VIEW v AS SELECT /* a */ \"future\",'a','future',('a''b') FROM t"
    );
    assert_eq!(
        sql(&c, "u"),
        "CREATE TABLE u(x CHECK('literal'),y AS('future'),z DEFAULT \"default\")"
    );
    assert_eq!(
        c.execute("SELECT * FROM t", &[]).unwrap().rows,
        vec![vec![i(1), Value::Text(Text::utf8("future"))]]
    );
    c.execute("ALTER TABLE t RENAME future TO FUTURE", &[])
        .unwrap();
    assert!(sql(&c, "t").contains("\"FUTURE\" INT"));
}

#[test]
fn column_rename_scopes_aliases_compounds_ctes_and_hidden_rowid_references() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a INTEGER PRIMARY KEY,b);INSERT INTO t VALUES(7,9);CREATE VIEW v AS SELECT rowid,oid,_rowid_,a FROM t;CREATE VIEW w AS SELECT a AS a FROM t UNION ALL SELECT a FROM t ORDER BY a;CREATE VIEW q AS WITH unused AS (SELECT a,missing FROM t),cte(x) AS (SELECT a FROM t) SELECT x FROM cte;CREATE VIEW outer_view AS SELECT (SELECT a) AS result FROM t;").unwrap();
    c.execute("ALTER TABLE t RENAME a TO c", &[]).unwrap();
    assert_eq!(sql(&c, "v"), "CREATE VIEW v AS SELECT c,c,c,c FROM t");
    assert_eq!(
        sql(&c, "w"),
        "CREATE VIEW w AS SELECT c AS a FROM t UNION ALL SELECT c FROM t ORDER BY a"
    );
    assert!(sql(&c, "q").contains("SELECT c,missing FROM t"));
    assert_eq!(
        c.execute("SELECT * FROM outer_view", &[]).unwrap().rows,
        vec![vec![i(7)]]
    );
    assert_eq!(
        c.execute("SELECT * FROM q", &[]).unwrap().rows,
        vec![vec![i(7)]]
    );
}

#[test]
fn column_rename_dependency_errors_and_savepoints_restore_all_state() {
    for view in [
        "SELECT * FROM t ORDER BY a",
        "SELECT * FROM t UNION ALL SELECT * FROM t ORDER BY a",
        "SELECT q.a FROM (SELECT a FROM t) q",
        "SELECT a FROM t JOIN u USING(a)",
        "SELECT * FROM absent",
    ] {
        let mut c = Connection::new();
        c.execute_batch("CREATE TABLE t(a,b);CREATE TABLE u(a);INSERT INTO t VALUES(1,2);")
            .unwrap();
        c.execute(&format!("CREATE VIEW v AS {view}"), &[]).unwrap();
        let before = c.to_image(512).unwrap();
        assert!(
            c.execute("ALTER TABLE t RENAME a TO c", &[]).is_err(),
            "{view}"
        );
        assert_eq!(c.to_image(512).unwrap(), before);
    }
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a,b);INSERT INTO t VALUES(1,2);CREATE INDEX ix ON t(a);")
        .unwrap();
    let before = c.to_image(512).unwrap();
    for statement in [
        "ALTER TABLE t RENAME a TO b",
        "ALTER TABLE t RENAME missing TO c",
        "ALTER TABLE t RENAME rowid TO c",
    ] {
        assert!(c.execute(statement, &[]).is_err());
        assert_eq!(c.to_image(512).unwrap(), before);
    }
    c.execute_batch("BEGIN;ALTER TABLE t RENAME a TO c;SAVEPOINT s;ALTER TABLE t RENAME b TO d;INSERT INTO t VALUES(3,4);ROLLBACK TO s;").unwrap();
    assert_eq!(
        c.execute("SELECT * FROM t", &[]).unwrap().columns,
        vec!["c", "b"]
    );
    c.execute("ROLLBACK", &[]).unwrap();
    assert_eq!(c.to_image(512).unwrap(), before);
    c.execute("CREATE INDEX bad ON t(\"missing\")", &[])
        .unwrap();
    let before = c.to_image(512).unwrap();
    assert!(c.execute("ALTER TABLE t RENAME a TO c", &[]).is_err());
    assert_eq!(c.to_image(512).unwrap(), before);
}

#[test]
fn quoted_string_binding_distinguishes_identifiers_defaults_and_result_labels() {
    let mut c = Connection::new();
    let result = c
        .execute("SELECT \"text\",(\"text\"),\"st\"\"r\"", &[])
        .unwrap();
    assert_eq!(
        result.columns,
        vec!["\"text\"", "(\"text\")", "\"st\"\"r\""]
    );
    assert_eq!(
        result.rows,
        vec![vec![
            Value::Text(Text::utf8("text")),
            Value::Text(Text::utf8("text")),
            Value::Text(Text::utf8("st\"r"))
        ]]
    );
    for statement in [
        "SELECT [text]",
        "SELECT `text`",
        "CREATE TABLE bad(a DEFAULT (\"missing\"))",
    ] {
        assert!(c.execute(statement, &[]).is_err());
    }
    c.execute_batch("CREATE TABLE t(a INTEGER PRIMARY KEY,b AS(a+1));INSERT INTO t(a) VALUES(3);")
        .unwrap();
    assert_eq!(
        c.execute("SELECT A,RowId,B FROM t", &[]).unwrap().columns,
        vec!["a", "a", "b"]
    );
}

#[test]
fn column_rename_limits_and_malformed_input_do_not_publish_partial_changes() {
    let statement = "ALTER TABLE t RENAME COLUMN a TO renamed";
    let mut c = Connection::with_limits(SqlLimits {
        max_steps: 150,
        ..SqlLimits::default()
    });
    c.execute("CREATE TABLE t(a)", &[]).unwrap();
    for _ in 0..200 {
        c.execute("INSERT INTO t VALUES(1)", &[]).unwrap();
    }
    let before = c.to_image(512).unwrap();
    assert!(matches!(c.execute(statement, &[]), Err(Error::Limit(_))));
    assert_eq!(c.to_image(512).unwrap(), before);
    for end in 1..statement.len() {
        let _ = c.prepare(&statement[..end]);
    }
    let mut c = Connection::with_limits(SqlLimits {
        max_sql_bytes: 150,
        ..SqlLimits::default()
    });
    c.execute_batch(
        "CREATE TABLE t(a);CREATE VIEW v AS SELECT a,a,a,a,a,a,a,a,a,a,a,a,a,a,a,a,a,a,a,a FROM t;",
    )
    .unwrap();
    let before = c.to_image(512).unwrap();
    assert!(matches!(
        c.execute("ALTER TABLE t RENAME a TO very_long_name", &[]),
        Err(Error::Limit(_))
    ));
    assert_eq!(c.to_image(512).unwrap(), before);
}

#[test]
fn column_rename_roundtrips_all_image_configurations_and_table_kinds() {
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
                    c.execute_batch(&format!("CREATE TABLE {name}(id INT PRIMARY KEY,a INT,s INT AS(a*2) STORED,g INT AS(s+1)) STRICT{suffix};INSERT INTO {name}(id,a) VALUES(1,2);CREATE INDEX ix_{name} ON {name}((a+s+g)) WHERE a>0;CREATE VIEW v_{name} AS SELECT id,a AS value,s,g FROM {name};ALTER TABLE {name} RENAME a TO renamed;")).unwrap();
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
                    c.execute_batch(&format!(
                        "ALTER TABLE {name} RENAME renamed TO final;UPDATE {name} SET final=3;"
                    ))
                    .unwrap();
                }
                let bytes = c.to_image(size).unwrap();
                assert_eq!(Database::parse(&bytes).unwrap().header().encoding, encoding);
            }
        }
    }
}
