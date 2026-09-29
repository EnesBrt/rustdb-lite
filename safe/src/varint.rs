//! SQLite's big-endian, one-to-nine-byte varints (not LEB128).
use crate::{Error, Result};
use alloc::vec::Vec;

pub fn decode(input: &[u8]) -> Result<(u64, usize)> {
    let mut value = 0u64;
    for i in 0..8 {
        let byte = *input.get(i).ok_or(Error::Truncated)?;
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }
    }
    Ok((
        (value << 8) | u64::from(*input.get(8).ok_or(Error::Truncated)?),
        9,
    ))
}

pub fn encoded_len(value: u64) -> usize {
    if value > 0x00ff_ffff_ffff_ffff {
        9
    } else {
        ((64 - value.leading_zeros()).max(1) as usize).div_ceil(7)
    }
}

pub fn encode(mut value: u64, output: &mut Vec<u8>) -> Result<()> {
    let count = encoded_len(value);
    let mut bytes = [0u8; 9];
    if count == 9 {
        bytes[8] = value as u8;
        value >>= 8;
        for i in (0..8).rev() {
            bytes[i] = (value as u8 & 0x7f) | 0x80;
            value >>= 7;
        }
    } else {
        for i in (0..count).rev() {
            bytes[i] = value as u8 & 0x7f;
            if i + 1 != count {
                bytes[i] |= 0x80;
            }
            value >>= 7;
        }
    }
    output
        .try_reserve(count)
        .map_err(|_| Error::Limit("allocation"))?;
    output.extend_from_slice(&bytes[..count]);
    Ok(())
}
