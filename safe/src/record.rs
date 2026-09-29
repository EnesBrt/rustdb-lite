use crate::error::slice;
use crate::{varint, Encoding, Error, Result};
use alloc::{string::String, vec::Vec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    pub bytes: Vec<u8>,
    pub encoding: Encoding,
}
impl Text {
    pub fn utf8(text: &str) -> Self {
        Self {
            bytes: text.as_bytes().to_vec(),
            encoding: Encoding::Utf8,
        }
    }
    /// Convert valid text while preserving bytes when the encoding already matches.
    pub fn transcode(&self, encoding: Encoding) -> Result<Self> {
        if self.encoding == encoding {
            return Ok(self.clone());
        }
        let string = self.to_string()?;
        let bytes = match encoding {
            Encoding::Utf8 => string.into_bytes(),
            Encoding::Utf16Le => string.encode_utf16().flat_map(u16::to_le_bytes).collect(),
            Encoding::Utf16Be => string.encode_utf16().flat_map(u16::to_be_bytes).collect(),
        };
        Ok(Self { bytes, encoding })
    }
    pub fn to_string(&self) -> Result<String> {
        match self.encoding {
            Encoding::Utf8 => String::from_utf8(self.bytes.clone()).map_err(|_| Error::InvalidText),
            encoding => {
                if self.bytes.len() % 2 != 0 {
                    return Err(Error::InvalidText);
                }
                let units = self.bytes.chunks_exact(2).map(|b| match encoding {
                    Encoding::Utf16Le => u16::from_le_bytes([b[0], b[1]]),
                    _ => u16::from_be_bytes([b[0], b[1]]),
                });
                core::char::decode_utf16(units)
                    .collect::<core::result::Result<String, _>>()
                    .map_err(|_| Error::InvalidText)
            }
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(Text),
    Blob(Vec<u8>),
}

pub fn decode(bytes: &[u8], encoding: Encoding, max_columns: usize) -> Result<Vec<Value>> {
    let (header_len, mut header_pos) = varint::decode(bytes)?;
    let header_len =
        usize::try_from(header_len).map_err(|_| Error::Corrupt("record header size"))?;
    if header_len < header_pos || header_len > bytes.len() {
        return Err(Error::Corrupt("record header size"));
    }
    let mut body_pos = header_len;
    let mut values = Vec::new();
    while header_pos < header_len {
        if values.len() >= max_columns {
            return Err(Error::Limit("record columns"));
        }
        let (serial, count) = varint::decode(&bytes[header_pos..header_len])?;
        header_pos += count;
        let size = match serial {
            0 | 8 | 9 => 0,
            1..=4 => serial,
            5 => 6,
            6 | 7 => 8,
            10 | 11 => return Err(Error::Corrupt("reserved serial type")),
            _ => (serial - 12) / 2,
        };
        let size = usize::try_from(size).map_err(|_| Error::Limit("record value size"))?;
        let body = slice(bytes, body_pos, size)?;
        body_pos += size;
        let value = match serial {
            0 => Value::Null,
            1..=6 => {
                let mut integer = if body[0] & 0x80 == 0 { 0u64 } else { u64::MAX };
                for &byte in body {
                    integer = (integer << 8) | u64::from(byte);
                }
                Value::Integer(integer as i64)
            }
            7 => Value::Real(f64::from_bits(u64::from_be_bytes(
                body.try_into().map_err(|_| Error::Truncated)?,
            ))),
            8 => Value::Integer(0),
            9 => Value::Integer(1),
            n if n % 2 == 0 => Value::Blob(copy(body)?),
            _ => Value::Text(Text {
                bytes: copy(body)?,
                encoding,
            }),
        };
        values
            .try_reserve(1)
            .map_err(|_| Error::Limit("allocation"))?;
        values.push(value);
    }
    if body_pos != bytes.len() {
        return Err(Error::Corrupt("trailing record bytes"));
    }
    Ok(values)
}

fn copy(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(bytes.len())
        .map_err(|_| Error::Limit("allocation"))?;
    result.extend_from_slice(bytes);
    Ok(result)
}

/// Encode schema-format-4 records. Text bytes retain their declared encoding;
/// mixing encodings in a database must be rejected by the caller.
pub fn encode(values: &[Value], max_bytes: usize) -> Result<Vec<u8>> {
    let mut serials = Vec::new();
    let mut body = Vec::new();
    for value in values {
        let (serial, data): (u64, Vec<u8>) = match value {
            Value::Null => (0, Vec::new()),
            Value::Integer(0) => (8, Vec::new()),
            Value::Integer(1) => (9, Vec::new()),
            Value::Integer(n) => {
                let (serial, size) = if (-128..=127).contains(n) {
                    (1, 1)
                } else if (-32768..=32767).contains(n) {
                    (2, 2)
                } else if (-8388608..=8388607).contains(n) {
                    (3, 3)
                } else if (i64::from(i32::MIN)..=i64::from(i32::MAX)).contains(n) {
                    (4, 4)
                } else if (-140737488355328..=140737488355327).contains(n) {
                    (5, 6)
                } else {
                    (6, 8)
                };
                (serial, n.to_be_bytes()[8 - size..].to_vec())
            }
            Value::Real(n) => (7, n.to_bits().to_be_bytes().to_vec()),
            Value::Blob(bytes) => {
                if bytes.len() > max_bytes {
                    return Err(Error::Limit("record size"));
                }
                (
                    u64::try_from(bytes.len())
                        .ok()
                        .and_then(|n| n.checked_mul(2))
                        .and_then(|n| n.checked_add(12))
                        .ok_or(Error::Limit("record size"))?,
                    copy(bytes)?,
                )
            }
            Value::Text(text) => {
                if text.bytes.len() > max_bytes {
                    return Err(Error::Limit("record size"));
                }
                (
                    u64::try_from(text.bytes.len())
                        .ok()
                        .and_then(|n| n.checked_mul(2))
                        .and_then(|n| n.checked_add(13))
                        .ok_or(Error::Limit("record size"))?,
                    copy(&text.bytes)?,
                )
            }
        };
        varint::encode(serial, &mut serials)?;
        let new_size = body
            .len()
            .checked_add(data.len())
            .and_then(|n| n.checked_add(serials.len()))
            .ok_or(Error::Limit("record size"))?;
        if new_size > max_bytes {
            return Err(Error::Limit("record size"));
        }
        body.try_reserve(data.len())
            .map_err(|_| Error::Limit("allocation"))?;
        body.extend_from_slice(&data);
    }
    let mut header_len = serials
        .len()
        .checked_add(1)
        .ok_or(Error::Limit("record size"))?;
    loop {
        let next = serials
            .len()
            .checked_add(varint::encoded_len(header_len as u64))
            .ok_or(Error::Limit("record size"))?;
        if next == header_len {
            break;
        }
        header_len = next;
    }
    let total = header_len
        .checked_add(body.len())
        .ok_or(Error::Limit("record size"))?;
    if total > max_bytes {
        return Err(Error::Limit("record size"));
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(total)
        .map_err(|_| Error::Limit("allocation"))?;
    varint::encode(header_len as u64, &mut output)?;
    output.extend_from_slice(&serials);
    output.extend_from_slice(&body);
    Ok(output)
}
