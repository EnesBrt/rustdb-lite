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
fn add_defaults_affinity_counters_and_prepared_rebinding() {
    let mut c = Connection::new();
    c.execute_batch(
        "CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,a);INSERT INTO t VALUES(8,'old');",
    )
    .unwrap();
    let query = c.prepare("SELECT * FROM t WHERE id=?1").unwrap();
    let add = c
        .prepare("ALTER TABLE main.t ADD [é,col] TEXT DEFAULT 1.20e2")
        .unwrap();
    c.execute_prepared(&add, &[]).unwrap();
    c.execute("ALTER TABLE t ADD n INT DEFAULT '003'", &[])
        .unwrap();
    c.execute("ALTER TABLE t ADD b BLOB DEFAULT x'0032'", &[])
        .unwrap();
    assert_eq!(
        (c.changes(), c.total_changes(), c.last_insert_rowid()),
        (1, 1, 8)
    );
    assert_eq!(
        c.execute_prepared(&query, &[i(8)]).unwrap().rows,
        vec![vec![
            i(8),
            t("old"),
            t("1.20e2"),
            i(3),
            Value::Blob(vec![0, 50])
        ]]
    );
    // Future INSERTs evaluate their defaults using normal SQL expression rules.
    c.execute("INSERT INTO t(a) VALUES('new')", &[]).unwrap();
    assert_eq!(
        c.execute("SELECT [é,col],n FROM t WHERE id=9", &[])
            .unwrap()
            .rows,
        vec![vec![t("120.0"), i(3)]]
    );
    assert!(c.execute_prepared(&add, &[]).is_err());
    c.execute("ALTER TABLE t DROP a", &[]).unwrap();
    assert_eq!(
        c.execute_prepared(&query, &[i(8)]).unwrap().columns,
        vec!["id", "é,col", "n", "b"]
    );
    assert_eq!(
        c.execute("SELECT seq FROM sqlite_sequence", &[])
            .unwrap()
            .rows,
        vec![vec![i(9)]]
    );
}

#[test]
fn add_empty_tables_strict_and_generated_constraints() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a INT);ALTER TABLE t ADD b INT NOT NULL;ALTER TABLE t ADD c INT DEFAULT (1+2);ALTER TABLE t ADD s INT AS(a+b) STORED;INSERT INTO t(a,b) VALUES(2,4);").unwrap();
    assert_eq!(
        c.execute("SELECT * FROM t", &[]).unwrap().rows,
        vec![vec![i(2), i(4), i(3), i(6)]]
    );
    let image = c.to_image(512).unwrap();
    for sql in [
        "ALTER TABLE t ADD d INT NOT NULL",
        "ALTER TABLE t ADD d INT DEFAULT (1+2)",
        "ALTER TABLE t ADD d INT AS(a) STORED",
        "ALTER TABLE t ADD d INT UNIQUE",
        "ALTER TABLE t ADD d INT AS(NULL) NOT NULL",
        "ALTER TABLE t ADD d INT CHECK(d>0) DEFAULT -1",
    ] {
        assert!(c.execute(sql, &[]).is_err(), "{sql}");
        assert_eq!(c.to_image(512).unwrap(), image);
    }
    c.execute("ALTER TABLE t ADD v INT AS(s+c) CHECK(v>0)", &[])
        .unwrap();
    assert_eq!(
        c.execute("SELECT v FROM t", &[]).unwrap().rows,
        vec![vec![i(9)]]
    );
    c.execute_batch("CREATE TABLE strict(a INT) STRICT;INSERT INTO strict VALUES(1);")
        .unwrap();
    assert!(c
        .execute("ALTER TABLE strict ADD b INT DEFAULT 'bad'", &[])
        .is_err());
    c.execute("ALTER TABLE strict ADD b ANY DEFAULT '003'", &[])
        .unwrap();
    assert_eq!(
        c.execute("SELECT b FROM strict", &[]).unwrap().rows,
        vec![vec![t("003")]]
    );
    // ALTER can introduce a virtual cycle; binding its value and writing fail.
    c.execute("ALTER TABLE t ADD cycle AS(cycle)", &[]).unwrap();
    assert!(c.execute("SELECT cycle FROM t", &[]).is_err());
    assert!(c.execute("INSERT INTO t(a,b) VALUES(5,6)", &[]).is_err());
    assert_eq!(
        c.execute("SELECT a FROM t", &[]).unwrap().rows,
        vec![vec![i(2)]]
    );
    let mut restored = Connection::from_image(&c.to_image(512).unwrap()).unwrap();
    assert_eq!(
        restored.execute("SELECT a FROM t", &[]).unwrap().rows,
        vec![vec![i(2)]]
    );
    restored.execute("ALTER TABLE t DROP cycle", &[]).unwrap();
}

#[test]
fn drop_rebinds_indexes_and_preserves_stored_values_and_rowids() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(dropme,a INTEGER PRIMARY KEY,b,s AS(b*2) STORED,v AS(s+1));INSERT INTO t VALUES('remove',9,4);CREATE INDEX ix ON t((s+v)) WHERE b>0;CREATE VIEW viewed AS SELECT a,b,s,v FROM t;ALTER TABLE t DROP dropme;").unwrap();
    assert_eq!(
        c.execute("SELECT rowid,* FROM t", &[]).unwrap().rows,
        vec![vec![i(9), i(9), i(4), i(8), i(9)]]
    );
    assert_eq!(
        c.execute("SELECT * FROM viewed", &[]).unwrap().rows,
        vec![vec![i(9), i(4), i(8), i(9)]]
    );
    c.execute("UPDATE t SET b=5", &[]).unwrap();
    assert_eq!(
        c.execute("SELECT s,v FROM t", &[]).unwrap().rows,
        vec![vec![i(10), i(11)]]
    );
    let image = c.to_image(512).unwrap();
    for name in ["a", "b", "s", "v", "rowid", "missing"] {
        assert!(c
            .execute(&format!("ALTER TABLE t DROP {name}"), &[])
            .is_err());
        assert_eq!(c.to_image(512).unwrap(), image);
    }
    c.execute_batch("DROP VIEW viewed;DROP INDEX ix;ALTER TABLE t DROP v;ALTER TABLE t DROP s;ALTER TABLE t DROP b;").unwrap();
    assert_eq!(
        c.execute("SELECT * FROM t", &[]).unwrap().rows,
        vec![vec![i(9)]]
    );
}

#[test]
fn drop_dependencies_and_schema_comments() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a,b,c AS(\"b\"+1));CREATE INDEX ix ON t((\"b\"+1));CREATE VIEW v AS SELECT b FROM t;").unwrap();
    assert!(c.execute("ALTER TABLE t DROP b", &[]).is_err());
    c.execute_batch("DROP VIEW v;DROP INDEX ix;ALTER TABLE t DROP c;")
        .unwrap();
    c.execute("CREATE TABLE u(a CHECK(b>0),b)", &[]).unwrap();
    assert!(c.execute("ALTER TABLE u DROP b", &[]).is_err());
    c.execute("ALTER TABLE u DROP a", &[]).unwrap();
    c.execute("CREATE VIEW broken AS SELECT * FROM nonexistent", &[])
        .unwrap();
    assert!(c.execute("ALTER TABLE t DROP b", &[]).is_err());
    // ADD does not validate unrelated deferred view definitions.
    c.execute("ALTER TABLE t ADD z DEFAULT 'ok'", &[]).unwrap();
    c.execute("DROP VIEW broken", &[]).unwrap();
    c.execute("ALTER TABLE t DROP b", &[]).unwrap();
    c.execute(
        "CREATE TABLE quoted(\"é\", /* , */ [b,c] DEFAULT 'x,y', d /* tail */, UNIQUE(\"é\"))",
        &[],
    )
    .unwrap();
    c.execute("ALTER TABLE quoted ADD e CHECK(e IN (1,2)) DEFAULT 1", &[])
        .unwrap();
    c.execute("ALTER TABLE quoted DROP [b,c]", &[]).unwrap();
    c.execute("ALTER TABLE quoted DROP e", &[]).unwrap();
    assert_eq!(
        c.execute("PRAGMA table_info(quoted)", &[])
            .unwrap()
            .rows
            .len(),
        2
    );
    c.to_image(512).unwrap();
}

#[test]
fn alter_failures_savepoints_and_rollback_restore_whole_schema() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a,b);INSERT INTO t VALUES(1,2);CREATE INDEX ix ON t(b);")
        .unwrap();
    let image = c.to_image(512).unwrap();
    c.execute_batch("BEGIN;ALTER TABLE t ADD c DEFAULT 3;SAVEPOINT s;DROP INDEX ix;ALTER TABLE t DROP b;ROLLBACK TO s;").unwrap();
    assert_eq!(
        c.execute("SELECT * FROM t", &[]).unwrap().rows,
        vec![vec![i(1), i(2), i(3)]]
    );
    assert!(c
        .execute("ALTER TABLE t ADD bad CHECK(bad<0) DEFAULT 1", &[])
        .is_err());
    assert_eq!(
        c.execute("SELECT * FROM t", &[]).unwrap().columns,
        vec!["a", "b", "c"]
    );
    assert!(!c.is_autocommit());
    c.execute("ROLLBACK", &[]).unwrap();
    assert_eq!(c.to_image(512).unwrap(), image);
    for sql in [
        "ALTER TABLE t ADD b",
        "ALTER TABLE t ADD x DEFAULT ?",
        "ALTER TABLE t ADD x AS(?)",
        "ALTER TABLE t DROP a,b",
        "ALTER TABLE t DROP COLUMN",
        "ALTER TABLE missing ADD x",
        "ALTER TABLE t ADD CONSTRAINT positive CHECK(a<0)",
        "ALTER TABLE t ADD COLUMN CONSTRAINT dangling",
        "ALTER TABLE t ADD CHECK(a<0)",
        "ALTER TABLE t DROP CONSTRAINT absent",
        "ALTER TABLE t ADD x DEFAULT abs(-3)",
        "ALTER TABLE t ADD x DEFAULT ++1",
        "ALTER TABLE t ADD x DEFAULT -(1)",
        "ALTER TABLE t ADD x DEFAULT +true",
        "ALTER TABLE t ADD x DEFAULT CURRENT_TIMESTAMP",
    ] {
        assert!(c.execute(sql, &[]).is_err());
        assert_eq!(c.to_image(512).unwrap(), image);
    }
    for seed in [
        "ALTER TABLE t ADD x INT CHECK(x>0) DEFAULT 1",
        "ALTER TABLE t DROP COLUMN b",
    ] {
        for end in 1..seed.len() {
            let _ = c.prepare(&seed[..end]);
        }
    }
}

#[test]
fn alter_limits_do_not_publish_partial_schema_or_rows() {
    let mut c = Connection::with_limits(SqlLimits {
        max_database_bytes: 2500,
        ..SqlLimits::default()
    });
    c.execute_batch(
        "CREATE TABLE t(a);INSERT INTO t VALUES(1),(2),(3),(4),(5),(6),(7),(8),(9),(10);",
    )
    .unwrap();
    let before = c.to_image(512).unwrap();
    let sql = format!("ALTER TABLE t ADD b TEXT DEFAULT '{}'", "z".repeat(400));
    assert!(matches!(c.execute(&sql, &[]), Err(Error::Limit(_))));
    assert_eq!(c.to_image(512).unwrap(), before);
    let mut limited = Connection::with_limits(SqlLimits {
        max_sql_bytes: 100,
        ..SqlLimits::default()
    });
    limited.execute("CREATE TABLE t(a)", &[]).unwrap();
    let before = limited.to_image(512).unwrap();
    for _ in 0..2 {
        assert!(matches!(
            limited.execute(
                &format!("ALTER TABLE t ADD b DEFAULT '{}'", "a".repeat(90)),
                &[]
            ),
            Err(Error::Limit(_))
        ));
    }
    assert_eq!(limited.to_image(512).unwrap(), before);
    let sql = format!(
        "ALTER TABLE t ADD b DEFAULT {}1{}",
        "(".repeat(1000),
        ")".repeat(1000)
    );
    assert!(matches!(c.prepare(&sql), Err(Error::Limit(_))));
}

#[test]
fn alter_roundtrips_all_image_configurations() {
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
                c.execute_batch("CREATE TABLE t(k TEXT PRIMARY KEY,discarded INT,a INT,s INT AS(a*2) STORED,v INT AS(s+1)) STRICT, WITHOUT ROWID;INSERT INTO t(k,discarded,a) VALUES('é',4,5),('🦀',6,7);CREATE INDEX ix ON t((s+v));CREATE VIEW q AS SELECT k,s,v FROM t;ALTER TABLE t ADD b TEXT DEFAULT 42;ALTER TABLE t DROP discarded;").unwrap();
                let mut restored = Connection::from_image(&c.to_image(size).unwrap()).unwrap();
                assert_eq!(
                    restored
                        .execute("SELECT b,s,v FROM t ORDER BY a", &[])
                        .unwrap()
                        .rows,
                    vec![vec![t("42"), i(10), i(11)], vec![t("42"), i(14), i(15)]]
                );
                restored
                    .execute_batch("ALTER TABLE t ADD c INT AS(a+1);UPDATE t SET a=a+1;")
                    .unwrap();
                let mut again = Connection::from_image(&restored.to_image(size).unwrap()).unwrap();
                assert_eq!(
                    again
                        .execute("SELECT c FROM t ORDER BY a", &[])
                        .unwrap()
                        .rows,
                    vec![vec![i(7)], vec![i(9)]]
                );
            }
        }
    }
}
