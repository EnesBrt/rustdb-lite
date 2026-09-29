use super::{error, SqlLimits};
use crate::{Error, Result};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Word(String, bool),
    String(String),
    Blob(Vec<u8>),
    Number(String),
    Parameter(String),
    Symbol(&'static str),
    End,
}
#[derive(Clone, Debug)]
pub struct Token {
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
}
fn id(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 128
}
fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
pub fn lex(sql: &str, limits: SqlLimits) -> Result<Vec<Token>> {
    if sql.len() > limits.max_sql_bytes {
        return Err(Error::Limit("SQL bytes"));
    }
    if sql.contains('\0') {
        return Err(error("NUL in SQL source"));
    }
    let b = sql.as_bytes();
    let mut at = 0;
    let mut tokens = Vec::new();
    while at < b.len() {
        if b[at].is_ascii_whitespace() {
            at += 1;
            continue;
        }
        if b[at..].starts_with(&[0xef, 0xbb, 0xbf]) {
            at += 3;
            continue;
        }
        if b[at..].starts_with(b"--") {
            at += 2;
            while at < b.len() && b[at] != b'\n' {
                at += 1;
            }
            continue;
        }
        if b[at..].starts_with(b"/*") {
            at += 2;
            while at < b.len() && !b[at..].starts_with(b"*/") {
                at += 1;
            }
            at = (at + 2).min(b.len());
            continue;
        }
        if tokens.len() >= limits.max_tokens {
            return Err(Error::Limit("SQL tokens"));
        }
        let start = at;
        let kind = match b[at] {
            b'\'' | b'"' | b'`' | b'[' => {
                let open = b[at];
                let close = if open == b'[' { b']' } else { open };
                at += 1;
                let mut bytes = Vec::new();
                loop {
                    if at == b.len() {
                        return Err(error(format!("unterminated quote at byte {start}")));
                    }
                    let c = b[at];
                    at += 1;
                    if c == close {
                        if open != b'[' && b.get(at) == Some(&close) {
                            bytes.push(close);
                            at += 1;
                        } else {
                            break;
                        }
                    } else {
                        bytes.push(c);
                    }
                }
                let value = String::from_utf8(bytes).map_err(|_| Error::InvalidText)?;
                if open == b'\'' {
                    Kind::String(value)
                } else {
                    Kind::Word(value, true)
                }
            }
            b'x' | b'X' if b.get(at + 1) == Some(&b'\'') => {
                at += 2;
                let mut value = Vec::new();
                while at < b.len() && b[at] != b'\'' {
                    let a = hex(b[at]).ok_or_else(|| error("invalid BLOB literal"))?;
                    let c = b
                        .get(at + 1)
                        .and_then(|b| hex(*b))
                        .ok_or_else(|| error("invalid BLOB literal"))?;
                    value.push((a << 4) | c);
                    at += 2;
                }
                if b.get(at) != Some(&b'\'') {
                    return Err(error("unterminated BLOB literal"));
                }
                at += 1;
                Kind::Blob(value)
            }
            b'0'..=b'9' | b'.'
                if b[at] != b'.' || b.get(at + 1).is_some_and(u8::is_ascii_digit) =>
            {
                let is_hex = b[at] == b'0' && matches!(b.get(at + 1), Some(b'x' | b'X'));
                if is_hex {
                    at += 2;
                    let first = at;
                    while at < b.len() && (hex(b[at]).is_some() || b[at] == b'_') {
                        at += 1;
                    }
                    if at == first {
                        return Err(error("invalid hexadecimal integer"));
                    }
                } else {
                    while at < b.len() && (b[at].is_ascii_digit() || b[at] == b'_') {
                        at += 1;
                    }
                    if b.get(at) == Some(&b'.') {
                        at += 1;
                        while at < b.len() && (b[at].is_ascii_digit() || b[at] == b'_') {
                            at += 1;
                        }
                    }
                    if matches!(b.get(at), Some(b'e' | b'E')) {
                        at += 1;
                        if matches!(b.get(at), Some(b'+' | b'-')) {
                            at += 1;
                        }
                        let first = at;
                        while at < b.len() && (b[at].is_ascii_digit() || b[at] == b'_') {
                            at += 1;
                        }
                        if at == first {
                            return Err(error("invalid exponent"));
                        }
                    }
                }
                for p in start..at {
                    if b[p] == b'_' {
                        let digit = |c: u8| {
                            if is_hex {
                                hex(c).is_some()
                            } else {
                                c.is_ascii_digit()
                            }
                        };
                        if p == start || p + 1 == at || !digit(b[p - 1]) || !digit(b[p + 1]) {
                            return Err(error("invalid numeric separator"));
                        }
                    }
                }
                if b.get(at).is_some_and(|c| id(*c)) {
                    return Err(error("invalid numeric literal suffix"));
                }
                Kind::Number(sql[start..at].replace('_', ""))
            }
            b'?' | b':' | b'@' | b'$' => {
                let prefix = b[at];
                at += 1;
                if prefix == b'?' {
                    while b.get(at).is_some_and(u8::is_ascii_digit) {
                        at += 1;
                    }
                } else {
                    let first = at;
                    while b.get(at).is_some_and(|c| id(*c)) {
                        at += 1;
                    }
                    if first == at {
                        return Err(error("empty parameter name"));
                    }
                    if b.get(at) == Some(&b':') || b.get(at) == Some(&b'(') {
                        return Err(Error::Unsupported("Tcl parameter suffix"));
                    }
                }
                Kind::Parameter(sql[start..at].to_string())
            }
            c if id(c) && !c.is_ascii_digit() => {
                at += 1;
                while b.get(at).is_some_and(|c| id(*c)) {
                    at += 1;
                }
                Kind::Word(sql[start..at].to_string(), false)
            }
            _ => {
                let mut found = None;
                for s in [
                    "->>", "||", "<<", ">>", "<=", ">=", "==", "!=", "<>", "->", "(", ")", ",",
                    ";", ".", "+", "-", "*", "/", "%", "=", "<", ">", "&", "|", "~",
                ] {
                    if b[at..].starts_with(s.as_bytes()) {
                        found = Some(s);
                        break;
                    }
                }
                let s = found.ok_or_else(|| error(format!("invalid token at byte {at}")))?;
                at += s.len();
                Kind::Symbol(s)
            }
        };
        tokens.push(Token {
            kind,
            start,
            end: at,
        });
    }
    tokens.push(Token {
        kind: Kind::End,
        start: at,
        end: at,
    });
    Ok(tokens)
}
