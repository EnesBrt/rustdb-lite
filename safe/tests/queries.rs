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

#[test]
fn scoped_sources_preserve_types_collations_aliases_and_namespace() {
    let mut c = Connection::new();
    c.execute_batch(
        "CREATE TABLE t(x TEXT COLLATE NOCASE,n INTEGER);INSERT INTO t VALUES('a',1),('B',2);",
    )
    .unwrap();
    assert_eq!(
        c.execute("SELECT x='A',n='1' FROM (SELECT x,n FROM t) WHERE n=1", &[])
            .unwrap()
            .rows,
        vec![vec![i(1), i(1)]]
    );
    assert_eq!(
        c.execute(
            "SELECT a,\"a:1\",\"a:2\" FROM (SELECT 1 a,2 a,3 \"a:1\")",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(1), i(2), i(3)]]
    );
    assert_eq!(
        c.execute(
            "WITH t(v) AS (VALUES(9)) SELECT v,n FROM t,main.t ORDER BY n",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(9), i(1)], vec![i(9), i(2)]]
    );
    assert_eq!(
        c.execute(
            "WITH q AS(SELECT * FROM later),later(v) AS(VALUES(7)) SELECT v FROM q",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(7)]]
    );
    assert_eq!(c.execute("WITH q(v) AS(VALUES(1)) SELECT a.v,b.v FROM q a,(WITH q(v) AS(VALUES(2)) SELECT v FROM q) b",&[]).unwrap().rows,vec![vec![i(1),i(2)]]);
    assert!(c
        .execute("SELECT rowid FROM (SELECT x FROM t)", &[])
        .is_err());
    assert!(c
        .execute("WITH q AS(SELECT 1),Q AS(SELECT 2) SELECT 3", &[])
        .is_err());
}

#[test]
fn compound_queries_handle_nulls_numeric_types_and_global_ordering() {
    let mut c = Connection::new();
    assert_eq!(
        c.execute("SELECT 1 UNION SELECT 1.0", &[]).unwrap().rows,
        vec![vec![Value::Real(1.0)]]
    );
    assert_eq!(
        c.execute(
            "SELECT 'a' COLLATE NOCASE x UNION SELECT 'A' ORDER BY x",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![t("A")]]
    );
    assert!(c.execute("SELECT x FROM (VALUES(1),(2)) q", &[]).is_err());
    assert_eq!(c.execute("SELECT NULL x UNION SELECT NULL UNION ALL SELECT 2 UNION ALL SELECT 1 ORDER BY x DESC NULLS LAST LIMIT 2 OFFSET 1",&[]).unwrap().rows,vec![vec![i(1)],vec![Value::Null]]);
    assert_eq!(
        c.execute(
            "SELECT 1 x UNION SELECT 2 INTERSECT SELECT 2 EXCEPT SELECT 3",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(2)]]
    );
    assert!(c
        .execute("SELECT 1 x UNION SELECT 2 ORDER BY x+1", &[])
        .is_err());
    assert!(c.execute("SELECT 1 UNION SELECT 2,3", &[]).is_err());
    assert!(c.execute("VALUES(1),(2,3)", &[]).is_err());
}

#[test]
fn recursive_work_queues_limit_offsets_priority_and_cycle_suppression() {
    let mut c = Connection::new();
    let rows=c.execute("WITH RECURSIVE q(x) AS(VALUES(0) UNION ALL SELECT x+1 FROM q) SELECT x FROM q LIMIT 4 OFFSET 2",&[]).unwrap().rows;
    assert_eq!(rows, vec![vec![i(2)], vec![i(3)], vec![i(4)], vec![i(5)]]);
    assert_eq!(
        c.execute(
            "WITH q(x) AS(VALUES(0) UNION SELECT (x+1)%3 FROM q) SELECT x FROM q ORDER BY x DESC",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(2)], vec![i(1)], vec![i(0)]]
    );
    assert_eq!(
        c.execute(
            "WITH q(x) AS(VALUES(NULL) UNION SELECT x FROM q) SELECT x FROM q",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![Value::Null]]
    );
    assert_eq!(c.execute("WITH q(x,d) AS(VALUES(1,0) UNION ALL SELECT x*2,d+1 FROM q WHERE d<2 UNION ALL SELECT x*2+1,d+1 FROM q WHERE d<2 ORDER BY 2 DESC,1) SELECT x FROM q",&[]).unwrap().rows,vec![vec![i(1)],vec![i(2)],vec![i(4)],vec![i(5)],vec![i(3)],vec![i(6)],vec![i(7)]]);
    for sql in [
        "WITH q(x) AS(VALUES(1) UNION ALL SELECT sum(x) FROM q) SELECT * FROM q",
        "WITH q(x) AS(VALUES(1) UNION ALL SELECT a.x FROM q a,q b) SELECT * FROM q",
        "WITH q AS(SELECT * FROM r),r AS(SELECT * FROM q) SELECT * FROM q",
    ] {
        assert!(c.execute(sql, &[]).is_err());
    }
}

#[test]
fn cte_bindings_are_statement_local_and_data_changes_roll_back() {
    let mut c = Connection::new();
    let query = c
        .prepare("WITH q(x) AS(VALUES(?1)) SELECT a.x,b.x FROM q a,q b")
        .unwrap();
    for n in [1, 2, 3] {
        assert_eq!(
            c.execute_prepared(&query, &[i(n)]).unwrap().rows,
            vec![vec![i(n), i(n)]]
        );
    }
    c.execute_batch("CREATE TABLE out(x UNIQUE);BEGIN;WITH RECURSIVE q(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM q WHERE x<5) INSERT INTO out SELECT x FROM q;ROLLBACK;").unwrap();
    assert_eq!(
        c.execute("SELECT count(*) FROM out", &[]).unwrap().rows,
        vec![vec![i(0)]]
    );
    assert!(c
        .execute(
            "WITH q(x) AS(VALUES(1),(1)) INSERT INTO out SELECT x FROM q",
            &[]
        )
        .is_err());
    assert_eq!(
        c.execute("SELECT count(*) FROM out", &[]).unwrap().rows,
        vec![vec![i(0)]]
    );
    assert_eq!(
        c.execute(
            "WITH unused AS(SELECT abs(-9223372036854775808)) SELECT 42",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![i(42)]]
    );
    assert!(c
        .execute(
            "SELECT x FROM (SELECT abs(-9223372036854775808) x) LIMIT 0",
            &[]
        )
        .unwrap()
        .rows
        .is_empty());
}

#[test]
fn query_nesting_recursion_and_materialization_are_bounded() {
    let mut c = Connection::with_limits(SqlLimits {
        max_steps: 200,
        ..Default::default()
    });
    assert!(matches!(
        c.execute(
            "WITH q(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM q) SELECT * FROM q",
            &[]
        ),
        Err(Error::Limit(_))
    ));
    let mut nested = "SELECT 1 x".to_string();
    for _ in 0..40 {
        nested = format!("SELECT x FROM ({nested})");
    }
    assert!(matches!(c.execute(&nested, &[]), Err(Error::Limit(_))));
    // Each recursive arm reads the next CTE. This nesting happens at execution,
    // despite none of the CTE definitions being syntactically nested.
    let definitions: Vec<_> = (0..80)
        .map(|n| {
            format!(
                "q{n}(x) AS(VALUES(0) UNION ALL SELECT b.x FROM q{n} a,q{} b WHERE a.x<1)",
                n + 1
            )
        })
        .collect();
    let chain = format!(
        "WITH {},q80(x) AS(VALUES(1)) SELECT * FROM q0",
        definitions.join(",")
    );
    assert!(matches!(
        Connection::new().execute(&chain, &[]),
        Err(Error::Limit("query nesting"))
    ));
    let seeds = [
        "WITH q(a) AS(VALUES(1)) SELECT a FROM q",
        "SELECT 1 UNION ALL SELECT 2 ORDER BY 1 LIMIT 3",
        "SELECT * FROM (WITH q AS(SELECT 1) SELECT * FROM q)",
    ];
    for seed in seeds {
        for end in 0..seed.len() {
            let result = std::panic::catch_unwind(|| Connection::new().execute(&seed[..end], &[]));
            assert!(result.is_ok(), "{seed} prefix {end}");
        }
    }
    let mut c = Connection::with_limits(SqlLimits {
        max_rows: 2,
        ..Default::default()
    });
    assert!(matches!(
        c.execute("VALUES(1),(2),(3)", &[]),
        Err(Error::Limit(_))
    ));
    assert!(matches!(
        c.execute("SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3", &[]),
        Err(Error::Limit(_))
    ));
}
