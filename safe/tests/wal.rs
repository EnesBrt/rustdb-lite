#![forbid(unsafe_code)]
use sqlite_safe::{wal, Database, Error, Value};

const BASE: &[u8] = include_bytes!("fixtures/wal-base.bin");
const NATIVE: &[u8] = include_bytes!("fixtures/wal-commits.bin");
const BIG_ENDIAN: &[u8] = include_bytes!("fixtures/wal-commits-be.bin");
const UNCOMMITTED: &[u8] = include_bytes!("fixtures/wal-uncommitted.bin");
const FRAME_BYTES: usize = 512 + 24;

fn values(image: &[u8]) -> Vec<i64> {
    let db = Database::parse(image).unwrap();
    let schema = db.schema().unwrap();
    if schema.is_empty() {
        return Vec::new();
    }
    assert_eq!(schema.len(), 1);
    assert_eq!(schema[0].name, "t");
    assert_eq!(schema[0].root_page, 2);
    db.rows(2)
        .unwrap()
        .into_iter()
        .map(|row| match row.values.as_slice() {
            [Value::Integer(n)] => *n,
            other => panic!("unexpected fixture record: {other:?}"),
        })
        .collect()
}

#[test]
fn native_fixtures_recover_both_checksum_byte_orders() {
    let expected: Vec<i64> = (0..10).chain([99]).collect();
    for fixture in [NATIVE, BIG_ENDIAN] {
        for base in [BASE, &[]] {
            let recovered = wal::recover(base, fixture, 4096, 10).unwrap();
            assert_eq!(recovered.valid_frames, 4);
            assert_eq!(recovered.committed_frames, 4);
            assert_eq!(recovered.ignored_tail_bytes, 0);
            assert_eq!(recovered.image.len(), 1024);
            assert_eq!(values(&recovered.image), expected);
        }
    }
}

#[test]
fn every_truncation_keeps_only_complete_commits() {
    for fixture in [NATIVE, BIG_ENDIAN] {
        for length in 0..=fixture.len() {
            let result = wal::recover(BASE, &fixture[..length], 4096, 10);
            if length < 32 {
                assert_eq!(result.unwrap_err(), Error::Truncated);
                continue;
            }
            let result = result.unwrap();
            let complete = (length - 32) / FRAME_BYTES;
            let committed = if complete < 2 { 0 } else { complete };
            assert_eq!(result.valid_frames, complete);
            assert_eq!(result.committed_frames, committed);
            assert_eq!(
                result.ignored_tail_bytes,
                length - 32 - committed * FRAME_BYTES
            );
            let expected: Vec<i64> = match committed {
                0 | 2 => Vec::new(),
                3 => (0..10).collect(),
                4 => (0..10).chain([99]).collect(),
                _ => unreachable!(),
            };
            assert_eq!(values(&result.image), expected, "truncation at {length}");
        }
    }
}

#[test]
fn uncommitted_stale_and_checksum_damaged_tails_are_ignored() {
    let expected: Vec<i64> = (0..10).collect();
    let uncommitted = wal::recover(BASE, UNCOMMITTED, 4096, 10).unwrap();
    assert_eq!(uncommitted.valid_frames, 4);
    assert_eq!(uncommitted.committed_frames, 3);
    assert_eq!(uncommitted.ignored_tail_bytes, FRAME_BYTES);
    assert_eq!(values(&uncommitted.image), expected);

    let last_frame = 32 + 3 * FRAME_BYTES;
    // Frame salt, frame checksum, and payload corruption respectively.
    for position in [last_frame + 8, last_frame + 16, NATIVE.len() - 1] {
        let mut damaged = NATIVE.to_vec();
        damaged[position] ^= 1;
        let recovered = wal::recover(BASE, &damaged, 4096, 10).unwrap();
        assert_eq!(recovered.valid_frames, 3);
        assert_eq!(recovered.committed_frames, 3);
        assert_eq!(recovered.ignored_tail_bytes, FRAME_BYTES);
        assert_eq!(values(&recovered.image), expected);
    }
}

#[test]
fn wal_header_errors_and_resource_limits_are_reported() {
    assert!(matches!(
        wal::recover(BASE, NATIVE, 512, 10),
        Err(Error::Limit(_))
    ));
    assert!(matches!(
        wal::recover(BASE, NATIVE, 4096, 3),
        Err(Error::Limit(_))
    ));
    assert!(wal::recover(&BASE[..511], NATIVE, 4096, 10).is_err());
    let mut wrong_size = BASE.to_vec();
    wrong_size[16..18].copy_from_slice(&1024u16.to_be_bytes());
    assert!(wal::recover(&wrong_size, NATIVE, 4096, 10).is_err());
    for position in [0, 4, 8, 16, 24] {
        let mut damaged = NATIVE.to_vec();
        damaged[position] ^= 1;
        assert!(wal::recover(BASE, &damaged, 4096, 10).is_err());
    }
}

#[test]
fn native_wal_seed_mutations_never_panic() {
    let mut random = 0x1032547698badcfeu64;
    for iteration in 0..3000 {
        let mut mutated = if iteration % 2 == 0 {
            NATIVE
        } else {
            BIG_ENDIAN
        }
        .to_vec();
        for _ in 0..1 + iteration % 8 {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            let position = random as usize % mutated.len();
            mutated[position] ^= (random >> 32) as u8;
        }
        let outcome = std::panic::catch_unwind(|| {
            if let Ok(recovered) = wal::recover(BASE, &mutated, 4096, 10) {
                assert!(recovered.image.len() <= 4096);
                if let Ok(db) = Database::parse(&recovered.image) {
                    if let Ok(schema) = db.schema() {
                        for entry in schema {
                            if entry.root_page != 0 {
                                let _ = db.rows(entry.root_page);
                            }
                        }
                    }
                }
            }
        });
        assert!(outcome.is_ok(), "WAL mutation {iteration} panicked");
    }
}
