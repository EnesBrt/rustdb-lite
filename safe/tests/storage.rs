#![forbid(unsafe_code)]
use sqlite_safe::{
    record, varint, Database, Encoding, Error, ImageBuilder, Limits, Table, Text, Value,
};

fn fixture(page_size: usize, count: usize) -> Vec<u8> {
    let mut table = Table::new("items", &["number", "text", "bytes"]);
    for n in 0..count {
        table.rows.push((
            n as i64 - 1000,
            vec![
                Value::Integer(n as i64),
                Value::Text(Text::utf8(&format!("row {n} 🦀\0尾"))),
                Value::Blob(vec![n as u8; if n % 31 == 0 { 7000 } else { 7 }]),
            ],
        ));
    }
    let mut builder = ImageBuilder::new(page_size).unwrap();
    builder.add_table(table).unwrap();
    builder.finish().unwrap()
}

#[test]
fn varint_golden_vectors_and_all_size_boundaries() {
    for (value, bytes) in [
        (0, vec![0]),
        (127, vec![127]),
        (128, vec![0x81, 0]),
        (16383, vec![0xff, 0x7f]),
        (16384, vec![0x81, 0x80, 0]),
        (u64::MAX, vec![0xff; 9]),
    ] {
        let mut actual = Vec::new();
        varint::encode(value, &mut actual).unwrap();
        assert_eq!(actual, bytes);
        assert_eq!(varint::decode(&bytes).unwrap(), (value, bytes.len()));
    }
    for bit in 0..64 {
        for value in [
            (1u64 << bit) - 1,
            1u64 << bit,
            (1u64 << bit).saturating_add(1),
        ] {
            let mut bytes = Vec::new();
            varint::encode(value, &mut bytes).unwrap();
            assert_eq!(bytes.len(), varint::encoded_len(value));
            assert_eq!(varint::decode(&bytes).unwrap(), (value, bytes.len()));
            for len in 0..bytes.len() {
                assert!(varint::decode(&bytes[..len]).is_err());
            }
        }
    }
}

#[test]
fn records_match_sqlite_serial_layout() {
    let values = vec![
        Value::Null,
        Value::Integer(0),
        Value::Integer(1),
        Value::Integer(-1),
        Value::Real(1.5),
        Value::Text(Text::utf8("hi")),
        Value::Blob(vec![255]),
    ];
    let expected = vec![
        8, 0, 8, 9, 1, 7, 17, 14, 255, 0x3f, 0xf8, 0, 0, 0, 0, 0, 0, b'h', b'i', 255,
    ];
    assert_eq!(record::encode(&values, 1024).unwrap(), expected);
    assert_eq!(
        record::decode(&expected, Encoding::Utf8, 10).unwrap(),
        values
    );
    for n in [
        i64::MIN,
        -140737488355329,
        -140737488355328,
        -2147483649,
        -2147483648,
        -8388609,
        -8388608,
        -32769,
        -32768,
        -129,
        -128,
        0,
        1,
        127,
        128,
        32767,
        32768,
        8388607,
        8388608,
        2147483647,
        2147483648,
        140737488355327,
        140737488355328,
        i64::MAX,
    ] {
        let values = [Value::Integer(n)];
        assert_eq!(
            record::decode(&record::encode(&values, 1024).unwrap(), Encoding::Utf8, 1).unwrap(),
            values
        );
    }
}

#[test]
fn record_lengths_invalid_serials_and_text_encodings() {
    let values = vec![Value::Null; 200];
    let encoded = record::encode(&values, 1000).unwrap();
    assert_eq!(varint::decode(&encoded).unwrap(), (202, 2));
    assert_eq!(
        record::decode(&encoded, Encoding::Utf8, 200).unwrap(),
        values
    );
    assert!(record::decode(&[2, 10], Encoding::Utf8, 1).is_err());
    assert!(record::decode(&[2, 11], Encoding::Utf8, 1).is_err());
    assert!(record::decode(&[2, 7], Encoding::Utf8, 1).is_err());
    assert!(record::decode(&[1, 0], Encoding::Utf8, 1).is_err());
    assert!(record::encode(&[Value::Blob(vec![0; 100])], 100).is_err());
    let text = "hello 🦀\0é";
    for encoding in [Encoding::Utf16Le, Encoding::Utf16Be] {
        let bytes = text
            .encode_utf16()
            .flat_map(|c| {
                if encoding == Encoding::Utf16Le {
                    c.to_le_bytes()
                } else {
                    c.to_be_bytes()
                }
            })
            .collect();
        let raw = Text { bytes, encoding };
        assert_eq!(raw.to_string().unwrap(), text);
        let values = [Value::Text(raw)];
        assert_eq!(
            record::decode(&record::encode(&values, 1024).unwrap(), encoding, 1).unwrap(),
            values
        );
    }
    assert!(Text {
        bytes: vec![0xff],
        encoding: Encoding::Utf8
    }
    .to_string()
    .is_err());
    assert!(Text {
        bytes: vec![0x00, 0xd8],
        encoding: Encoding::Utf16Le
    }
    .to_string()
    .is_err());
}

#[test]
fn multi_level_btrees_overflow_pages_and_all_page_sizes() {
    for page_size in [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536] {
        let bytes = fixture(page_size, 1500);
        let db = Database::parse(&bytes).unwrap();
        assert_eq!(db.header().page_size, page_size);
        let schema = db.schema().unwrap();
        assert_eq!(schema.len(), 1);
        let rows = db.rows(schema[0].root_page).unwrap();
        assert_eq!(rows.len(), 1500);
        for (n, row) in rows.iter().enumerate() {
            assert_eq!(row.rowid, Some(n as i64 - 1000));
            assert_eq!(row.values[0], Value::Integer(n as i64));
            assert_eq!(
                row.values[1],
                Value::Text(Text::utf8(&format!("row {n} 🦀\0尾")))
            );
            assert_eq!(
                row.values[2],
                Value::Blob(vec![n as u8; if n % 31 == 0 { 7000 } else { 7 }])
            );
        }
    }
}

#[test]
fn schema_splits_quoted_identifiers_and_empty_tables() {
    let mut builder = ImageBuilder::new(512).unwrap();
    for n in 0..400 {
        builder
            .add_table(Table::new(&format!("table {n} \"quoted\""), &["a", "b"]))
            .unwrap();
    }
    let bytes = builder.finish().unwrap();
    let db = Database::parse(&bytes).unwrap();
    let schema = db.schema().unwrap();
    assert_eq!(schema.len(), 400);
    for entry in schema {
        assert!(db.rows(entry.root_page).unwrap().is_empty());
        assert!(entry.sql.unwrap().contains("\"\"quoted\"\""));
    }
    for page_size in [512, 65536] {
        let empty = ImageBuilder::new(page_size).unwrap().finish().unwrap();
        assert!(Database::parse(&empty)
            .unwrap()
            .schema()
            .unwrap()
            .is_empty());
    }
}

#[test]
fn resource_budgets_invalid_header_and_cycle_detection() {
    let bytes = fixture(512, 40);
    let root = Database::parse(&bytes).unwrap().schema().unwrap()[0].root_page;
    for limits in [
        Limits {
            max_rows: 0,
            ..Limits::default()
        },
        Limits {
            max_payload_bytes: 1,
            ..Limits::default()
        },
        Limits {
            max_pages: 1,
            ..Limits::default()
        },
        Limits {
            max_columns: 1,
            ..Limits::default()
        },
        Limits {
            max_depth: 1,
            ..Limits::default()
        },
        Limits {
            max_total_payload_bytes: 10,
            ..Limits::default()
        },
    ] {
        assert!(Database::with_limits(&bytes, limits)
            .unwrap()
            .rows(root)
            .is_err());
    }
    for len in 0..100 {
        assert!(Database::parse(&bytes[..len]).is_err());
    }
    let mut cycle = ImageBuilder::new(512).unwrap().finish().unwrap();
    cycle[100] = 5;
    cycle[108..112].copy_from_slice(&1u32.to_be_bytes());
    assert!(matches!(
        Database::parse(&cycle).unwrap().schema(),
        Err(Error::Corrupt(_))
    ));
    let mut bad_header = bytes.clone();
    bad_header[16..18].copy_from_slice(&513u16.to_be_bytes());
    assert!(Database::parse(&bad_header).is_err());
}

#[test]
fn malformed_inputs_never_panic_with_bounded_resources() {
    let original = fixture(512, 50);
    let limits = Limits {
        max_payload_bytes: 64 * 1024,
        max_total_payload_bytes: 128 * 1024,
        max_pages: 1000,
        max_rows: 200,
        max_columns: 100,
        max_depth: 20,
    };
    let mut random = 0x445566778899u64;
    for iteration in 0..3000 {
        let mut bytes = original.clone();
        for _ in 0..1 + iteration % 8 {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            let position = (random as usize) % bytes.len();
            bytes[position] ^= (random >> 32) as u8;
        }
        if iteration % 4 == 0 {
            bytes.truncate((random as usize) % bytes.len());
        }
        let outcome = std::panic::catch_unwind(|| {
            if let Ok(db) = Database::with_limits(&bytes, limits) {
                if let Ok(schema) = db.schema() {
                    for entry in schema {
                        if entry.root_page != 0 {
                            let _ = db.rows(entry.root_page);
                        }
                    }
                }
            }
            let _ = record::decode(&bytes[..bytes.len().min(500)], Encoding::Utf8, 100);
            let _ = sqlite_safe::wal::recover(&original, &bytes, 128 * 1024, 100);
        });
        assert!(outcome.is_ok(), "mutation {iteration} panicked");
    }
}

#[test]
fn builder_rejects_inconsistent_inputs_and_enforces_limits() {
    let no_depth = ImageBuilder::new(512).unwrap().limits(
        Limits {
            max_depth: 0,
            ..Limits::default()
        },
        512,
    );
    assert_eq!(no_depth.finish(), Err(Error::Limit("B-tree depth")));
    let mut b = ImageBuilder::new(512).unwrap();
    assert!(b.add_table(Table::new("sqlite_internal", &["x"])).is_err());
    assert!(b.add_table(Table::new("t", &["x", "X"])).is_err());
    let mut t = Table::new("t", &["x"]);
    t.rows = vec![(1, vec![Value::Null]), (1, vec![Value::Null])];
    assert!(b.add_table(t).is_err());
    let mut b = ImageBuilder::new(512)
        .unwrap()
        .limits(Limits::default(), 512);
    b.add_table(Table::new("t", &["x"])).unwrap();
    assert!(b.finish().is_err());
}

#[test]
fn utf16_image_writer_encodes_schema_records_and_overflow() {
    use sqlite_safe::{Encoding, Text};
    for encoding in [Encoding::Utf16Le, Encoding::Utf16Be] {
        for page_size in [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536] {
            let mut builder = ImageBuilder::new(page_size)
                .unwrap()
                .encoding(encoding)
                .unwrap();
            let mut table = Table::new("🦀 table", &["unicode"]);
            let contents = "A\0Ā🦀\u{e000}".repeat(300);
            let text = Text::utf8(&contents).transcode(encoding).unwrap();
            for n in 0..80 {
                table.rows.push((n, vec![Value::Text(text.clone())]));
            }
            builder.add_table(table).unwrap();
            let image = builder.finish().unwrap();
            let db = Database::parse(&image).unwrap();
            assert_eq!(db.header().encoding, encoding);
            let schema = db.schema().unwrap();
            assert_eq!(schema[0].name, "🦀 table");
            let rows = db.rows(schema[0].root_page).unwrap();
            assert_eq!(rows.len(), 80);
            for row in rows {
                assert_eq!(row.values, vec![Value::Text(text.clone())]);
            }
        }
    }
}
