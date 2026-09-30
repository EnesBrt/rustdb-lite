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
fn glob_classes_unicode_and_case_rules() {
    let mut c = Connection::new();
    assert_eq!(c.execute("SELECT 'Ab' GLOB 'A*','Ab' GLOB 'a*','é' GLOB '[é-ê]','🦀' GLOB '?','A' GLOB '[a-z]' COLLATE NOCASE,'Ab' LIKE 'a%'",&[]).unwrap().rows,vec![vec![i(1),i(0),i(1),i(1),i(0),i(1)]]);
    assert_eq!(c.execute("SELECT ']' GLOB '[]]','a' GLOB '[^]]','z' GLOB '[z-a]','a' GLOB '[z-a]','-' GLOB '[-a]','a' GLOB '[]','a' GLOB '*['",&[]).unwrap().rows,vec![vec![i(1),i(1),i(1),i(0),i(1),i(0),i(0)]]);
    assert_eq!(
        c.execute(
            "SELECT NULL GLOB '*',NULL NOT GLOB '*','a' NOT GLOB 'b','a' LIKE 'A','é' LIKE 'É'",
            &[]
        )
        .unwrap()
        .rows,
        vec![vec![Value::Null, Value::Null, i(1), i(1), i(0)]]
    );
}

#[test]
fn escape_precedence_parameters_and_error_order() {
    let mut c = Connection::new();
    let p = c
        .prepare("SELECT ?1 LIKE ?2 ESCAPE ?3,like(?2,?1,?3),?1 NOT LIKE ?2 ESCAPE ?3")
        .unwrap();
    for (text, pattern, escape) in [
        ("a_b", "a!_b", "!"),
        ("a%b", "aé%b", "é"),
        ("a%b", "a%%b", "%"),
        ("a_b", "a__b", "_"),
    ] {
        assert_eq!(
            c.execute_prepared(&p, &[t(text), t(pattern), t(escape)])
                .unwrap()
                .rows,
            vec![vec![i(1), i(1), i(0)]]
        );
    }
    assert_eq!(
        c.execute_prepared(&p, &[t("a"), t("a"), Value::Null])
            .unwrap()
            .rows,
        vec![vec![Value::Null; 3]]
    );
    assert_eq!(c.execute("SELECT 'a' LIKE 'a' ESCAPE '!'='x','a' LIKE 'a' ESCAPE '!'||'','a' LIKE 'a' ESCAPE '!' < 'z',NOT 'a' GLOB 'a*'",&[]).unwrap().rows,vec![vec![i(0),i(1),i(1),i(0)]]);
    for q in [
        "SELECT NULL LIKE 'a' ESCAPE ''",
        "SELECT like(NULL,'a','xx')",
        "SELECT 'a' LIKE NULL ESCAPE ''",
        "SELECT 'a' GLOB '*' ESCAPE '!'",
        "SELECT 'a' LIKE '%' ESCAPE char(0)",
    ] {
        assert!(c.execute(q, &[]).is_err(), "{q}");
    }
    assert_eq!(c.execute("SELECT CASE WHEN 1 THEN 7 ELSE 'a' LIKE '%' ESCAPE '' END,like('a','a','!'||char(0)||'tail')",&[]).unwrap().rows,vec![vec![i(7),i(1)]]);
}

#[test]
fn blob_patterns_and_embedded_nuls_follow_native_character_boundaries() {
    let mut c = Connection::new();
    assert_eq!(c.execute("SELECT glob(x'C080',x'EFBFBD'),glob(x'80',x'C280'),like(x'610062',x'61'),glob('*',x'00'),glob('?',x'00'),like(x'25',x'FF')",&[]).unwrap().rows,vec![vec![i(1),i(1),i(1),i(1),i(0),i(1)]]);
    let mut source = vec![0xff];
    source.extend(vec![0x80; 80]);
    let p = c.prepare("SELECT glob(?1,?1),like(?1,?1)").unwrap();
    assert_eq!(
        c.execute_prepared(&p, &[Value::Blob(source)]).unwrap().rows,
        vec![vec![i(1), i(1)]]
    );
    for encoding in [Encoding::Utf16Le, Encoding::Utf16Be] {
        let image = ImageBuilder::new(512)
            .unwrap()
            .encoding(encoding)
            .unwrap()
            .finish()
            .unwrap();
        let mut c = Connection::from_image(&image).unwrap();
        let b = if encoding == Encoding::Utf16Le {
            vec![b'A', 0]
        } else {
            vec![0, b'A']
        };
        assert_eq!(
            c.execute(
                "SELECT ?1 GLOB 'A',?1 LIKE 'a',?2 GLOB ''",
                &[Value::Blob(b), Value::Blob(vec![b'A'])]
            )
            .unwrap()
            .rows,
            vec![vec![i(1), i(1), i(1)]]
        );
    }
}

#[test]
fn patterns_work_in_indexes_generated_columns_and_rollback() {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(id INT PRIMARY KEY,name TEXT,literal INT AS(name LIKE '%!_%' ESCAPE '!') STORED,CHECK(name GLOB '[A-Za-z]*'));INSERT INTO t(id,name) VALUES(1,'A_one'),(2,'b_two');CREATE UNIQUE INDEX ix ON t((name LIKE 'A!_%' ESCAPE '!')) WHERE name GLOB 'A*';CREATE VIEW v AS SELECT id,name,literal FROM t WHERE name GLOB '[A-Z]*';").unwrap();
    c.execute("INSERT INTO t(id,name) VALUES(3,'A_new') ON CONFLICT(like('A!_%',name,'!')) WHERE glob('A*',name) DO UPDATE SET name=excluded.name",&[]).unwrap();
    assert_eq!(
        c.execute("SELECT * FROM v", &[]).unwrap().rows,
        vec![vec![i(1), t("A_new"), i(1)]]
    );
    let image = c.to_image(512).unwrap();
    c.execute_batch(
        "BEGIN;UPDATE t SET name=upper(name) WHERE name LIKE '%!_%' ESCAPE '!';ROLLBACK;",
    )
    .unwrap();
    assert_eq!(image, c.to_image(512).unwrap());
    for q in [
        "UPDATE t SET name='bad' WHERE name LIKE '%' ESCAPE ''",
        "INSERT INTO t(id,name) VALUES(4,'_bad')",
    ] {
        assert!(c.execute(q, &[]).is_err());
        assert_eq!(image, c.to_image(512).unwrap());
    }
}

#[test]
fn pattern_limits_and_deep_syntax_return_bounded_errors() {
    let mut c = Connection::new();
    let p = c.prepare("SELECT glob(?1,NULL)").unwrap();
    assert_eq!(
        c.execute_prepared(&p, &[t(&"a".repeat(50_000))])
            .unwrap()
            .rows,
        vec![vec![Value::Null]]
    );
    assert!(matches!(
        c.execute_prepared(&p, &[t(&"a".repeat(50_001))]),
        Err(Error::Limit(_))
    ));
    assert_eq!(
        c.execute(
            "SELECT glob(?1,?2)",
            &[t(&"*a".repeat(10_000)), t(&"a".repeat(10_000))]
        )
        .unwrap()
        .rows,
        vec![vec![i(1)]]
    );
    for (prefix, suffix) in [
        ("'x' LIKE (", ")"),
        ("'x' GLOB (", ")"),
        ("'x' LIKE 'x' ESCAPE (", ")"),
    ] {
        let q = format!("SELECT {}1{}", prefix.repeat(1000), suffix.repeat(1000));
        assert!(matches!(c.execute(&q, &[]), Err(Error::Limit(_))));
    }
    let mut limited = Connection::with_limits(SqlLimits {
        max_steps: 200,
        ..SqlLimits::default()
    });
    assert!(matches!(
        limited.execute(
            "SELECT like(?1,?2)",
            &[t("%aaaaaaaaab"), t(&"a".repeat(1000))]
        ),
        Err(Error::Limit(_))
    ));
    assert_eq!(
        limited.execute("SELECT 1", &[]).unwrap().rows,
        vec![vec![i(1)]]
    );
    let seed = "SELECT 'a_b' LIKE 'a!_b' ESCAPE '!' OR 'a' GLOB '[a-z]'";
    for end in 1..seed.len() {
        let _ = c.execute(&seed[..end], &[]);
    }
}

#[test]
fn pattern_schema_roundtrips_all_image_configurations() {
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
                c.execute_batch("CREATE TABLE t(id INT PRIMARY KEY,name TEXT,literal INT AS(name LIKE '%!_%' ESCAPE '!') STORED);INSERT INTO t(id,name) VALUES(1,'A_one'),(2,'é_été'),(3,'🦀');CREATE VIEW v AS SELECT * FROM t WHERE name GLOB '[A-Zé]*';CREATE INDEX ix ON t((name LIKE 'A!_%' ESCAPE '!')) WHERE name GLOB '[A-Z]*';UPDATE t SET name=lower(name) WHERE name GLOB '[A-Z]*';").unwrap();
                let mut restored = Connection::from_image(&c.to_image(size).unwrap()).unwrap();
                assert_eq!(
                    restored
                        .execute("SELECT id,literal FROM t ORDER BY id", &[])
                        .unwrap()
                        .rows,
                    vec![vec![i(1), i(1)], vec![i(2), i(1)], vec![i(3), i(0)]]
                );
                assert_eq!(
                    restored.execute("SELECT id FROM v", &[]).unwrap().rows,
                    vec![vec![i(2)]]
                );
                restored
                    .execute(
                        "UPDATE t SET name='R_more' WHERE name LIKE 'a!_%' ESCAPE '!'",
                        &[],
                    )
                    .unwrap();
                restored.to_image(size).unwrap();
            }
        }
    }
}
