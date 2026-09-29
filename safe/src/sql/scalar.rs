use super::{error, parser::Binary};
use crate::{Encoding, Error, Result, Text, Value};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use core::cmp::Ordering;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Affinity {
    None,
    Blob,
    Text,
    Integer,
    Real,
    Numeric,
    /// Numeric affinity that preserves existing INTEGER/REAL storage classes.
    FlexNumeric,
}
impl Affinity {
    pub fn from_type(name: &str) -> Self {
        let name = name.to_ascii_uppercase();
        if name.contains("INT") {
            Self::Integer
        } else if name.contains("CHAR") || name.contains("CLOB") || name.contains("TEXT") {
            Self::Text
        } else if name.is_empty() || name.contains("BLOB") {
            Self::Blob
        } else if name.contains("REAL") || name.contains("FLOA") || name.contains("DOUB") {
            Self::Real
        } else {
            Self::Numeric
        }
    }
    pub fn numeric(self) -> bool {
        matches!(
            self,
            Self::Integer | Self::Real | Self::Numeric | Self::FlexNumeric
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Collation {
    Binary,
    NoCase,
    RTrim,
}
impl Collation {
    pub fn parse(name: &str) -> Result<Self> {
        if name.eq_ignore_ascii_case("BINARY") {
            Ok(Self::Binary)
        } else if name.eq_ignore_ascii_case("NOCASE") {
            Ok(Self::NoCase)
        } else if name.eq_ignore_ascii_case("RTRIM") {
            Ok(Self::RTrim)
        } else {
            Err(error(format!("no such collation: {name}")))
        }
    }
}
pub fn null(v: &Value) -> bool {
    matches!(v, Value::Null)
}
pub fn real(n: f64) -> Value {
    if n.is_nan() {
        Value::Null
    } else {
        Value::Real(n)
    }
}
pub fn float(v: &Value) -> f64 {
    match v {
        Value::Integer(n) => *n as f64,
        Value::Real(n) => *n,
        _ => 0.0,
    }
}
pub fn size(v: &Value) -> usize {
    match v {
        Value::Text(t) => t.bytes.len(),
        Value::Blob(b) => b.len(),
        _ => 8,
    }
}
pub fn text_bytes(v: &Value) -> Result<Vec<u8>> {
    match v {
        Value::Text(t) if t.encoding == Encoding::Utf8 => Ok(t.bytes.clone()),
        Value::Text(t) => Ok(t.to_string()?.into_bytes()),
        Value::Blob(b) => Ok(b.clone()),
        Value::Null => Ok(Vec::new()),
        Value::Integer(n) => Ok(n.to_string().into_bytes()),
        Value::Real(n) => Ok(float_text(*n).into_bytes()),
    }
}
pub fn text(v: &Value) -> Result<String> {
    String::from_utf8(text_bytes(v)?).map_err(|_| Error::InvalidText)
}
pub fn float_text(n: f64) -> String {
    if n == 0.0 {
        return "0.0".into();
    }
    if n.is_infinite() {
        return if n.is_sign_negative() { "-Inf" } else { "Inf" }.into();
    }
    if n.is_nan() {
        return "NaN".into();
    }
    // Match the pinned engine's default 17-digit conversion and its reduction
    // of long runs of 9s/0s when the shorter value round-trips. Keep decimal
    // rounding explicit rather than using Rust's shortest Display convention.
    let formatted = format!("{:.17e}", n.abs());
    let (mantissa, exp) = formatted.split_once('e').unwrap_or((&formatted, "0"));
    let mut exponent = exp.parse::<i32>().unwrap_or(0);
    let negative = n.is_sign_negative();
    let mut digits = mantissa.replace('.', "").into_bytes();
    let mut precision = 17;
    let mut candidate = None;
    if digits[15] == b'9' && digits[14] == b'9' {
        let mut first = 14;
        while first > 0 && digits[first - 1] == b'9' {
            first -= 1;
        }
        let mut value = 0u64;
        for digit in &digits[..first] {
            value = value * 10 + u64::from(digit - b'0');
        }
        candidate = Some((first, value + 1));
    } else if exponent + 1 >= 18 || (digits[15] == b'0' && digits[14] == b'0' && digits[13] == b'0')
    {
        let mut first = 13;
        while first > 0 && digits[first - 1] == b'0' {
            first -= 1;
        }
        let mut value = 0u64;
        for digit in &digits[..first] {
            value = value * 10 + u64::from(digit - b'0');
        }
        candidate = Some((first, value));
    }
    if let Some((first, value)) = candidate {
        let decimal = format!("{value}e{}", exponent + 1 - first as i32);
        if decimal.parse::<f64>().ok() == Some(n.abs()) {
            precision = first + 1;
        }
    }
    let round_up = digits[precision] >= b'5';
    digits.truncate(precision);
    if round_up {
        let mut at = digits.len();
        loop {
            if at == 0 {
                digits.insert(0, b'1');
                exponent += 1;
                break;
            }
            at -= 1;
            if digits[at] != b'9' {
                digits[at] += 1;
                break;
            }
            digits[at] = b'0';
        }
    }
    while digits.len() > 1 && digits.last() == Some(&b'0') {
        digits.pop();
    }
    let digits = String::from_utf8(digits).unwrap_or_default();
    let sign = if negative { "-" } else { "" };
    if !(-4..17).contains(&exponent) {
        let rest = &digits[1..];
        format!(
            "{sign}{}.{}e{}{:02}",
            &digits[..1],
            if rest.is_empty() { "0" } else { rest },
            if exponent < 0 { "-" } else { "+" },
            exponent.unsigned_abs()
        )
    } else if exponent < 0 {
        format!("{sign}0.{}{digits}", "0".repeat((-exponent - 1) as usize))
    } else {
        let split = exponent as usize + 1;
        if split >= digits.len() {
            format!("{sign}{digits}{}.0", "0".repeat(split - digits.len()))
        } else {
            format!("{sign}{}.{}", &digits[..split], &digits[split..])
        }
    }
}
fn prefix(bytes: &[u8]) -> (usize, usize, bool) {
    let mut at = 0;
    while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    let start = at;
    if matches!(bytes.get(at), Some(b'+' | b'-')) {
        at += 1;
    }
    let mut digits = 0;
    while bytes.get(at).is_some_and(u8::is_ascii_digit) {
        at += 1;
        digits += 1;
    }
    let mut floating = false;
    if bytes.get(at) == Some(&b'.') {
        floating = true;
        at += 1;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return (start, start, false);
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        let mut end = at + 1;
        if matches!(bytes.get(end), Some(b'+' | b'-')) {
            end += 1;
        }
        let first = end;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        if end > first {
            at = end;
            floating = true;
        }
    }
    (start, at, floating)
}
fn parse_numeric(bytes: &[u8], full: bool, integerize: bool) -> Option<Value> {
    let (start, end, floating) = prefix(bytes);
    if end == start {
        return None;
    }
    if full && bytes[end..].iter().any(|b| !b.is_ascii_whitespace()) {
        return None;
    }
    let s = core::str::from_utf8(&bytes[start..end]).ok()?;
    if !floating {
        if let Ok(n) = s.parse::<i64>() {
            return Some(Value::Integer(n));
        }
    }
    let n = s.parse::<f64>().ok()?;
    if integerize && n >= i64::MIN as f64 && n < 9223372036854775808.0 && n == (n as i64) as f64 {
        return Some(Value::Integer(n as i64));
    }
    Some(real(n))
}
pub fn numeric(v: &Value) -> Result<Value> {
    match v {
        Value::Text(_) | Value::Blob(_) => {
            Ok(parse_numeric(&text_bytes(v)?, false, false).unwrap_or(Value::Integer(0)))
        }
        _ => Ok(v.clone()),
    }
}
pub fn numeric_type(v: &Value) -> Result<Value> {
    if matches!(v, Value::Text(_)) {
        Ok(parse_numeric(&text_bytes(v)?, true, false).unwrap_or_else(|| v.clone()))
    } else {
        Ok(v.clone())
    }
}
pub fn integer(v: &Value) -> Result<i64> {
    match v {
        Value::Integer(n) => Ok(*n),
        Value::Real(n) => Ok(*n as i64),
        Value::Null => Ok(0),
        _ => {
            let bytes = text_bytes(v)?;
            let mut at = 0;
            while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
                at += 1;
            }
            let negative = bytes.get(at) == Some(&b'-');
            if negative || bytes.get(at) == Some(&b'+') {
                at += 1;
            }
            let mut n = 0u64;
            while let Some(b'0'..=b'9') = bytes.get(at) {
                n = n
                    .saturating_mul(10)
                    .saturating_add(u64::from(bytes[at] - b'0'));
                at += 1;
            }
            if negative {
                if n >= 1u64 << 63 {
                    Ok(i64::MIN)
                } else {
                    Ok(-(n as i64))
                }
            } else {
                Ok(n.min(i64::MAX as u64) as i64)
            }
        }
    }
}
pub fn truth(v: &Value) -> Result<Option<bool>> {
    if null(v) {
        Ok(None)
    } else {
        Ok(Some(float(&numeric(v)?) != 0.0))
    }
}
pub fn affinity(v: Value, a: Affinity) -> Result<Value> {
    match a {
        Affinity::Text => match v {
            Value::Integer(_) | Value::Real(_) => Ok(Value::Text(Text {
                bytes: text_bytes(&v)?,
                encoding: Encoding::Utf8,
            })),
            _ => Ok(v),
        },
        a if a.numeric() => {
            let v = match &v {
                Value::Text(_) => parse_numeric(&text_bytes(&v)?, true, true).unwrap_or(v),
                _ => v,
            };
            if a == Affinity::Real {
                if let Value::Integer(n) = v {
                    return Ok(Value::Real(n as f64));
                }
                // INTEGER/REAL storage compaction removes the sign of zero
                // in a REAL-affinity column; expression REAL values keep it.
                if matches!(v, Value::Real(n) if n == 0.0) {
                    return Ok(Value::Real(0.0));
                }
            } else if a != Affinity::FlexNumeric {
                if let Value::Real(n) = v {
                    if n >= i64::MIN as f64 && n < 9223372036854775808.0 && n == (n as i64) as f64 {
                        return Ok(Value::Integer(n as i64));
                    }
                }
            }
            Ok(v)
        }
        _ => Ok(v),
    }
}
pub fn cast(v: Value, a: Affinity) -> Result<Value> {
    if null(&v) {
        return Ok(v);
    }
    match a {
        Affinity::Text => Ok(Value::Text(Text {
            bytes: text_bytes(&v)?,
            encoding: Encoding::Utf8,
        })),
        Affinity::Blob | Affinity::None => Ok(Value::Blob(text_bytes(&v)?)),
        Affinity::Integer => Ok(Value::Integer(integer(&v)?)),
        Affinity::Real => Ok(real(float(&numeric(&v)?))),
        Affinity::Numeric | Affinity::FlexNumeric => match v {
            Value::Text(_) | Value::Blob(_) => {
                let v = parse_numeric(&text_bytes(&v)?, false, false).unwrap_or(Value::Integer(0));
                Ok(match v {
                    Value::Real(n)
                        if (-2251799813685248.0..2251799813685248.0).contains(&n)
                            && n == (n as i64) as f64 =>
                    {
                        Value::Integer(n as i64)
                    }
                    _ => v,
                })
            }
            _ => Ok(v),
        },
    }
}
pub fn cast_encoded(v: Value, a: Affinity, encoding: Encoding) -> Result<Value> {
    if encoding == Encoding::Utf8 || null(&v) {
        return cast(v, a);
    }
    if a == Affinity::Blob {
        return match v {
            Value::Blob(_) => Ok(v),
            v => Ok(Value::Blob(
                Text::utf8(&text(&v)?).transcode(encoding)?.bytes,
            )),
        };
    }
    let v = if let Value::Blob(bytes) = v {
        Value::Text(Text { bytes, encoding }.transcode(Encoding::Utf8)?)
    } else {
        v
    };
    cast(v, a)
}
fn int_real(i: i64, r: f64) -> Ordering {
    if r < i64::MIN as f64 {
        return Ordering::Greater;
    }
    if r >= 9223372036854775808.0 {
        return Ordering::Less;
    }
    let cmp = i.cmp(&(r as i64));
    if cmp != Ordering::Equal {
        return cmp;
    }
    (i as f64).partial_cmp(&r).unwrap_or(Ordering::Equal)
}
pub fn compare(a: &Value, b: &Value, c: Collation) -> Result<Ordering> {
    let rank = |v: &Value| match v {
        Value::Null => 0,
        Value::Integer(_) | Value::Real(_) => 1,
        Value::Text(_) => 2,
        Value::Blob(_) => 3,
    };
    let ranks = rank(a).cmp(&rank(b));
    if ranks != Ordering::Equal {
        return Ok(ranks);
    }
    Ok(match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Integer(a), Value::Integer(b)) => a.cmp(b),
        (Value::Integer(a), Value::Real(b)) => int_real(*a, *b),
        (Value::Real(a), Value::Integer(b)) => int_real(*b, *a).reverse(),
        (Value::Real(a), Value::Real(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
        (Value::Blob(a), Value::Blob(b)) => a.cmp(b),
        (Value::Text(_), Value::Text(_)) => {
            let a = text_bytes(a)?;
            let b = text_bytes(b)?;
            match c {
                Collation::Binary => a.cmp(&b),
                Collation::RTrim => trim_spaces(&a).cmp(trim_spaces(&b)),
                Collation::NoCase => {
                    let aa = a
                        .iter()
                        .take_while(|x| **x != 0)
                        .map(u8::to_ascii_lowercase);
                    let bb = b
                        .iter()
                        .take_while(|x| **x != 0)
                        .map(u8::to_ascii_lowercase);
                    let cmp = aa.cmp(bb);
                    if cmp == Ordering::Equal {
                        a.len().cmp(&b.len())
                    } else {
                        cmp
                    }
                }
            }
        }
        _ => Ordering::Equal,
    })
}
pub fn compare_encoded(a: &Value, b: &Value, c: Collation, encoding: Encoding) -> Result<Ordering> {
    if c == Collation::Binary && encoding != Encoding::Utf8 {
        if let (Value::Text(a), Value::Text(b)) = (a, b) {
            return Ok(a
                .transcode(encoding)?
                .bytes
                .cmp(&b.transcode(encoding)?.bytes));
        }
    }
    compare(a, b, c)
}
fn trim_spaces(mut b: &[u8]) -> &[u8] {
    while b.last() == Some(&b' ') {
        b = &b[..b.len() - 1];
    }
    b
}
pub fn compare_affinity(
    a: Value,
    b: Value,
    aa: Affinity,
    ba: Affinity,
    c: Collation,
    encoding: Encoding,
) -> Result<Ordering> {
    let mut comparison = comparison_affinity(aa, ba);
    if comparison.numeric() {
        comparison = Affinity::Numeric;
    }
    // SQLite's scalar comparison opcodes only stringify if one operand is
    // already text. Two numeric values continue to compare numerically.
    if comparison == Affinity::Text && !matches!(a, Value::Text(_)) && !matches!(b, Value::Text(_))
    {
        comparison = Affinity::None;
    }
    let a = affinity(a, comparison)?;
    let b = affinity(b, comparison)?;
    compare_encoded(&a, &b, c, encoding)
}
pub fn comparison_affinity(aa: Affinity, ba: Affinity) -> Affinity {
    if aa == Affinity::None {
        ba
    } else if ba == Affinity::None {
        aa
    } else if aa.numeric() || ba.numeric() {
        Affinity::Numeric
    } else {
        Affinity::None
    }
}
pub fn arithmetic(op: Binary, a: Value, b: Value, max_bytes: usize) -> Result<Value> {
    if null(&a) || null(&b) {
        return Ok(Value::Null);
    }
    if op == Binary::Concat {
        let mut a = text_bytes(&a)?;
        let b = text_bytes(&b)?;
        if a.len().checked_add(b.len()).is_none_or(|n| n > max_bytes) {
            return Err(Error::Limit("SQL value size"));
        }
        a.extend_from_slice(&b);
        return Ok(Value::Text(Text {
            bytes: a,
            encoding: Encoding::Utf8,
        }));
    }
    let a = numeric(&a)?;
    let b = numeric(&b)?;
    if matches!(
        op,
        Binary::BitAnd | Binary::BitOr | Binary::ShiftLeft | Binary::ShiftRight
    ) {
        let i = integer(&a)?;
        let j = integer(&b)?;
        let n = match op {
            Binary::BitAnd => i & j,
            Binary::BitOr => i | j,
            _ => {
                let left = (op == Binary::ShiftLeft) ^ (j < 0);
                let distance = j.unsigned_abs();
                if distance >= 64 {
                    if !left && i < 0 {
                        -1
                    } else {
                        0
                    }
                } else if left {
                    i.wrapping_shl(distance as u32)
                } else {
                    i >> distance
                }
            }
        };
        return Ok(Value::Integer(n));
    }
    if op == Binary::Remainder {
        let i = integer(&a)?;
        let j = integer(&b)?;
        if j == 0 {
            return Ok(Value::Null);
        }
        let n = if j == -1 { 0 } else { i % j };
        return Ok(
            if matches!(a, Value::Real(_)) || matches!(b, Value::Real(_)) {
                Value::Real(n as f64)
            } else {
                Value::Integer(n)
            },
        );
    }
    if let (Value::Integer(i), Value::Integer(j)) = (&a, &b) {
        let n = match op {
            Binary::Add => i.checked_add(*j),
            Binary::Subtract => i.checked_sub(*j),
            Binary::Multiply => i.checked_mul(*j),
            Binary::Divide => {
                if *j == 0 {
                    return Ok(Value::Null);
                }
                i.checked_div(*j)
            }
            _ => None,
        };
        if let Some(n) = n {
            return Ok(Value::Integer(n));
        }
    }
    let (a, b) = (float(&a), float(&b));
    Ok(real(match op {
        Binary::Add => a + b,
        Binary::Subtract => a - b,
        Binary::Multiply => a * b,
        Binary::Divide => {
            if b == 0.0 {
                return Ok(Value::Null);
            }
            a / b
        }
        _ => return Err(error("invalid arithmetic operator")),
    }))
}
