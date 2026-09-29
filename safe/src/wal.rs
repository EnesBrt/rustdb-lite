//! Recover an immutable pair of database/WAL snapshots to the last valid commit.
//! This does not implement WAL locks, a live WAL index, or a transaction writer.
use crate::error::{slice, u32be};
use crate::{Error, Result};
use alloc::vec::Vec;

#[derive(Debug)]
pub struct Recovery {
    pub image: Vec<u8>,
    pub valid_frames: usize,
    pub committed_frames: usize,
    /// Everything after the last committed frame, including valid uncommitted
    /// frames and any truncated, stale-salt, or invalid-checksum suffix.
    pub ignored_tail_bytes: usize,
}

pub fn recover(
    database: &[u8],
    wal: &[u8],
    max_image_bytes: usize,
    max_frames: usize,
) -> Result<Recovery> {
    slice(wal, 0, 32)?;
    let little = match u32be(wal, 0)? {
        0x377f0682 => true,
        0x377f0683 => false,
        _ => return Err(Error::Corrupt("WAL signature")),
    };
    if u32be(wal, 4)? != 3007000 {
        return Err(Error::Unsupported("WAL format version"));
    }
    let page_size = usize::try_from(u32be(wal, 8)?).map_err(|_| Error::Limit("WAL page size"))?;
    if !(512..=65536).contains(&page_size) || !page_size.is_power_of_two() {
        return Err(Error::Corrupt("WAL page size"));
    }
    if database.len() % page_size != 0 {
        return Err(Error::Corrupt("database/WAL page size mismatch"));
    }
    if !database.is_empty() {
        if slice(database, 0, 16)? != b"SQLite format 3\0" {
            return Err(Error::InvalidHeader("signature"));
        }
        let encoded = crate::error::u16be(database, 16)?;
        let actual = if encoded == 1 {
            65536
        } else {
            usize::from(encoded)
        };
        if actual != page_size {
            return Err(Error::Corrupt("database/WAL page size mismatch"));
        }
    }
    let mut sum = checksum(&wal[..24], little, (0, 0))?;
    if sum != (u32be(wal, 24)?, u32be(wal, 28)?) {
        return Err(Error::Corrupt("WAL header checksum"));
    }
    let frame_size = page_size + 24;
    let mut frames = Vec::new();
    let mut position = 32usize;
    let mut committed = 0;
    let mut committed_pages = 0u32;
    while wal.len() - position >= frame_size {
        if frames.len() >= max_frames {
            return Err(Error::Limit("WAL frame count"));
        }
        let frame = &wal[position..position + frame_size];
        let page = u32be(frame, 0)?;
        if page == 0 || frame[8..16] != wal[16..24] {
            break;
        }
        let candidate = checksum(&frame[..8], little, sum)?;
        let candidate = checksum(&frame[24..], little, candidate)?;
        if candidate != (u32be(frame, 16)?, u32be(frame, 20)?) {
            break;
        }
        sum = candidate;
        frames
            .try_reserve(1)
            .map_err(|_| Error::Limit("allocation"))?;
        frames.push((page, position + 24));
        let pages = u32be(frame, 4)?;
        if pages != 0 {
            committed = frames.len();
            committed_pages = pages;
        }
        position += frame_size;
    }
    let size = if committed == 0 {
        database.len()
    } else {
        usize::try_from(committed_pages)
            .ok()
            .and_then(|n| n.checked_mul(page_size))
            .ok_or(Error::Limit("recovered image size"))?
    };
    if size > max_image_bytes {
        return Err(Error::Limit("recovered image size"));
    }
    let mut image = Vec::new();
    image
        .try_reserve_exact(size)
        .map_err(|_| Error::Limit("allocation"))?;
    image.extend_from_slice(&database[..database.len().min(size)]);
    image.resize(size, 0);
    for &(page, at) in &frames[..committed] {
        if page > committed_pages {
            continue;
        }
        let start = usize::try_from(page - 1)
            .ok()
            .and_then(|n| n.checked_mul(page_size))
            .ok_or(Error::Limit("WAL page offset"))?;
        image[start..start + page_size].copy_from_slice(&wal[at..at + page_size]);
    }
    Ok(Recovery {
        image,
        valid_frames: frames.len(),
        committed_frames: committed,
        ignored_tail_bytes: wal.len() - (32 + committed * frame_size),
    })
}

fn checksum(bytes: &[u8], little: bool, (mut a, mut b): (u32, u32)) -> Result<(u32, u32)> {
    if bytes.len() % 8 != 0 {
        return Err(Error::Corrupt("WAL checksum input"));
    }
    for chunk in bytes.chunks_exact(8) {
        let x: [u8; 4] = chunk[..4].try_into().map_err(|_| Error::Truncated)?;
        let y: [u8; 4] = chunk[4..].try_into().map_err(|_| Error::Truncated)?;
        let (x, y) = if little {
            (u32::from_le_bytes(x), u32::from_le_bytes(y))
        } else {
            (u32::from_be_bytes(x), u32::from_be_bytes(y))
        };
        a = a.wrapping_add(x).wrapping_add(b);
        b = b.wrapping_add(y).wrapping_add(a);
    }
    Ok((a, b))
}
