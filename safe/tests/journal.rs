#![forbid(unsafe_code)]
use sqlite_safe::{
    journal::{self, JournalLimits},
    ImageBuilder, Table, Text, Value,
};
fn image(page_size: usize, count: i64) -> Vec<u8> {
    let mut b = ImageBuilder::new(page_size).unwrap();
    let mut t = Table::new("t", &["value"]);
    for i in 0..count {
        t.rows.push((
            i,
            vec![Value::Text(Text::utf8(&format!(
                "row-{i}-{}",
                "x".repeat(200)
            )))],
        ));
    }
    b.add_table(t).unwrap();
    b.finish().unwrap()
}
#[test]
fn rollback_all_page_sizes_and_growth_truncation_empty_origins() {
    for page_size in [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536] {
        let old = image(page_size, 30);
        for sector in [512, 4096, 65536] {
            let journal = journal::encode(
                &old,
                page_size,
                sector,
                u32::MAX - 1,
                JournalLimits::default(),
            )
            .unwrap();
            for main in [
                image(page_size, 5),
                image(page_size, 100),
                vec![0u8; page_size / 2],
                Vec::new(),
            ] {
                let recovery = journal::recover(&main, &journal, JournalLimits::default()).unwrap();
                assert_eq!(recovery.image, old);
                assert_eq!(recovery.restored_pages, old.len() / page_size);
                assert!(recovery.active);
                assert_eq!(recovery.ignored_tail_bytes, 0);
            }
        }
        let empty = journal::encode(&[], page_size, 512, 17, JournalLimits::default()).unwrap();
        assert!(journal::recover(&old, &empty, JournalLimits::default())
            .unwrap()
            .image
            .is_empty());
    }
}
#[test]
fn journal_headers_records_budgets_and_duplicate_first_image() {
    let old = image(512, 5);
    let mut journal =
        journal::encode(&old, 512, 512, 0x12345678, JournalLimits::default()).unwrap();
    assert!(journal::recover(
        &old,
        &journal,
        JournalLimits {
            max_records: 1,
            ..JournalLimits::default()
        }
    )
    .is_err());
    assert!(journal::encode(
        &old,
        512,
        512,
        1,
        JournalLimits {
            max_journal_bytes: 512,
            ..JournalLimits::default()
        }
    )
    .is_err());
    for cut in 1..journal.len() {
        assert!(
            journal::recover(&old, &journal[..cut], JournalLimits::default()).is_err(),
            "cut {cut}"
        );
    }
    // A second sector-aligned segment with different page contents must not
    // override the first undo image of that page.
    journal.resize(journal.len().div_ceil(512) * 512, 0);
    let other = image(512, 3);
    let second = journal::encode(&other, 512, 512, 19, JournalLimits::default()).unwrap();
    journal.extend_from_slice(&second);
    assert_eq!(
        journal::recover(&[], &journal, JournalLimits::default())
            .unwrap()
            .image,
        old
    );
    journal[512 + 4 + 312] ^= 1; // Covered by the native sparse checksum.
    assert!(journal::recover(&old, &journal, JournalLimits::default()).is_err());
    journal[..8].fill(0);
    let inactive = journal::recover(&old, &journal, JournalLimits::default()).unwrap();
    assert!(!inactive.active);
    assert_eq!(inactive.image, old);
}
#[test]
fn mutated_journals_are_bounded_and_never_panic() {
    let old = image(512, 5);
    let seed = journal::encode(&old, 512, 512, 91, JournalLimits::default()).unwrap();
    let limits = JournalLimits {
        max_records: 100,
        max_image_bytes: 65536,
        max_journal_bytes: 65536,
    };
    let mut random = 0x91893279u64;
    for _ in 0..3000 {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        let mut data = seed.clone();
        let at = random as usize % data.len();
        data[at] ^= (random >> 32) as u8;
        if random & 7 == 0 {
            data.truncate(at);
        }
        assert!(std::panic::catch_unwind(|| journal::recover(&old, &data, limits)).is_ok());
    }
}
