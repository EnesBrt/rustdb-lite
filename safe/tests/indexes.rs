#![forbid(unsafe_code)]
use sqlite_safe::{sql::Connection, Database, Text, Value};

#[test]
fn composite_unique_collations_nulls_and_statement_rollback() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a TEXT,b,n);INSERT INTO t VALUES('a',1,0),('A',2,0),(NULL,1,0),(NULL,1,0);CREATE UNIQUE INDEX u ON t(a COLLATE NOCASE DESC,b);").unwrap();
    assert!(c.execute("INSERT INTO t VALUES('A',1,0)", &[]).is_err());
    assert!(c.execute("UPDATE t SET b=1", &[]).is_err());
    assert_eq!(
        c.execute("SELECT b FROM t WHERE a='A'", &[]).unwrap().rows,
        vec![vec![Value::Integer(2)]]
    );
    assert!(c.execute("CREATE UNIQUE INDEX bad ON t(b)", &[]).is_err());
    c.execute("CREATE INDEX bad ON t(b)", &[]).unwrap();
    c.execute_batch("BEGIN;DROP INDEX u;INSERT INTO t VALUES('A',1,0);ROLLBACK;")
        .unwrap();
    assert!(c.execute("INSERT INTO t VALUES('A',1,0)", &[]).is_err());
    c.execute_batch("DROP INDEX u;INSERT INTO t VALUES('A',1,0);DROP TABLE t;CREATE INDEX IF NOT EXISTS bad ON missing(a);").unwrap_err();
    c.execute("CREATE TABLE bad(x)", &[]).unwrap();
}

#[test]
fn automatic_indexes_and_namespace_are_preserved() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE main.t(a TEXT PRIMARY KEY DESC UNIQUE,b UNIQUE,c INTEGER UNIQUE);INSERT INTO t VALUES('c',1,2),('a',2,3),(NULL,NULL,NULL),(NULL,NULL,NULL);").unwrap();
    assert!(c.execute("DROP INDEX sqlite_autoindex_t_1", &[]).is_err());
    assert!(c.execute("CREATE INDEX t ON t(a)", &[]).is_err());
    c.execute("CREATE INDEX main.idx ON t(a)", &[]).unwrap();
    assert!(c.execute("CREATE TABLE IF NOT EXISTS idx(x)", &[]).is_err());
    c.execute("CREATE INDEX IF NOT EXISTS idx ON missing(nope)", &[])
        .unwrap();
    for page_size in [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536] {
        let image = c.to_image(page_size).unwrap();
        let db = Database::parse(&image).unwrap();
        assert_eq!(
            db.schema()
                .unwrap()
                .iter()
                .filter(|e| e.kind == "index")
                .count(),
            4
        );
        let mut loaded = Connection::from_image(&image).unwrap();
        assert!(loaded
            .execute("INSERT INTO t VALUES('c',7,8)", &[])
            .is_err());
        assert_eq!(
            loaded
                .execute("SELECT * FROM t ORDER BY rowid", &[])
                .unwrap()
                .rows,
            c.execute("SELECT * FROM t ORDER BY rowid", &[])
                .unwrap()
                .rows
        );
    }
}

#[test]
fn index_balancing_and_overflow_keep_all_records_once() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(k,v);CREATE INDEX idx ON t(k DESC,v COLLATE NOCASE);")
        .unwrap();
    let insert = c.prepare("INSERT INTO t VALUES(?,?)").unwrap();
    for count in 0..240 {
        if count > 0 {
            let text = if count % 7 == 0 {
                "🦀".repeat(900)
            } else {
                format!("key-{count}")
            };
            c.execute_prepared(
                &insert,
                &[Value::Integer(count % 13), Value::Text(Text::utf8(&text))],
            )
            .unwrap();
        }
        let image = c.to_image(512).unwrap();
        let db = Database::parse(&image).unwrap();
        let entry = db
            .schema()
            .unwrap()
            .into_iter()
            .find(|e| e.name == "idx")
            .unwrap();
        let entries = db.rows(entry.root_page).unwrap();
        assert_eq!(entries.len(), count as usize);
        let expected = c
            .execute(
                "SELECT k,v,rowid FROM t ORDER BY k DESC,v COLLATE NOCASE,rowid",
                &[],
            )
            .unwrap()
            .rows;
        assert_eq!(
            entries
                .into_iter()
                .map(|r| {
                    assert!(r.rowid.is_none());
                    r.values
                })
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn index_exports_share_the_database_allocation_budget() {
    let mut c = Connection::with_limits(sqlite_safe::sql::SqlLimits {
        max_database_bytes: 8192,
        ..Default::default()
    });
    c.execute("CREATE TABLE t(x)", &[]).unwrap();
    c.execute(
        "INSERT INTO t VALUES(?)",
        &[Value::Text(Text::utf8(&"x".repeat(3000)))],
    )
    .unwrap();
    c.execute_batch("CREATE INDEX a ON t(x); CREATE INDEX b ON t(x);")
        .unwrap();
    assert!(matches!(c.to_image(512), Err(sqlite_safe::Error::Limit(_))));
    c.execute("DROP INDEX b", &[]).unwrap();
    assert!(c.to_image(512).is_ok());
}
