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
fn rows(db: &mut Connection) -> Vec<Vec<Value>> {
    db.execute("SELECT * FROM t ORDER BY id", &[]).unwrap().rows
}

#[test]
fn primary_keys_use_defaults_and_keep_last_insert_rowid_unchanged() {
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE ordinary(v);INSERT INTO ordinary(rowid,v) VALUES(42,1);CREATE TABLE t(id INTEGER PRIMARY KEY DEFAULT 5,v TEXT) WITHOUT ROWID;").unwrap();
    db.execute("INSERT INTO t DEFAULT VALUES", &[]).unwrap();
    let p = db
        .prepare("INSERT INTO t VALUES(?1,?2) RETURNING id,v,last_insert_rowid()")
        .unwrap();
    assert_eq!(
        db.execute_prepared(&p, &[i(3), text("value")])
            .unwrap()
            .rows,
        vec![vec![i(3), text("value"), i(42)]]
    );
    assert_eq!(
        rows(&mut db),
        vec![vec![i(3), text("value")], vec![i(5), Value::Null]]
    );
    assert_eq!(
        (db.changes(), db.total_changes(), db.last_insert_rowid()),
        (1, 3, 42)
    );
    assert!(matches!(
        db.execute_prepared(&p, &[Value::Null, Value::Null]),
        Err(Error::Constraint(_))
    ));
    assert_eq!(
        (db.changes(), db.total_changes(), db.last_insert_rowid()),
        (0, 3, 42)
    );
}

#[test]
fn hidden_rowids_are_absent_but_real_columns_can_use_their_names() {
    let mut db = Connection::new();
    db.execute(
        "CREATE TABLE t(id INT PRIMARY KEY) STRICT,WITHOUT ROWID",
        &[],
    )
    .unwrap();
    for sql in [
        "SELECT rowid FROM t",
        "SELECT oid FROM t",
        "INSERT INTO t(_rowid_) VALUES(1)",
        "UPDATE t SET rowid=1",
        "INSERT INTO t VALUES(1) RETURNING rowid",
    ] {
        assert!(db.execute(sql, &[]).is_err(), "{sql}");
    }
    assert!(rows(&mut db).is_empty());
    let flags = db.execute("PRAGMA table_list(t)", &[]).unwrap().rows;
    assert_eq!(flags[0][4..], [i(1), i(1)]);
    db.execute_batch("CREATE TABLE names(rowid PRIMARY KEY,oid,_rowid_) WITHOUT ROWID;INSERT INTO names VALUES('key',2,3);").unwrap();
    assert_eq!(
        db.execute("SELECT rowid,oid,_rowid_ FROM names", &[])
            .unwrap()
            .rows,
        vec![vec![text("key"), i(2), i(3)]]
    );
}

#[test]
fn primary_and_secondary_index_metadata_describe_physical_suffixes() {
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE t(a INT,b TEXT COLLATE NOCASE,c BLOB,PRIMARY KEY(a DESC,b),UNIQUE(c)) WITHOUT ROWID;CREATE INDEX ix ON t(c);").unwrap();
    let primary = db.execute("PRAGMA index_xinfo(t)", &[]).unwrap().rows;
    assert_eq!(
        primary,
        vec![
            vec![i(0), i(0), text("a"), i(1), text("BINARY"), i(1)],
            vec![i(1), i(1), text("b"), i(0), text("NOCASE"), i(1)],
            vec![i(2), i(2), text("c"), i(0), text("BINARY"), i(0)]
        ]
    );
    let automatic = db
        .execute("PRAGMA index_xinfo(sqlite_autoindex_t_2)", &[])
        .unwrap()
        .rows;
    let explicit = db.execute("PRAGMA index_xinfo(ix)", &[]).unwrap().rows;
    assert_eq!(automatic[1][3], i(0));
    assert_eq!(explicit[1][3], i(1));
    assert_eq!(explicit[1][5], i(0));
    let image = db.to_image(512).unwrap();
    let schema = Database::parse(&image).unwrap().schema().unwrap();
    assert!(!schema.iter().any(|s| s.name == "sqlite_autoindex_t_1"));
    assert!(schema.iter().any(|s| s.name == "sqlite_autoindex_t_2"));
}

#[test]
fn updates_revisit_captured_primary_keys_after_replacement() {
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE t(id INT PRIMARY KEY,v TEXT) WITHOUT ROWID;INSERT INTO t VALUES(1,'a'),(2,'b'),(3,'c');").unwrap();
    db.execute("SAVEPOINT before_update", &[]).unwrap();
    let r = db
        .execute("UPDATE OR REPLACE t SET id=id+1 RETURNING *", &[])
        .unwrap();
    assert_eq!(
        r.rows,
        vec![
            vec![i(2), text("a")],
            vec![i(3), text("a")],
            vec![i(4), text("a")]
        ]
    );
    assert_eq!(r.changes, 3);
    assert_eq!(rows(&mut db), vec![vec![i(4), text("a")]]);
    assert_eq!(db.last_insert_rowid(), 0);
    db.execute("ROLLBACK TO before_update", &[]).unwrap();
    assert_eq!(rows(&mut db).len(), 3);
    db.execute("RELEASE before_update", &[]).unwrap();
    assert_eq!(db.total_changes(), 6);
}

#[test]
fn upsert_binds_excluded_columns_without_a_hidden_slot() {
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE t(a TEXT COLLATE NOCASE,b INT,y ANY,PRIMARY KEY(a,b)) WITHOUT ROWID,STRICT;INSERT INTO t VALUES('A',1,'001'),('B',1,2);").unwrap();
    let r=db.execute("INSERT INTO t VALUES('a',1,'new') ON CONFLICT(a,b) DO UPDATE SET b=2,y=excluded.y RETURNING a,b,y",&[]).unwrap();
    assert_eq!(r.rows, vec![vec![text("A"), i(2), text("new")]]);
    let before = db.to_image(512).unwrap();
    db.execute("BEGIN", &[]).unwrap();
    assert!(matches!(
        db.execute(
            "INSERT INTO t VALUES('C',1,3),('B',1,4) ON CONFLICT(a,b) DO UPDATE SET b=NULL",
            &[]
        ),
        Err(Error::Constraint(_))
    ));
    assert_eq!(db.to_image(512).unwrap(), before);
    db.execute("ROLLBACK", &[]).unwrap();
    assert_eq!(db.last_insert_rowid(), 0);
}

#[test]
fn primary_storage_roundtrips_at_every_page_size_encoding_and_vacuum_mode() {
    for encoding in [Encoding::Utf8, Encoding::Utf16Le, Encoding::Utf16Be] {
        for mode in [AutoVacuum::None, AutoVacuum::Full, AutoVacuum::Incremental] {
            for size in [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536] {
                let image = ImageBuilder::new(size)
                    .unwrap()
                    .encoding(encoding)
                    .unwrap()
                    .auto_vacuum(mode)
                    .finish()
                    .unwrap();
                let mut db = Connection::from_image(&image).unwrap();
                db.execute_batch("CREATE TABLE t(id INT,a TEXT,y BLOB,PRIMARY KEY(a DESC,id),UNIQUE(id)) WITHOUT ROWID,STRICT;CREATE INDEX ix ON t(id DESC,a COLLATE NOCASE);").unwrap();
                let p = db.prepare("INSERT INTO t VALUES(?1,?2,?3)").unwrap();
                for id in 0..30 {
                    db.execute_prepared(
                        &p,
                        &[
                            i(id),
                            text(&format!("é-{id:03}-{}", "x".repeat(700))),
                            Value::Blob(vec![id as u8; 800]),
                        ],
                    )
                    .unwrap();
                }
                let expected = rows(&mut db);
                let image = db.to_image(size).unwrap();
                let physical = Database::parse(&image).unwrap();
                let root = physical
                    .schema()
                    .unwrap()
                    .into_iter()
                    .find(|s| s.name == "t")
                    .unwrap()
                    .root_page;
                assert!(physical
                    .rows(root)
                    .unwrap()
                    .iter()
                    .all(|r| r.rowid.is_none()));
                let mut copy = Connection::from_image(&image).unwrap();
                assert_eq!(rows(&mut copy), expected);
                copy.execute("UPDATE t SET id=id+100 WHERE id<3", &[])
                    .unwrap();
                assert_eq!(copy.last_insert_rowid(), 0);
                assert_eq!(rows(&mut copy).len(), 30);
            }
        }
    }
}

#[test]
fn malformed_declarations_and_execution_limits_preserve_owned_state() {
    for sql in [
        "CREATE TABLE t(x) WITHOUT ROWID",
        "CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT) WITHOUT ROWID",
        "CREATE TABLE t(x PRIMARY KEY) WITHOUT",
        "CREATE TABLE t(x PRIMARY KEY) WITHOUT ROWID,",
        "CREATE TABLE t(x PRIMARY KEY) WITHOUT ROWID STRICT",
    ] {
        let mut db = Connection::new();
        assert!(db.execute(sql, &[]).is_err());
        assert!(db.execute("SELECT * FROM t", &[]).is_err());
        assert!(db.execute("SELECT * FROM sqlite_sequence", &[]).is_err());
        for end in 0..=sql.len() {
            assert!(std::panic::catch_unwind(|| Connection::new().prepare(&sql[..end])).is_ok());
        }
    }
    let mut db = Connection::with_limits(SqlLimits {
        max_steps: 256,
        ..Default::default()
    });
    db.execute(
        "CREATE TABLE t(id INT PRIMARY KEY,v INT) WITHOUT ROWID",
        &[],
    )
    .unwrap();
    for n in 0..20 {
        db.execute("INSERT INTO t VALUES(?1,?1)", &[i(n)]).unwrap();
    }
    let before = db.to_image(512).unwrap();
    db.execute("BEGIN", &[]).unwrap();
    assert!(matches!(
        db.execute("UPDATE t SET id=id+100", &[]),
        Err(Error::Limit(_))
    ));
    assert_eq!(db.to_image(512).unwrap(), before);
    assert!(!db.is_autocommit());
    db.execute("ROLLBACK", &[]).unwrap();
}
