#![forbid(unsafe_code)]
use sqlite_safe::{
    sql::{Connection, SqlLimits},
    AutoVacuum, Database, Encoding, Error, ImageBuilder, Text, Value,
};
fn i(n: i64) -> Value {
    Value::Integer(n)
}
fn t(s: &str) -> Value {
    Value::Text(Text::utf8(s))
}
fn schema(c: &Connection, name: &str) -> String {
    let image = c.to_image(512).unwrap();
    Database::parse(&image)
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
fn set_not_null_replaces_first_constraint_and_preserves_counters() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT,a INT DEFAULT 9 CONSTRAINT old NOT NULL ON CONFLICT IGNORE);INSERT INTO t(a) VALUES(3);").unwrap();
    let insert = c.prepare("INSERT INTO t(a) VALUES(?)").unwrap();
    c.execute(
        "ALTER TABLE main.t ALTER COLUMN a SET NOT NULL ON CONFLICT REPLACE",
        &[],
    )
    .unwrap();
    assert_eq!(
        (c.changes(), c.total_changes(), c.last_insert_rowid()),
        (1, 1, 1)
    );
    c.execute_prepared(&insert, &[Value::Null]).unwrap();
    assert_eq!(
        c.execute("SELECT a FROM t ORDER BY id", &[]).unwrap().rows,
        vec![vec![i(3)], vec![i(9)]]
    );
    assert!(c.execute("ALTER TABLE t DROP CONSTRAINT old", &[]).is_err());
    c.execute("ALTER TABLE t ALTER a DROP NOT NULL", &[])
        .unwrap();
    c.execute_prepared(&insert, &[Value::Null]).unwrap();
    assert_eq!(
        c.execute("SELECT a FROM t WHERE id=3", &[]).unwrap().rows,
        vec![vec![Value::Null]]
    );
    let image = c.to_image(512).unwrap();
    assert!(c
        .execute("ALTER TABLE t ALTER a SET NOT NULL ON CONFLICT IGNORE", &[])
        .is_err());
    assert_eq!(c.to_image(512).unwrap(), image);
    c.execute("ALTER TABLE t ALTER a DROP NOT NULL", &[])
        .unwrap();
    assert_eq!(c.to_image(512).unwrap(), image);
}

#[test]
fn add_check_requires_true_but_future_checks_accept_null() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a);INSERT INTO t VALUES(NULL);")
        .unwrap();
    let image = c.to_image(512).unwrap();
    assert!(c
        .execute("ALTER TABLE t ADD CONSTRAINT positive CHECK(a>0)", &[])
        .is_err());
    assert_eq!(c.to_image(512).unwrap(), image);
    c.execute("DELETE FROM t", &[]).unwrap();
    c.execute("ALTER TABLE t ADD CONSTRAINT positive CHECK(a>0)", &[])
        .unwrap();
    c.execute("INSERT INTO t VALUES(NULL),(1)", &[]).unwrap();
    assert!(c.execute("INSERT INTO t VALUES(-1)", &[]).is_err());
    assert!(c
        .execute("ALTER TABLE t ADD CONSTRAINT POSITIVE CHECK(a<10)", &[])
        .is_err());
    c.execute("ALTER TABLE t DROP CONSTRAINT Positive", &[])
        .unwrap();
    c.execute("INSERT INTO t VALUES(-1)", &[]).unwrap();
    assert_eq!(
        c.execute("SELECT count(*) FROM t", &[]).unwrap().rows,
        vec![vec![i(3)]]
    );
    // The reference's validation WHERE binds TRUE as an identifier if present.
    c.execute_batch("CREATE TABLE shadow(a,\"true\");INSERT INTO shadow VALUES(2,2);ALTER TABLE shadow ADD CHECK(a);").unwrap();
    // Constant validation expressions can raise before an empty-table scan.
    c.execute("CREATE TABLE empty(a)", &[]).unwrap();
    assert!(c
        .execute(
            "ALTER TABLE empty ADD CHECK(abs(-9223372036854775808)>0)",
            &[]
        )
        .is_err());
    c.execute("ALTER TABLE empty ADD CHECK(0)", &[]).unwrap();
    assert!(c.execute("INSERT INTO empty VALUES(1)", &[]).is_err());
}

#[test]
fn check_branches_use_null_policy_without_changing_scalar_evaluation() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a);ALTER TABLE t ADD CHECK(a>0 OR abs(-9223372036854775808)>0);INSERT INTO t VALUES(1),(NULL);").unwrap();
    assert_eq!(
        c.execute("SELECT * FROM t", &[]).unwrap().rows,
        vec![vec![i(1)], vec![Value::Null]]
    );
    let image = c.to_image(512).unwrap();
    assert!(c.execute("INSERT INTO t VALUES(0)", &[]).is_err());
    assert_eq!(c.to_image(512).unwrap(), image);
    assert!(c
        .execute(
            "ALTER TABLE t ADD CHECK(a>0 OR abs(-9223372036854775808)>0)",
            &[]
        )
        .is_err());
    c.execute_batch("CREATE TABLE lazy(a CHECK(abs(-9223372036854775808) OR 1));INSERT INTO lazy VALUES(1);ALTER TABLE lazy ADD CHECK(NOT (0 AND abs(-9223372036854775808)));INSERT INTO lazy VALUES(2);").unwrap();
    assert!(c
        .execute("SELECT 1 OR abs(-9223372036854775808)", &[])
        .is_err());
    assert_eq!(
        c.execute("SELECT count(*) FROM lazy", &[]).unwrap().rows,
        vec![vec![i(2)]]
    );
}

#[test]
fn named_edits_preserve_comments_labels_and_unrelated_constraints() {
    let mut c = Connection::new();
    c.execute("CREATE TABLE t(a INT /* before */ CONSTRAINT nn NOT /* middle */ NULL /* after */, b, CONSTRAINT ck CHECK(a IN (1,2)))",&[]).unwrap();
    c.execute("ALTER TABLE t ALTER a DROP NOT NULL", &[])
        .unwrap();
    assert_eq!(
        schema(&c, "t"),
        "CREATE TABLE t(a INT, b, CONSTRAINT ck CHECK(a IN (1,2)))"
    );
    c.execute(
        "ALTER TABLE t ADD CONSTRAINT [é 🦀] CHECK(a>0) /* keep */ -- discard\n",
        &[],
    )
    .unwrap();
    assert!(schema(&c, "t").ends_with(", CONSTRAINT [é 🦀] CHECK(a>0) /* keep */)"));
    c.execute("ALTER TABLE t DROP CONSTRAINT ck", &[]).unwrap();
    c.execute("ALTER TABLE t DROP CONSTRAINT [é 🦀]", &[])
        .unwrap();
    assert_eq!(schema(&c, "t"), "CREATE TABLE t(a INT, b)");
    c.execute("CREATE TABLE labels(a CONSTRAINT one CONSTRAINT two NOT NULL DEFAULT 4, b CONSTRAINT uq UNIQUE)",&[]).unwrap();
    c.execute("ALTER TABLE labels DROP CONSTRAINT one", &[])
        .unwrap();
    assert!(schema(&c, "labels").contains("CONSTRAINT two NOT NULL"));
    c.execute("ALTER TABLE labels DROP CONSTRAINT two", &[])
        .unwrap();
    assert!(schema(&c, "labels").contains("DEFAULT 4"));
    let image = c.to_image(512).unwrap();
    assert!(c
        .execute("ALTER TABLE labels DROP CONSTRAINT uq", &[])
        .is_err());
    assert_eq!(c.to_image(512).unwrap(), image);
}

#[test]
fn constraint_edits_rollback_and_keep_generated_and_primary_values() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(id INT PRIMARY KEY,a INT,s INT AS(a*2) STORED,g INT AS(s+1)) STRICT, WITHOUT ROWID;INSERT INTO t(id,a) VALUES(1,3);CREATE INDEX ix ON t((s+g));CREATE VIEW broken AS SELECT * FROM absent;").unwrap();
    let before = c.to_image(512).unwrap();
    c.execute_batch("BEGIN;ALTER TABLE t ALTER id DROP NOT NULL;ALTER TABLE t ALTER g SET NOT NULL;ALTER TABLE t ADD CONSTRAINT c CHECK(g>0);SAVEPOINT s;ALTER TABLE t DROP CONSTRAINT c;UPDATE t SET a=-a;ROLLBACK TO s;").unwrap();
    assert!(c
        .execute("ALTER TABLE t ADD CHECK(g<0) ON CONFLICT ROLLBACK", &[])
        .is_err());
    assert!(!c.is_autocommit());
    assert_eq!(
        c.execute("SELECT s,g FROM t", &[]).unwrap().rows,
        vec![vec![i(6), i(7)]]
    );
    c.execute("ROLLBACK", &[]).unwrap();
    assert_eq!(c.to_image(512).unwrap(), before);
    c.execute(
        "ALTER TABLE t ALTER a SET NOT NULL ON CONFLICT ROLLBACK",
        &[],
    )
    .unwrap();
    c.execute_batch("BEGIN;ALTER TABLE t ADD CONSTRAINT c CHECK(g>0);")
        .unwrap();
    assert!(c
        .execute("INSERT INTO t(id,a) VALUES(2,NULL)", &[])
        .is_err());
    assert!(c.is_autocommit());
    assert!(c.execute("ALTER TABLE t DROP CONSTRAINT c", &[]).is_err());
}

#[test]
fn constraint_limits_and_malformed_edits_preserve_state() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a);INSERT INTO t VALUES(1);")
        .unwrap();
    let image = c.to_image(512).unwrap();
    for sql in [
        "ALTER TABLE t ADD CHECK(?)",
        "ALTER TABLE t ADD CHECK((SELECT 1))",
        "ALTER TABLE t ADD CHECK(sum(a))",
        "ALTER TABLE t ALTER missing SET NOT NULL",
        "ALTER TABLE t ALTER rowid DROP NOT NULL",
        "ALTER TABLE t ALTER a SET NOT NULL ON CONFLICT unknown",
        "ALTER TABLE t ALTER a DROP NOT NULL ON CONFLICT IGNORE",
        "ALTER TABLE t ADD COLUMN CHECK(1)",
        "ALTER TABLE t ADD CONSTRAINT c UNIQUE(a)",
        "ALTER TABLE t DROP CONSTRAINT missing",
    ] {
        assert!(c.execute(sql, &[]).is_err(), "{sql}");
        assert_eq!(c.to_image(512).unwrap(), image);
    }
    for seed in [
        "ALTER TABLE t ALTER COLUMN a SET NOT NULL ON CONFLICT REPLACE",
        "ALTER TABLE t ADD CONSTRAINT c CHECK(a>0)",
    ] {
        for end in 1..seed.len() {
            let _ = c.prepare(&seed[..end]);
        }
    }
    let deep = format!(
        "ALTER TABLE t ADD CHECK({}a>0{})",
        "(".repeat(1000),
        ")".repeat(1000)
    );
    assert!(matches!(c.execute(&deep, &[]), Err(Error::Limit(_))));
    let mut limited = Connection::with_limits(SqlLimits {
        max_steps: 150,
        ..SqlLimits::default()
    });
    limited.execute("CREATE TABLE t(a)", &[]).unwrap();
    for _ in 0..100 {
        limited.execute("INSERT INTO t VALUES(1)", &[]).unwrap();
    }
    let before = limited.to_image(512).unwrap();
    assert!(matches!(
        limited.execute("ALTER TABLE t ALTER a SET NOT NULL", &[]),
        Err(Error::Limit(_))
    ));
    assert_eq!(limited.to_image(512).unwrap(), before);
}

#[test]
fn constraint_schema_roundtrips_all_image_configurations() {
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
                c.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,a TEXT,s INT AS(length(a)) STORED);CREATE TABLE w(id INT PRIMARY KEY,a TEXT,s INT AS(length(a)) STORED) STRICT,WITHOUT ROWID;INSERT INTO t(id,a) VALUES(1,'é🦀');INSERT INTO w(id,a) VALUES(1,'é🦀');").unwrap();
                for table in ["t", "w"] {
                    c.execute(&format!("ALTER TABLE {table} ALTER a SET NOT NULL"), &[])
                        .unwrap();
                    c.execute(
                        &format!("ALTER TABLE {table} ADD CONSTRAINT nonempty CHECK(s>0)"),
                        &[],
                    )
                    .unwrap();
                }
                let mut restored = Connection::from_image(&c.to_image(size).unwrap()).unwrap();
                for table in ["t", "w"] {
                    assert!(restored
                        .execute(&format!("UPDATE {table} SET a=NULL"), &[])
                        .is_err());
                    assert!(restored
                        .execute(&format!("UPDATE {table} SET a=''"), &[])
                        .is_err());
                    assert_eq!(
                        restored
                            .execute(&format!("SELECT a,s FROM {table}"), &[])
                            .unwrap()
                            .rows,
                        vec![vec![t("é🦀"), i(2)]]
                    );
                    restored
                        .execute(&format!("ALTER TABLE {table} ALTER a DROP NOT NULL"), &[])
                        .unwrap();
                    restored
                        .execute(
                            &format!("ALTER TABLE {table} DROP CONSTRAINT nonempty"),
                            &[],
                        )
                        .unwrap();
                    restored
                        .execute(&format!("UPDATE {table} SET a=NULL"), &[])
                        .unwrap();
                }
                Connection::from_image(&restored.to_image(size).unwrap()).unwrap();
            }
        }
    }
}
