//! SQLite rollback-journal snapshots and full-image undo records.
//!
//! These functions do not determine whether a journal is hot: that requires
//! filesystem locks and, for attached databases, super-journal coordination.
//! Callers must supply immutable snapshots belonging to the same transaction.
//! Damaged declared records fail closed instead of silently accepting partial
//! recovery. Super-journals and pre-3.5.8 page-size inference are unsupported.
use crate::{
    error::{slice, u32be},
    Database, Error, Result,
};
use alloc::{collections::BTreeSet, vec::Vec};

pub const MAGIC: [u8; 8] = [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7];

#[derive(Clone, Copy, Debug)]
pub struct JournalLimits {
    pub max_image_bytes: usize,
    pub max_journal_bytes: usize,
    pub max_records: usize,
}
impl Default for JournalLimits {
    fn default() -> Self {
        Self {
            max_image_bytes: 256 * 1024 * 1024,
            max_journal_bytes: 272 * 1024 * 1024,
            max_records: 100_000,
        }
    }
}
#[derive(Debug)]
pub struct Recovery {
    pub image: Vec<u8>,
    pub restored_pages: usize,
    pub ignored_tail_bytes: usize,
    pub active: bool,
}
fn power_size(n: usize, min: usize) -> bool {
    (min..=65536).contains(&n) && n.is_power_of_two()
}
fn checksum(data: &[u8], salt: u32) -> u32 {
    let mut sum = salt;
    let mut at = data.len().saturating_sub(200);
    while at > 0 {
        sum = sum.wrapping_add(u32::from(data[at]));
        at = at.saturating_sub(200);
    }
    sum
}
fn copy_image(data: &[u8], len: usize, limit: usize) -> Result<Vec<u8>> {
    if len > limit {
        return Err(Error::Limit("journal image bytes"));
    }
    let mut result = Vec::new();
    result
        .try_reserve_exact(len)
        .map_err(|_| Error::Limit("allocation"))?;
    result.extend_from_slice(&data[..data.len().min(len)]);
    result.resize(len, 0);
    Ok(result)
}
/// Encode every original database page, including pages removed by a truncating
/// commit. `salt` must be fresh for each transaction; the storage adapter supplies
/// randomness. An empty original database is permitted with an explicit page size.
pub fn encode(
    database: &[u8],
    page_size: usize,
    sector_size: usize,
    salt: u32,
    limits: JournalLimits,
) -> Result<Vec<u8>> {
    if !power_size(page_size, 512) || !power_size(sector_size, 32) {
        return Err(Error::InvalidInput("journal page/sector size"));
    }
    if database.len() > limits.max_image_bytes {
        return Err(Error::Limit("journal image bytes"));
    }
    if !database.is_empty() {
        let db = Database::parse(database)?;
        if db.header().page_size != page_size {
            return Err(Error::InvalidInput("journal page size mismatch"));
        }
        if database[18] != 1 || database[19] != 1 {
            return Err(Error::Unsupported("WAL database journal writing"));
        }
    }
    let count = database.len() / page_size;
    if count > limits.max_records {
        return Err(Error::Limit("journal records"));
    }
    let count32 = u32::try_from(count).map_err(|_| Error::Limit("journal records"))?;
    let length = count
        .checked_mul(page_size + 8)
        .and_then(|n| n.checked_add(sector_size))
        .ok_or(Error::Limit("journal bytes"))?;
    if length > limits.max_journal_bytes {
        return Err(Error::Limit("journal bytes"));
    }
    let mut journal = copy_image(&[], sector_size, limits.max_journal_bytes)?;
    journal
        .try_reserve_exact(length - sector_size)
        .map_err(|_| Error::Limit("allocation"))?;
    journal[..8].copy_from_slice(&MAGIC);
    journal[8..12].copy_from_slice(&count32.to_be_bytes());
    journal[12..16].copy_from_slice(&salt.to_be_bytes());
    journal[16..20].copy_from_slice(&count32.to_be_bytes());
    journal[20..24].copy_from_slice(&(sector_size as u32).to_be_bytes());
    journal[24..28].copy_from_slice(&(page_size as u32).to_be_bytes());
    for (i, page) in database.chunks_exact(page_size).enumerate() {
        let number = (i + 1) as u32;
        if number == 0x40000000 / page_size as u32 + 1 {
            return Err(Error::Unsupported("journal lock-byte page"));
        }
        journal.extend_from_slice(&number.to_be_bytes());
        journal.extend_from_slice(page);
        journal.extend_from_slice(&checksum(page, salt).to_be_bytes());
    }
    Ok(journal)
}

/// Apply complete, checksum-valid rollback records in journal order. Repeated
/// page numbers keep the first image; records beyond the original size are skipped.
/// Invalidated zero headers are inactive. A nonzero invalid header or damaged
/// declared record is an error and must not authorize deletion of the journal.
pub fn recover(database: &[u8], journal: &[u8], limits: JournalLimits) -> Result<Recovery> {
    if journal.len() > limits.max_journal_bytes || database.len() > limits.max_image_bytes {
        return Err(Error::Limit("journal input bytes"));
    }
    if journal.iter().take(8).all(|b| *b == 0) {
        return Ok(Recovery {
            image: copy_image(database, database.len(), limits.max_image_bytes)?,
            restored_pages: 0,
            ignored_tail_bytes: journal.len(),
            active: false,
        });
    }
    if slice(journal, 0, 8)? != MAGIC {
        return Err(Error::Corrupt("journal signature"));
    }
    // A super-journal trailer changes whether rollback is appropriate. Its file
    // existence cannot be decided from these two byte snapshots.
    if journal.len() >= 16 && journal[journal.len() - 8..] == MAGIC {
        return Err(Error::Unsupported("super-journal coordination"));
    }
    let page_size =
        usize::try_from(u32be(journal, 24)?).map_err(|_| Error::Limit("journal page size"))?;
    let sector =
        usize::try_from(u32be(journal, 20)?).map_err(|_| Error::Limit("journal sector size"))?;
    if !power_size(page_size, 512) || !power_size(sector, 32) {
        return Err(Error::Corrupt("journal page/sector size"));
    }
    let original =
        usize::try_from(u32be(journal, 16)?).map_err(|_| Error::Limit("journal original pages"))?;
    let length = original
        .checked_mul(page_size)
        .ok_or(Error::Limit("journal image bytes"))?;
    let mut image = copy_image(database, length, limits.max_image_bytes)?;
    let mut restored = BTreeSet::new();
    let mut records = 0usize;
    let mut header = 0usize;
    let mut consumed;
    loop {
        slice(journal, header, sector)?;
        let salt = u32be(journal, header + 12)?;
        let declared = u32be(journal, header + 8)?;
        let mut at = header
            .checked_add(sector)
            .ok_or(Error::Limit("journal offset"))?;
        let count = if declared == u32::MAX {
            if header != 0 {
                return Err(Error::Corrupt("journal unlimited record count"));
            }
            (journal.len() - at) / (page_size + 8)
        } else {
            usize::try_from(declared).map_err(|_| Error::Limit("journal records"))?
        };
        records = records
            .checked_add(count)
            .ok_or(Error::Limit("journal records"))?;
        if records > limits.max_records {
            return Err(Error::Limit("journal records"));
        }
        for _ in 0..count {
            let record = slice(journal, at, page_size + 8)?;
            let number = u32be(record, 0)?;
            if number == 0 || number == 0x40000000 / page_size as u32 + 1 {
                return Err(Error::Corrupt("journal page number"));
            }
            let page = &record[4..4 + page_size];
            if checksum(page, salt) != u32be(record, 4 + page_size)? {
                return Err(Error::Corrupt("journal checksum"));
            }
            let number =
                usize::try_from(number).map_err(|_| Error::Limit("journal page number"))?;
            if number <= original && restored.insert(number) {
                let start = (number - 1) * page_size;
                image[start..start + page_size].copy_from_slice(page);
            }
            at += page_size + 8;
        }
        consumed = at;
        if declared == u32::MAX || declared == 0 {
            break;
        }
        let next = at
            .checked_add(sector - 1)
            .ok_or(Error::Limit("journal offset"))?
            / sector
            * sector;
        if next >= journal.len() || journal[next..].iter().take(8).all(|b| *b == 0) {
            break;
        }
        if slice(journal, next, 8)? != MAGIC {
            return Err(Error::Corrupt("journal segment signature"));
        }
        header = next;
    }
    // Missing tail pages cannot be invented when a database was truncated. Every
    // page absent from the supplied main file must have an undo record.
    for number in database.len() / page_size + 1..=original {
        if !restored.contains(&number) {
            return Err(Error::Corrupt("journal missing truncated page"));
        }
    }
    if !image.is_empty() {
        let db = Database::parse(&image)?;
        if db.header().page_size != page_size {
            return Err(Error::Corrupt("recovered page size"));
        }
    }
    Ok(Recovery {
        image,
        restored_pages: restored.len(),
        ignored_tail_bytes: journal.len() - consumed,
        active: true,
    })
}
