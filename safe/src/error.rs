use core::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Truncated,
    InvalidHeader(&'static str),
    Corrupt(&'static str),
    Limit(&'static str),
    Unsupported(&'static str),
    InvalidInput(&'static str),
    InvalidText,
    Sql(alloc::string::String),
    Constraint(alloc::string::String),
    Storage(alloc::string::String),
    Busy(&'static str),
    Full,
}
pub type Result<T> = core::result::Result<T, Error>;
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("truncated SQLite input"),
            Self::InvalidHeader(s) => write!(f, "invalid SQLite header: {s}"),
            Self::Corrupt(s) => write!(f, "corrupt SQLite data: {s}"),
            Self::Limit(s) => write!(f, "resource limit: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported: {s}"),
            Self::InvalidInput(s) => write!(f, "invalid input: {s}"),
            Self::InvalidText => f.write_str("invalid text encoding"),
            Self::Sql(s) => write!(f, "SQL error: {s}"),
            Self::Constraint(s) => write!(f, "constraint failed: {s}"),
            Self::Storage(s) => write!(f, "storage error: {s}"),
            Self::Busy(s) => write!(f, "database busy: {s}"),
            Self::Full => f.write_str("database or disk is full"),
        }
    }
}
impl core::error::Error for Error {}

pub(crate) fn slice(bytes: &[u8], at: usize, len: usize) -> Result<&[u8]> {
    bytes
        .get(at..at.checked_add(len).ok_or(Error::Truncated)?)
        .ok_or(Error::Truncated)
}
pub(crate) fn u16be(bytes: &[u8], at: usize) -> Result<u16> {
    let b = slice(bytes, at, 2)?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}
pub(crate) fn u32be(bytes: &[u8], at: usize) -> Result<u32> {
    let b = slice(bytes, at, 4)?;
    Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}
