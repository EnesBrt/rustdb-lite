//! Iterative LIKE/GLOB matching over SQLite's NUL-terminated UTF-8 characters.
//! The matcher keeps only positions and one wildcard retry, never a call stack
//! or a matrix proportional to the input lengths. Every scan consumes fuel.
use super::*;
use alloc::borrow::Cow;

const MAX_PATTERN_BYTES: usize = 50_000;

fn bytes<'a>(
    value: &'a Value,
    encoding: Encoding,
    fuel: &mut Fuel,
    max_bytes: usize,
) -> Result<Cow<'a, [u8]>> {
    let (input, encoding) = match value {
        Value::Text(t) => (&t.bytes, t.encoding),
        Value::Blob(b) => (b, encoding),
        _ => return Ok(Cow::Owned(scalar::text_bytes(value)?)),
    };
    if encoding == Encoding::Utf8 {
        return Ok(Cow::Borrowed(input));
    }
    // BLOB-to-text conversion uses the database encoding. Native UTF-16
    // conversion ignores a trailing byte and combines a surrogate with the
    // following code unit even if that unit is not a low surrogate.
    let mut units = input.chunks_exact(2).map(|b| {
        u32::from(if encoding == Encoding::Utf16Le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    });
    let mut output = Vec::new();
    while let Some(mut c) = units.next() {
        fuel.spend()?;
        if (0xd800..0xe000).contains(&c) {
            if let Some(second) = units.next() {
                c = ((c & 0x3ff) << 10) + (second & 0x3ff) + 0x10000;
            }
        }
        let (encoded, count) = if c < 0x80 {
            ([c as u8, 0, 0, 0], 1)
        } else if c < 0x800 {
            ([0xc0 | (c >> 6) as u8, 0x80 | (c & 0x3f) as u8, 0, 0], 2)
        } else if c < 0x10000 {
            (
                [
                    0xe0 | (c >> 12) as u8,
                    0x80 | ((c >> 6) & 0x3f) as u8,
                    0x80 | (c & 0x3f) as u8,
                    0,
                ],
                3,
            )
        } else {
            (
                [
                    0xf0 | (c >> 18) as u8,
                    0x80 | ((c >> 12) & 0x3f) as u8,
                    0x80 | ((c >> 6) & 0x3f) as u8,
                    0x80 | (c & 0x3f) as u8,
                ],
                4,
            )
        };
        if output.len().saturating_add(count) > max_bytes {
            return Err(Error::Limit("pattern text bytes"));
        }
        output.extend_from_slice(&encoded[..count]);
    }
    Ok(Cow::Owned(output))
}

// Native pattern functions accept BLOBs and malformed UTF-8. Match their
// character boundaries and replacement rules without unchecked byte reads.
fn read(input: &[u8], at: &mut usize, fuel: &mut Fuel) -> Result<Option<u32>> {
    let Some(&first) = input.get(*at).filter(|b| **b != 0) else {
        return Ok(None);
    };
    fuel.spend()?;
    *at += 1;
    let mut c = u32::from(first);
    if first >= 0xc0 {
        c &= match first {
            0xc0..=0xdf => 0x1f,
            0xe0..=0xef => 0x0f,
            0xf0..=0xf7 => 0x07,
            0xf8..=0xfb => 0x03,
            0xfc..=0xfd => 0x01,
            _ => 0,
        };
        while let Some(&b) = input.get(*at).filter(|b| **b & 0xc0 == 0x80) {
            fuel.spend()?;
            c = c.wrapping_shl(6).wrapping_add(u32::from(b & 0x3f));
            *at += 1;
        }
        if c < 0x80 || c & 0xffff_f800 == 0xd800 || c & 0xffff_fffe == 0xfffe {
            c = 0xfffd;
        }
    }
    Ok(Some(c))
}

fn equal(a: u32, b: u32, glob: bool) -> bool {
    a == b || (!glob && a < 128 && b < 128 && (a as u8).eq_ignore_ascii_case(&(b as u8)))
}

fn class(pattern: &[u8], at: &mut usize, value: u32, fuel: &mut Fuel) -> Result<bool> {
    let mut c = read(pattern, at, fuel)?;
    let invert = c == Some(u32::from(b'^'));
    if invert {
        c = read(pattern, at, fuel)?;
    }
    let mut seen = false;
    if c == Some(u32::from(b']')) {
        seen = value == u32::from(b']');
        c = read(pattern, at, fuel)?;
    }
    let mut prior = 0;
    while let Some(current) = c {
        if current == u32::from(b']') {
            return Ok(seen ^ invert);
        }
        if current == u32::from(b'-')
            && pattern.get(*at).is_some_and(|b| *b != b']' && *b != 0)
            && prior > 0
        {
            let end = read(pattern, at, fuel)?.ok_or(Error::Corrupt("pattern range"))?;
            seen |= value >= prior && value <= end;
            prior = 0;
        } else {
            seen |= value == current;
            prior = current;
        }
        c = read(pattern, at, fuel)?;
    }
    Ok(false)
}

fn matches(
    pattern: &[u8],
    text: &[u8],
    glob: bool,
    escape: Option<u32>,
    fuel: &mut Fuel,
) -> Result<bool> {
    let all = u32::from(if glob { b'*' } else { b'%' });
    let one = u32::from(if glob { b'?' } else { b'_' });
    let (mut p, mut t) = (0, 0);
    let mut retry = None;
    loop {
        fuel.spend()?;
        let code = read(pattern, &mut p, fuel)?;
        let accepted = if let Some(c) = code {
            if c == all && Some(c) != escape {
                retry = Some((p, t));
                continue;
            }
            let value = read(text, &mut t, fuel)?;
            if Some(c) == escape {
                match (read(pattern, &mut p, fuel)?, value) {
                    (Some(a), Some(b)) => equal(a, b, glob),
                    _ => false,
                }
            } else if glob && c == u32::from(b'[') {
                if let Some(value) = value {
                    class(pattern, &mut p, value, fuel)?
                } else {
                    false
                }
            } else {
                value.is_some_and(|v| c == one || equal(c, v, glob))
            }
        } else {
            if text.get(t).is_none_or(|b| *b == 0) {
                return Ok(true);
            }
            false
        };
        if !accepted {
            let Some((next, mut consumed)) = retry else {
                return Ok(false);
            };
            if read(text, &mut consumed, fuel)?.is_none() {
                return Ok(false);
            }
            retry = Some((next, consumed));
            p = next;
            t = consumed;
        }
    }
}

impl Eval<'_> {
    pub(super) fn pattern(
        &mut self,
        name: &str,
        pattern: &Value,
        text: &Value,
        escape: Option<&Value>,
    ) -> Result<Value> {
        let p = bytes(
            pattern,
            self.encoding,
            self.fuel,
            self.limits.max_database_bytes,
        )?;
        // The native length check precedes ESCAPE validation and NULL results,
        // and includes bytes after an embedded NUL in the pattern.
        let length = if let Value::Blob(b) = pattern {
            b.len()
        } else {
            p.len()
        };
        if length > MAX_PATTERN_BYTES {
            return Err(Error::Limit("LIKE or GLOB pattern bytes"));
        }
        let escape = if let Some(value) = escape {
            if scalar::null(value) {
                return Ok(Value::Null);
            }
            let bytes = bytes(
                value,
                self.encoding,
                self.fuel,
                self.limits.max_database_bytes,
            )?;
            let mut at = 0;
            let first = read(&bytes, &mut at, self.fuel)?;
            if first.is_none() || read(&bytes, &mut at, self.fuel)?.is_some() {
                return Err(error("ESCAPE expression must be a single character"));
            }
            first
        } else {
            None
        };
        if scalar::null(pattern) || scalar::null(text) {
            return Ok(Value::Null);
        }
        let t = bytes(
            text,
            self.encoding,
            self.fuel,
            self.limits.max_database_bytes,
        )?;
        Ok(boolean(Some(matches(
            &p,
            &t,
            name == "glob",
            escape,
            self.fuel,
        )?)))
    }
}
