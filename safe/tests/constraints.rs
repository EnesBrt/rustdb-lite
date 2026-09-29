#![forbid(unsafe_code)]
use sqlite_safe::{sql::Connection, Database, Error, Value};

#[test]
fn composite_constraints_enforce_affinity_collation_and_statement_atomicity() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a TEXT COLLATE NOCASE,b INTEGER,c, CONSTRAINT pk PRIMARY KEY(a DESC,b),UNIQUE(b,c),CHECK(b>0 AND c>=b));INSERT INTO t VALUES('a',1,2),('A',2,4),(NULL,3,5),(NULL,3,6),('b',NULL,NULL);").unwrap();
    for sql in [
        "INSERT INTO t VALUES('A','1',9)",
        "INSERT INTO t VALUES('x',2,4)",
        "INSERT INTO t VALUES('x',-1,9)",
        "INSERT INTO t VALUES('x',4,3)",
        "UPDATE t SET a='a',b=1 WHERE b=2",
        "UPDATE t SET c=0",
    ] {
        assert!(
            matches!(c.execute(sql, &[]), Err(Error::Constraint(_))),
            "{sql}"
        );
    }
    assert_eq!(
        c.execute("SELECT count(*),sum(b) FROM t", &[])
            .unwrap()
            .rows,
        vec![vec![Value::Integer(5), Value::Integer(9)]]
    );
    assert!(c
        .execute("INSERT INTO t VALUES('z',9,10),('A',1,12)", &[])
        .is_err());
    assert_eq!(
        c.execute("SELECT count(*) FROM t", &[]).unwrap().rows,
        vec![vec![Value::Integer(5)]]
    );
    c.execute_batch("BEGIN;INSERT INTO t VALUES('z',9,10);ROLLBACK;")
        .unwrap();
    let image = c.to_image(512).unwrap();
    let mut imported = Connection::from_image(&image).unwrap();
    assert!(matches!(
        imported.execute("INSERT INTO t VALUES('A',1,8)", &[]),
        Err(Error::Constraint(_))
    ));
    assert!(matches!(
        imported.execute("UPDATE t SET c=-1", &[]),
        Err(Error::Constraint(_))
    ));
}

#[test]
fn table_primary_key_desc_aliases_rowid_but_composite_and_column_desc_do_not() {
    for declaration in [
        "x INTEGER,PRIMARY KEY(x)",
        "x INTEGER,PRIMARY KEY(x DESC)",
        "x INTEGER,PRIMARY KEY(x COLLATE NOCASE DESC)",
    ] {
        let mut c = Connection::new();
        c.execute(&format!("CREATE TABLE t({declaration})"), &[])
            .unwrap();
        c.execute_batch("INSERT INTO t VALUES(NULL),(5),(NULL);UPDATE t SET x=8 WHERE x=5;")
            .unwrap();
        assert_eq!(
            c.execute("SELECT rowid,x FROM t ORDER BY rowid", &[])
                .unwrap()
                .rows,
            vec![
                vec![Value::Integer(1); 2],
                vec![Value::Integer(6); 2],
                vec![Value::Integer(8); 2]
            ]
        );
        let image = c.to_image(512).unwrap();
        assert_eq!(Database::parse(&image).unwrap().schema().unwrap().len(), 1);
        let mut loaded = Connection::from_image(&image).unwrap();
        assert_eq!(
            loaded
                .execute("SELECT rowid,x FROM t ORDER BY rowid", &[])
                .unwrap()
                .rows,
            c.execute("SELECT rowid,x FROM t ORDER BY rowid", &[])
                .unwrap()
                .rows
        );
    }
    for declaration in [
        "x INTEGER PRIMARY KEY DESC,y",
        "x INTEGER,y,PRIMARY KEY(x,y)",
    ] {
        let mut c = Connection::new();
        c.execute(&format!("CREATE TABLE t({declaration})"), &[])
            .unwrap();
        c.execute_batch("INSERT INTO t VALUES(NULL,1),(5,2),(NULL,1);")
            .unwrap();
        assert_eq!(
            c.execute("SELECT rowid,x FROM t ORDER BY rowid", &[])
                .unwrap()
                .rows,
            vec![
                vec![Value::Integer(1), Value::Null],
                vec![Value::Integer(2), Value::Integer(5)],
                vec![Value::Integer(3), Value::Null]
            ]
        );
    }
}

#[test]
fn automatic_constraint_indexes_deduplicate_columns_and_collations_not_directions() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(a TEXT CONSTRAINT u UNIQUE COLLATE NOCASE,b,CONSTRAINT p PRIMARY KEY(a DESC),UNIQUE(a ASC),UNIQUE(a COLLATE BINARY),UNIQUE(a,b),UNIQUE(a DESC,b DESC),UNIQUE(b,a),CHECK(a IS NULL OR length(a)>0));INSERT INTO t VALUES('x',1),(NULL,2),(NULL,2);").unwrap();
    let image = c.to_image(512).unwrap();
    let db = Database::parse(&image).unwrap();
    let schema = db.schema().unwrap();
    let names: Vec<_> = schema
        .iter()
        .filter(|e| e.kind == "index")
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "sqlite_autoindex_t_1",
            "sqlite_autoindex_t_2",
            "sqlite_autoindex_t_3",
            "sqlite_autoindex_t_4"
        ]
    );
    let mut loaded = Connection::from_image(&image).unwrap();
    assert!(loaded.execute("INSERT INTO t VALUES('X',9)", &[]).is_err());
    assert!(loaded.execute("INSERT INTO t VALUES('',9)", &[]).is_err());
}

#[test]
fn invalid_table_constraints_do_not_install_partial_schema() {
    for declaration in [
        "x(3),y",
        "x INTEGER PRIMARY KEY,PRIMARY KEY(x)",
        "x,PRIMARY KEY(x),PRIMARY KEY(x)",
        "x,UNIQUE(missing)",
        "x,PRIMARY KEY(rowid)",
        "x,CHECK(missing>0)",
        "x,CHECK(?1)",
        "x,CHECK(count(x)>0)",
        "x,UNIQUE(x+1)",
        "x,CHECK(x),y",
    ] {
        let mut c = Connection::new();
        assert!(
            c.execute(&format!("CREATE TABLE t({declaration})"), &[])
                .is_err(),
            "{declaration}"
        );
        c.execute("CREATE TABLE t(x)", &[]).unwrap();
    }
}
