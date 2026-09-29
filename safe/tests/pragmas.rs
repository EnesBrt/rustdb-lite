#![forbid(unsafe_code)]
use sqlite_safe::{
    sql::{Connection, SqlLimits},
    Error, Text, Value,
};

fn text(value: &str) -> Value {
    Value::Text(Text::utf8(value))
}
fn integer(value: i64) -> Value {
    Value::Integer(value)
}

#[test]
fn schema_metadata_survives_roundtrip_and_transactional_ddl() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a text COLLATE nocase CONSTRAINT u UNIQUE,b int DEFAULT ( 1 + 2 ),PRIMARY KEY(a DESC),CHECK(b>0));CREATE INDEX idx ON t(b DESC,a COLLATE rtrim);").unwrap();
    let columns = c.execute("PRAGMA main.table_info='T'", &[]).unwrap();
    assert_eq!(
        columns.columns,
        vec!["cid", "name", "type", "notnull", "dflt_value", "pk"]
    );
    assert_eq!(
        columns.rows,
        vec![
            vec![
                integer(0),
                text("a"),
                text("TEXT"),
                integer(0),
                Value::Null,
                integer(1)
            ],
            vec![
                integer(1),
                text("b"),
                text("INT"),
                integer(0),
                text("1 + 2"),
                integer(0)
            ],
        ]
    );
    let indexes = c.execute("PRAGMA index_list(t)", &[]).unwrap();
    assert_eq!(
        indexes.rows,
        vec![
            vec![integer(0), text("idx"), integer(0), text("c"), integer(0)],
            vec![
                integer(1),
                text("sqlite_autoindex_t_1"),
                integer(1),
                text("pk"),
                integer(0)
            ],
        ]
    );
    assert_eq!(
        c.execute("PRAGMA index_xinfo(idx)", &[]).unwrap().rows,
        vec![
            vec![
                integer(0),
                integer(1),
                text("b"),
                integer(1),
                text("BINARY"),
                integer(1)
            ],
            vec![
                integer(1),
                integer(0),
                text("a"),
                integer(0),
                text("rtrim"),
                integer(1)
            ],
            vec![
                integer(2),
                integer(-1),
                Value::Null,
                integer(0),
                text("BINARY"),
                integer(0)
            ],
        ]
    );
    assert_eq!(
        c.execute("PRAGMA index_info(idx)", &[]).unwrap().rows.len(),
        2
    );
    assert_eq!(
        c.execute("PRAGMA table_xinfo(sqlite_schema)", &[])
            .unwrap()
            .rows
            .len(),
        5
    );
    assert_eq!(
        c.execute("PRAGMA table_info(missing)", &[])
            .unwrap()
            .columns,
        columns.columns
    );
    assert!(c.execute("PRAGMA index_info", &[]).unwrap().rows.is_empty());
    assert!(c.execute("PRAGMA table_info(1_2)", &[]).is_err());
    c.execute_batch("BEGIN;DROP INDEX idx;").unwrap();
    assert_eq!(
        c.execute("PRAGMA index_list(t)", &[]).unwrap().rows.len(),
        1
    );
    c.execute("ROLLBACK", &[]).unwrap();
    assert_eq!(c.execute("PRAGMA index_list(t)", &[]).unwrap(), indexes);
    let mut imported = Connection::from_image(&c.to_image(512).unwrap()).unwrap();
    assert_eq!(
        imported.execute("PRAGMA table_info(t)", &[]).unwrap(),
        columns
    );
    assert_eq!(
        imported.execute("PRAGMA index_list(t)", &[]).unwrap(),
        indexes
    );
}

#[test]
fn metadata_queries_enforce_limits_without_changing_schema() {
    for limits in [
        SqlLimits {
            max_rows: 1,
            ..Default::default()
        },
        SqlLimits {
            max_value_bytes: 3,
            ..Default::default()
        },
        SqlLimits {
            max_steps: 1,
            ..Default::default()
        },
    ] {
        let mut c = Connection::with_limits(limits);
        c.execute("CREATE TABLE t(long_name,b)", &[]).unwrap();
        assert!(matches!(
            c.execute("PRAGMA table_info(t)", &[]),
            Err(Error::Limit(_))
        ));
        // The limit failure is read-only and leaves the connection usable.
        c.execute("DROP TABLE t", &[]).unwrap();
        assert!(c
            .execute("PRAGMA table_info(t)", &[])
            .unwrap()
            .rows
            .is_empty());
    }
}
