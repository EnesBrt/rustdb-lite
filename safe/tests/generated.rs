#![forbid(unsafe_code)]
use sqlite_safe::{
    sql::{Connection, SqlLimits},
    AutoVacuum, Database, Encoding, Error, ImageBuilder, Text, Value,
};
fn i(n: i64) -> Value {
    Value::Integer(n)
}
fn text(s: &str) -> Value {
    Value::Text(Text::utf8(s))
}

#[test]
fn chained_values_bind_to_the_correct_rows_and_outer_scopes() {
    let mut db = Connection::new();
    db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY,v INT AS(s+1),x INT,s INT GENERATED ALWAYS AS(x*id) STORED)", &[]).unwrap();
    let insert = db
        .prepare("INSERT INTO t(x) VALUES(?1) RETURNING *")
        .unwrap();
    assert_eq!(
        db.execute_prepared(&insert, &[i(4)]).unwrap().rows,
        vec![vec![i(1), i(5), i(4), i(4)]]
    );
    assert_eq!(
        db.execute_prepared(&insert, &[i(6)]).unwrap().rows,
        vec![vec![i(2), i(13), i(6), i(12)]]
    );
    assert_eq!(db.execute("SELECT a.id,b.v,(SELECT a.v),(SELECT b.v) FROM t a LEFT JOIN t b ON b.id=a.id+1 ORDER BY a.id", &[]).unwrap().rows,
        vec![vec![i(1),i(13),i(5),i(13)],vec![i(2),Value::Null,i(13),Value::Null]]);
    assert_eq!(
        db.execute("UPDATE t SET id=id+10 RETURNING v,s", &[])
            .unwrap()
            .rows,
        vec![vec![i(45), i(44)], vec![i(73), i(72)]]
    );
    db.execute_batch("CREATE VIEW v AS SELECT * FROM t;CREATE TABLE copied AS SELECT * FROM v;")
        .unwrap();
    assert_eq!(
        db.execute("SELECT * FROM copied ORDER BY id", &[])
            .unwrap()
            .rows,
        db.execute("SELECT * FROM t ORDER BY id", &[]).unwrap().rows
    );
}

#[test]
fn defaults_replacement_unique_upsert_and_savepoints_recompute_dependencies() {
    let mut db = Connection::new();
    db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY,g INT AS(x*2) NOT NULL UNIQUE,x INT NOT NULL ON CONFLICT REPLACE DEFAULT 7,h INT AS(g+1) STORED)", &[]).unwrap();
    assert_eq!(
        db.execute("INSERT INTO t(x) VALUES(NULL) RETURNING *", &[])
            .unwrap()
            .rows,
        vec![vec![i(1), i(14), i(7), i(15)]]
    );
    assert_eq!(
        db.execute(
            "INSERT INTO t(x) VALUES(7) ON CONFLICT(g) DO UPDATE SET x=excluded.h RETURNING *",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(1), i(30), i(15), i(31)]]
    );
    db.execute("SAVEPOINT s", &[]).unwrap();
    assert!(matches!(
        db.execute("INSERT OR FAIL INTO t(x) VALUES(9),(15),(20)", &[]),
        Err(Error::Constraint(_))
    ));
    assert_eq!(db.changes(), 1);
    assert_eq!(
        db.execute("SELECT g FROM t ORDER BY id", &[]).unwrap().rows,
        vec![vec![i(30)], vec![i(18)]]
    );
    db.execute("ROLLBACK TO s", &[]).unwrap();
    db.execute("RELEASE s", &[]).unwrap();
    let restored = Connection::from_image(&db.to_image(512).unwrap()).unwrap();
    let mut restored = restored;
    assert!(restored
        .execute("INSERT INTO t(x) VALUES(15)", &[])
        .is_err());
    assert_eq!(
        restored.execute("SELECT g,h FROM t", &[]).unwrap().rows,
        vec![vec![i(30), i(31)]]
    );
}

#[test]
fn generated_storage_omits_virtual_slots_in_all_image_layouts() {
    for encoding in [Encoding::Utf8, Encoding::Utf16Le, Encoding::Utf16Be] {
        for page_size in [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536] {
            for mode in [AutoVacuum::None, AutoVacuum::Full, AutoVacuum::Incremental] {
                let empty = ImageBuilder::new(page_size)
                    .unwrap()
                    .encoding(encoding)
                    .unwrap()
                    .auto_vacuum(mode)
                    .finish()
                    .unwrap();
                let mut db = Connection::from_image(&empty).unwrap();
                for (name, suffix) in [("t", ""), ("w", " WITHOUT ROWID")] {
                    db.execute(&format!("CREATE TABLE {name}(id INTEGER PRIMARY KEY,v TEXT AS(upper(x)),x TEXT,s INT AS(length(v)) STORED){suffix}"), &[]).unwrap();
                    db.execute(&format!("CREATE INDEX ix_{name} ON {name}(v DESC,s)"), &[])
                        .unwrap();
                    db.execute(
                        &format!("INSERT INTO {name}(id,x) VALUES(3,'é'),(1,'Rust'),(2,'🦀')"),
                        &[],
                    )
                    .unwrap();
                }
                let image = db.to_image(page_size).unwrap();
                let parsed = Database::parse(&image).unwrap();
                for entry in parsed
                    .schema()
                    .unwrap()
                    .iter()
                    .filter(|e| e.kind == "table")
                {
                    parsed
                        .visit_rows(entry.root_page, |row| {
                            assert_eq!(row.values.len(), 3);
                            Ok(())
                        })
                        .unwrap();
                }
                let mut restored = Connection::from_image(&image).unwrap();
                for name in ["t", "w"] {
                    let query = format!("SELECT * FROM {name} ORDER BY id");
                    assert_eq!(
                        restored.execute(&query, &[]).unwrap().rows,
                        db.execute(&query, &[]).unwrap().rows
                    );
                    assert_eq!(
                        restored
                            .execute(
                                &format!("UPDATE {name} SET x='safe' WHERE id=1 RETURNING v,s"),
                                &[]
                            )
                            .unwrap()
                            .rows,
                        vec![vec![text("SAFE"), i(4)]]
                    );
                }
            }
        }
    }
}

#[test]
fn metadata_filters_generated_columns_and_preserves_declared_affinity() {
    let mut db = Connection::new();
    db.execute(
        "CREATE TABLE t(a INT,g TEXT AS(a+1),b,h INT AS(g+1) STORED)",
        &[],
    )
    .unwrap();
    let info = db.execute("PRAGMA table_info(t)", &[]).unwrap().rows;
    assert_eq!(
        info.iter().map(|r| r[0..2].to_vec()).collect::<Vec<_>>(),
        vec![vec![i(0), text("a")], vec![i(1), text("b")]]
    );
    let info = db.execute("PRAGMA table_xinfo(t)", &[]).unwrap().rows;
    assert_eq!(
        info.iter().map(|r| r[6].clone()).collect::<Vec<_>>(),
        vec![i(0), i(2), i(0), i(3)]
    );
    db.execute("INSERT INTO t VALUES(1,0)", &[]).unwrap();
    assert_eq!(
        db.execute("SELECT g,h,g=2,h='3',typeof(g),typeof(h) FROM t", &[])
            .unwrap()
            .rows,
        vec![vec![
            text("2"),
            i(3),
            i(1),
            i(1),
            text("text"),
            text("integer")
        ]]
    );
}

#[test]
fn invalid_definitions_writes_and_cycles_do_not_mutate_the_database() {
    let mut db = Connection::new();
    for sql in [
        "CREATE TABLE t(a AS(1))",
        "CREATE TABLE t(a,b AS(rowid))",
        "CREATE TABLE t(a,b AS(sum(a)))",
        "CREATE TABLE t(a,b AS(changes()))",
        "CREATE TABLE t(a,b AS(?1))",
        "CREATE TABLE t(a,b AS((SELECT 1)))",
        "CREATE TABLE t(a,b AS(a) DEFAULT 1)",
        "CREATE TABLE t(a,b AS(a),PRIMARY KEY(b))",
        "CREATE TABLE t(a,b AS(b+1))",
        "CREATE TABLE t(a,b AS(c),c AS(b))",
    ] {
        assert!(db.execute(sql, &[]).is_err(), "{sql}");
    }
    db.execute("CREATE TABLE t(a,b AS(c+1),c AS(b+1) STORED)", &[])
        .unwrap();
    assert!(db.execute("SELECT a FROM t", &[]).unwrap().rows.is_empty());
    assert!(db.execute("SELECT b FROM t", &[]).unwrap().rows.is_empty());
    assert!(db.execute("INSERT INTO t VALUES(1)", &[]).is_err());
    db.execute("DROP TABLE t", &[]).unwrap();
    db.execute("CREATE TABLE t(a,b AS(b+1) STORED)", &[])
        .unwrap();
    assert!(db.execute("SELECT b FROM t", &[]).unwrap().rows.is_empty());
    assert!(db.execute("INSERT INTO t VALUES(1)", &[]).is_err());
    db.execute("DELETE FROM t RETURNING a", &[]).unwrap();
    db.execute("DROP TABLE t", &[]).unwrap();
    let sql = "CREATE TABLE t(a INT DEFAULT 4,b INT AS(a+1) STORED,c INT AS(b+1))";
    for end in 1..sql.len() {
        assert!(db.execute(&sql[..end], &[]).is_err(), "{}", &sql[..end]);
    }
    db.execute(sql, &[]).unwrap();
    assert_eq!(
        db.execute("INSERT INTO t DEFAULT VALUES RETURNING *", &[])
            .unwrap()
            .rows,
        vec![vec![i(4), i(5), i(6)]]
    );
    let before = db.to_image(512).unwrap();
    for sql in [
        "INSERT INTO t(b) VALUES(2)",
        "INSERT INTO t VALUES(1,2,3)",
        "UPDATE t SET b=2",
        "INSERT INTO t VALUES(4) ON CONFLICT DO UPDATE SET b=2",
    ] {
        assert!(db.execute(sql, &[]).is_err(), "{sql}");
        assert_eq!(db.to_image(512).unwrap(), before);
    }
}

#[test]
fn strict_generated_datatype_errors_retain_only_the_native_transaction_prefix() {
    for storage in ["VIRTUAL", "STORED"] {
        let mut db = Connection::new();
        db.execute(&format!("CREATE TABLE t(x INT,g INT AS(CASE x WHEN 9 THEN 'bad' ELSE x END) {storage}) STRICT"),&[]).unwrap();
        db.execute("BEGIN", &[]).unwrap();
        assert!(matches!(
            db.execute("INSERT INTO t VALUES(1),(9)", &[]),
            Err(Error::Datatype(_))
        ));
        assert_eq!((db.changes(), db.total_changes()), (0, 0));
        assert_eq!(
            db.execute("SELECT * FROM t", &[]).unwrap().rows,
            vec![vec![i(1), i(1)]]
        );
        db.execute("COMMIT", &[]).unwrap();
        let mut restored = Connection::from_image(&db.to_image(512).unwrap()).unwrap();
        assert_eq!(
            restored.execute("SELECT g FROM t", &[]).unwrap().rows,
            vec![vec![i(1)]]
        );
    }
}

#[test]
fn dependency_reuse_and_large_generated_values_obey_budgets() {
    let mut db = Connection::with_limits(SqlLimits {
        max_value_bytes: 64,
        max_steps: 300,
        ..SqlLimits::default()
    });
    db.execute(
        "CREATE TABLE t(a TEXT,b TEXT AS(a||a) STORED,c TEXT AS(b||b))",
        &[],
    )
    .unwrap();
    assert!(matches!(
        db.execute("INSERT INTO t VALUES(?1)", &[text(&"a".repeat(20))]),
        Err(Error::Limit(_))
    ));
    assert!(db.execute("SELECT * FROM t", &[]).unwrap().rows.is_empty());
    let columns = (0..16)
        .map(|i| {
            if i == 0 {
                "a0 AS(x+x)".into()
            } else {
                format!("a{i} AS(a{}+a{})", i - 1, i - 1)
            }
        })
        .collect::<Vec<String>>()
        .join(",");
    let mut db = Connection::with_limits(SqlLimits {
        max_steps: 5000,
        ..SqlLimits::default()
    });
    db.execute(&format!("CREATE TABLE chain(x,{columns})"), &[])
        .unwrap();
    db.execute("INSERT INTO chain VALUES(1)", &[]).unwrap();
    assert!(matches!(
        db.execute("SELECT a15 FROM chain", &[]),
        Err(Error::Limit(_))
    ));
    assert_eq!(
        db.execute("SELECT x FROM chain", &[]).unwrap().rows,
        vec![vec![i(1)]]
    );
}
