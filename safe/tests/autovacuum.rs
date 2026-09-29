#![forbid(unsafe_code)]
use sqlite_safe::{
    sql::Connection, AutoVacuum, Database, Error, ImageBuilder, Limits, Table, Value,
};

#[test]
fn pointer_map_pages_and_roots_are_included_in_allocation_limits() {
    for mode in [AutoVacuum::Full, AutoVacuum::Incremental] {
        let mut builder = ImageBuilder::new(512).unwrap().auto_vacuum(mode).limits(
            Limits {
                max_pages: 2,
                ..Default::default()
            },
            1536,
        );
        builder.add_table(Table::new("t", &["x"])).unwrap();
        assert!(matches!(builder.finish(), Err(Error::Limit(_))));
        let mut builder = ImageBuilder::new(512)
            .unwrap()
            .auto_vacuum(mode)
            .limits(Limits::default(), 1024);
        builder.add_table(Table::new("t", &["x"])).unwrap();
        assert!(matches!(builder.finish(), Err(Error::Limit(_))));
        let mut builder = ImageBuilder::new(512).unwrap().auto_vacuum(mode);
        // The roots themselves cross three pointer-map page positions.
        for n in 0..320 {
            let mut table = Table::new(&format!("table{n}"), &["x"]);
            table.rows.push((1, vec![Value::Blob(vec![n as u8; 1300])]));
            builder.add_table(table).unwrap();
        }
        let image = builder.finish().unwrap();
        let db = Database::parse(&image).unwrap();
        assert_eq!(db.header().auto_vacuum, mode);
        let schema = db.schema().unwrap();
        assert_eq!(schema.len(), 320);
        assert_eq!(
            db.header().largest_root_page,
            schema.iter().map(|s| s.root_page).max().unwrap()
        );
        for (n, entry) in schema.iter().enumerate() {
            assert_eq!(
                db.rows(entry.root_page).unwrap()[0].values,
                vec![Value::Blob(vec![n as u8; 1300])]
            );
            let map = (entry.root_page - 2) / 103 * 103 + 2;
            let offset = (map as usize - 1) * 512 + (entry.root_page - map - 1) as usize * 5;
            assert_eq!(&image[offset..offset + 5], &[1, 0, 0, 0, 0]);
        }
    }
}

#[test]
fn sql_rebuild_preserves_modes_across_schema_changes_and_rollback() {
    for mode in [AutoVacuum::None, AutoVacuum::Full, AutoVacuum::Incremental] {
        for page_size in [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536] {
            let empty = ImageBuilder::new(page_size)
                .unwrap()
                .auto_vacuum(mode)
                .finish()
                .unwrap();
            assert_eq!(empty.len(), page_size);
            let mut connection = Connection::from_image(&empty).unwrap();
            connection.execute_batch("CREATE TABLE t(k TEXT UNIQUE,v); INSERT INTO t VALUES('a',1),('b',2);CREATE INDEX idx ON t(v DESC);BEGIN;DROP TABLE t;ROLLBACK;DELETE FROM t WHERE v=1;").unwrap();
            let image = connection.to_image(page_size).unwrap();
            let header = Database::parse(&image).unwrap().header().clone();
            assert_eq!(header.auto_vacuum, mode);
            let mut loaded = Connection::from_image(&image).unwrap();
            assert_eq!(
                loaded.execute("SELECT v FROM t", &[]).unwrap().rows,
                vec![vec![Value::Integer(2)]]
            );
            assert!(loaded.execute("INSERT INTO t VALUES('b',3)", &[]).is_err());
            assert_eq!(
                loaded.execute("PRAGMA auto_vacuum", &[]).unwrap().rows,
                vec![vec![Value::Integer(match mode {
                    AutoVacuum::None => 0,
                    AutoVacuum::Full => 1,
                    AutoVacuum::Incremental => 2,
                })]]
            );
            loaded.execute("DROP TABLE t", &[]).unwrap();
            let image = loaded.to_image(page_size).unwrap();
            assert_eq!(image.len(), page_size);
            assert_eq!(Database::parse(&image).unwrap().header().auto_vacuum, mode);
        }
    }
}

#[test]
fn malformed_auto_vacuum_header_combinations_are_rejected() {
    let mut image = ImageBuilder::new(512).unwrap().finish().unwrap();
    image[64..68].copy_from_slice(&1u32.to_be_bytes());
    assert!(Database::parse(&image).is_err());
    image[52..56].copy_from_slice(&2u32.to_be_bytes());
    assert!(Database::parse(&image).is_err());
    image[52..56].copy_from_slice(&1u32.to_be_bytes());
    assert_eq!(
        Database::parse(&image).unwrap().header().auto_vacuum,
        AutoVacuum::Incremental
    );
}
