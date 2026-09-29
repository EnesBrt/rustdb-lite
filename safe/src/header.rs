use crate::error::{slice, u16be, u32be};
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
}

/// On-disk pointer-map mode. This does not itself perform vacuum operations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AutoVacuum {
    #[default]
    None,
    Full,
    Incremental,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub page_size: usize,
    pub reserved_bytes: usize,
    pub page_count: u32,
    pub write_version: u8,
    pub read_version: u8,
    pub schema_cookie: u32,
    pub schema_format: u32,
    pub encoding: Encoding,
    pub freelist_head: u32,
    pub freelist_pages: u32,
    pub user_version: u32,
    pub application_id: u32,
    pub largest_root_page: u32,
    pub auto_vacuum: AutoVacuum,
    pub sqlite_version: u32,
}
impl Header {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        slice(bytes, 0, 100)?;
        if &bytes[..16] != b"SQLite format 3\0" {
            return Err(Error::InvalidHeader("signature"));
        }
        let page_size = match u16be(bytes, 16)? {
            1 => 65536,
            n => usize::from(n),
        };
        if !(512..=65536).contains(&page_size) || !page_size.is_power_of_two() {
            return Err(Error::InvalidHeader("page size"));
        }
        let reserved_bytes = usize::from(bytes[20]);
        if page_size - reserved_bytes < 480 {
            return Err(Error::InvalidHeader("usable page size"));
        }
        if ![1, 2].contains(&bytes[18]) || ![1, 2].contains(&bytes[19]) {
            return Err(Error::Unsupported("database read/write format"));
        }
        if bytes[21..24] != [64, 32, 32] {
            return Err(Error::InvalidHeader("payload fractions"));
        }
        if bytes.len() % page_size != 0 {
            return Err(Error::Truncated);
        }
        let file_pages =
            u32::try_from(bytes.len() / page_size).map_err(|_| Error::Limit("page count"))?;
        let declared_pages = u32be(bytes, 28)?;
        let page_count = if declared_pages != 0 && u32be(bytes, 24)? == u32be(bytes, 92)? {
            if declared_pages > file_pages {
                return Err(Error::Truncated);
            }
            declared_pages
        } else {
            file_pages
        };
        if page_count == 0 {
            return Err(Error::InvalidHeader("no schema page"));
        }
        let schema_format = u32be(bytes, 44)?;
        if schema_format > 4 {
            return Err(Error::Unsupported("schema format"));
        }
        let encoding = match u32be(bytes, 56)? {
            0 if schema_format == 0 => Encoding::Utf8,
            1 => Encoding::Utf8,
            2 => Encoding::Utf16Le,
            3 => Encoding::Utf16Be,
            _ => return Err(Error::InvalidHeader("text encoding")),
        };
        let largest_root_page = u32be(bytes, 52)?;
        let incremental = u32be(bytes, 64)? != 0;
        if largest_root_page > page_count || (largest_root_page == 0 && incremental) {
            return Err(Error::InvalidHeader("auto-vacuum settings"));
        }
        let auto_vacuum = match (largest_root_page != 0, incremental) {
            (false, _) => AutoVacuum::None,
            (true, false) => AutoVacuum::Full,
            (true, true) => AutoVacuum::Incremental,
        };
        Ok(Self {
            page_size,
            reserved_bytes,
            page_count,
            write_version: bytes[18],
            read_version: bytes[19],
            schema_cookie: u32be(bytes, 40)?,
            schema_format,
            encoding,
            freelist_head: u32be(bytes, 32)?,
            freelist_pages: u32be(bytes, 36)?,
            user_version: u32be(bytes, 60)?,
            application_id: u32be(bytes, 68)?,
            largest_root_page,
            auto_vacuum,
            sqlite_version: u32be(bytes, 96)?,
        })
    }
    pub fn usable_size(&self) -> usize {
        self.page_size - self.reserved_bytes
    }
}
