//! B-tree reads from an immutable database snapshot. This module does not open
//! files or acquire database locks: the caller must supply a consistent image.
use crate::error::{slice, u16be, u32be};
use crate::{record, varint, Error, Header, Result, Value};
use alloc::{collections::BTreeSet, string::String, vec, vec::Vec};

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_payload_bytes: usize,
    pub max_total_payload_bytes: usize,
    pub max_columns: usize,
    pub max_rows: usize,
    pub max_pages: usize,
    pub max_depth: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_payload_bytes: 16 * 1024 * 1024,
            max_total_payload_bytes: 256 * 1024 * 1024,
            max_columns: 32767,
            max_rows: 1_000_000,
            max_pages: 100_000,
            max_depth: 64,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKind {
    InteriorIndex,
    InteriorTable,
    LeafIndex,
    LeafTable,
}
impl PageKind {
    fn parse(n: u8) -> Result<Self> {
        match n {
            2 => Ok(Self::InteriorIndex),
            5 => Ok(Self::InteriorTable),
            10 => Ok(Self::LeafIndex),
            13 => Ok(Self::LeafTable),
            _ => Err(Error::Corrupt("B-tree page type")),
        }
    }
    pub fn is_leaf(self) -> bool {
        matches!(self, Self::LeafIndex | Self::LeafTable)
    }
    pub fn is_table(self) -> bool {
        matches!(self, Self::InteriorTable | Self::LeafTable)
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Absent for index B-trees, including WITHOUT ROWID tables. These return
    /// physical key-column order, not a SQL projection of the table definition.
    pub rowid: Option<i64>,
    pub values: Vec<Value>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaEntry {
    pub kind: String,
    pub name: String,
    pub table_name: String,
    pub root_page: u32,
    pub sql: Option<String>,
}
#[derive(Debug)]
pub struct Database<'image> {
    bytes: &'image [u8],
    header: Header,
    limits: Limits,
}

struct Layout {
    kind: PageKind,
    cells: Vec<usize>,
    rightmost: Option<u32>,
    freeblocks: Vec<(usize, usize)>,
}
struct Budget {
    pages: BTreeSet<u32>,
    payload_bytes: usize,
    rows: usize,
}
impl Budget {
    fn page(&mut self, id: u32, limits: Limits) -> Result<()> {
        if self.pages.len() >= limits.max_pages {
            return Err(Error::Limit("visited pages"));
        }
        if !self.pages.insert(id) {
            return Err(Error::Corrupt("repeated or cyclic page reference"));
        }
        Ok(())
    }
}
enum Task {
    Page(u32, usize),
    Record(Option<i64>, Vec<u8>),
}

impl<'image> Database<'image> {
    pub fn parse(bytes: &'image [u8]) -> Result<Self> {
        Self::with_limits(bytes, Limits::default())
    }
    pub fn with_limits(bytes: &'image [u8], limits: Limits) -> Result<Self> {
        let header = Header::parse(bytes)?;
        let result = Self {
            bytes,
            header,
            limits,
        };
        let layout = result.layout(1)?;
        if !layout.kind.is_table() {
            return Err(Error::Corrupt("schema is not a table B-tree"));
        }
        Ok(result)
    }
    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn page(&self, id: u32) -> Result<&'image [u8]> {
        if id == 0 || id > self.header.page_count {
            return Err(Error::Corrupt("page number out of range"));
        }
        let index = usize::try_from(id - 1).map_err(|_| Error::Limit("page offset"))?;
        let start = index
            .checked_mul(self.header.page_size)
            .ok_or(Error::Limit("page offset"))?;
        slice(self.bytes, start, self.header.usable_size())
    }
    pub fn page_kind(&self, id: u32) -> Result<PageKind> {
        Ok(self.layout(id)?.kind)
    }

    fn layout(&self, id: u32) -> Result<Layout> {
        let bytes = self.page(id)?;
        let base = if id == 1 { 100 } else { 0 };
        let header = slice(bytes, base, 8)?;
        let kind = PageKind::parse(header[0])?;
        let header_size = if kind.is_leaf() { 8 } else { 12 };
        let n = usize::from(u16be(header, 3)?);
        let pointers = base + header_size;
        let pointers_end = pointers + n * 2;
        slice(bytes, pointers, n * 2)?;
        let content_start = match u16be(header, 5)? {
            0 => 65536,
            n => usize::from(n),
        };
        if content_start < pointers_end || content_start > bytes.len() {
            return Err(Error::Corrupt("cell content region"));
        }
        if header[7] > 60 {
            return Err(Error::Corrupt("fragmented byte count"));
        }
        let mut cells = Vec::new();
        cells
            .try_reserve_exact(n)
            .map_err(|_| Error::Limit("allocation"))?;
        let mut offsets = BTreeSet::new();
        for i in 0..n {
            let offset = usize::from(u16be(bytes, pointers + 2 * i)?);
            if offset < content_start || offset >= bytes.len() || !offsets.insert(offset) {
                return Err(Error::Corrupt("cell pointer"));
            }
            cells.push(offset);
        }
        let mut free = usize::from(u16be(header, 1)?);
        let mut freeblocks = Vec::new();
        while free != 0 {
            if free < content_start {
                return Err(Error::Corrupt("freeblock offset"));
            }
            let next = usize::from(u16be(bytes, free)?);
            let size = usize::from(u16be(bytes, free + 2)?);
            if size < 4 {
                return Err(Error::Corrupt("freeblock size"));
            }
            slice(bytes, free, size)?;
            if next != 0 && next < free + size {
                return Err(Error::Corrupt("freeblock cycle or overlap"));
            }
            freeblocks.push((free, free + size));
            free = next;
        }
        Ok(Layout {
            kind,
            cells,
            freeblocks,
            rightmost: if kind.is_leaf() {
                None
            } else {
                Some(u32be(bytes, base + 8)?)
            },
        })
    }

    fn payload(
        &self,
        page: &[u8],
        at: usize,
        size: usize,
        kind: PageKind,
        budget: &mut Budget,
    ) -> Result<(Vec<u8>, usize)> {
        if size > self.limits.max_payload_bytes {
            return Err(Error::Limit("cell payload"));
        }
        budget.payload_bytes = budget
            .payload_bytes
            .checked_add(size)
            .ok_or(Error::Limit("total payload"))?;
        if budget.payload_bytes > self.limits.max_total_payload_bytes {
            return Err(Error::Limit("total payload"));
        }
        let local =
            local_payload_size(size, self.header.usable_size(), kind == PageKind::LeafTable);
        let mut data = Vec::new();
        data.try_reserve_exact(size)
            .map_err(|_| Error::Limit("allocation"))?;
        data.extend_from_slice(slice(page, at, local)?);
        let mut end = at + local;
        if local != size {
            let mut next = u32be(page, end)?;
            end += 4;
            while data.len() < size {
                budget.page(next, self.limits)?;
                let overflow = self.page(next)?;
                next = u32be(overflow, 0)?;
                let count = (size - data.len()).min(overflow.len() - 4);
                data.extend_from_slice(&overflow[4..4 + count]);
            }
            if next != 0 {
                return Err(Error::Corrupt("overflow chain exceeds payload"));
            }
        }
        Ok((data, end))
    }

    /// Stream physical records without recursive traversal or retaining every
    /// decoded row. Index interior-page records are included in key order.
    pub fn visit_rows(&self, root: u32, mut visit: impl FnMut(Row) -> Result<()>) -> Result<()> {
        let is_table = self.page_kind(root)?.is_table();
        let mut budget = Budget {
            pages: BTreeSet::new(),
            payload_bytes: 0,
            rows: 0,
        };
        let mut stack = vec![Task::Page(root, 0)];
        let mut previous_rowid = None;
        while let Some(task) = stack.pop() {
            match task {
                Task::Record(rowid, payload) => {
                    if budget.rows >= self.limits.max_rows {
                        return Err(Error::Limit("row count"));
                    }
                    if let Some(id) = rowid {
                        if previous_rowid.is_some_and(|previous| previous >= id) {
                            return Err(Error::Corrupt("table rowid order"));
                        }
                        previous_rowid = Some(id);
                    }
                    budget.rows += 1;
                    let values =
                        record::decode(&payload, self.header.encoding, self.limits.max_columns)?;
                    visit(Row { rowid, values })?;
                }
                Task::Page(id, depth) => {
                    if depth >= self.limits.max_depth {
                        return Err(Error::Limit("B-tree depth"));
                    }
                    budget.page(id, self.limits)?;
                    let page = self.page(id)?;
                    let layout = self.layout(id)?;
                    if layout.kind.is_table() != is_table {
                        return Err(Error::Corrupt("mixed B-tree page types"));
                    }
                    let mut ranges = layout.freeblocks;
                    if let Some(rightmost) = layout.rightmost {
                        stack.push(Task::Page(rightmost, depth + 1));
                    }
                    let mut previous_separator = None;
                    for &cell in layout.cells.iter().rev() {
                        let mut at = cell;
                        let child = if layout.kind.is_leaf() {
                            None
                        } else {
                            let child = u32be(page, at)?;
                            at += 4;
                            Some(child)
                        };
                        if layout.kind == PageKind::InteriorTable {
                            let (rowid, count) =
                                varint::decode(page.get(at..).ok_or(Error::Truncated)?)?;
                            let rowid = rowid as i64;
                            if previous_separator.is_some_and(|previous| previous <= rowid) {
                                return Err(Error::Corrupt("separator order"));
                            }
                            previous_separator = Some(rowid);
                            at += count;
                        } else {
                            let (size, count) =
                                varint::decode(page.get(at..).ok_or(Error::Truncated)?)?;
                            at += count;
                            let rowid = if layout.kind == PageKind::LeafTable {
                                let (rowid, count) =
                                    varint::decode(page.get(at..).ok_or(Error::Truncated)?)?;
                                at += count;
                                Some(rowid as i64)
                            } else {
                                None
                            };
                            let size =
                                usize::try_from(size).map_err(|_| Error::Limit("payload size"))?;
                            let (payload, end) =
                                self.payload(page, at, size, layout.kind, &mut budget)?;
                            at = end;
                            stack.push(Task::Record(rowid, payload));
                        }
                        ranges.push((cell, at));
                        if let Some(child) = child {
                            stack.push(Task::Page(child, depth + 1));
                        }
                    }
                    ranges.sort_unstable();
                    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
                        return Err(Error::Corrupt("overlapping cell/freeblock ranges"));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn rows(&self, root: u32) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        self.visit_rows(root, |row| {
            rows.try_reserve(1)
                .map_err(|_| Error::Limit("allocation"))?;
            rows.push(row);
            Ok(())
        })?;
        Ok(rows)
    }

    pub fn schema(&self) -> Result<Vec<SchemaEntry>> {
        let mut entries = Vec::new();
        self.visit_rows(1, |row| {
            if row.values.len() != 5 {
                return Err(Error::Corrupt("schema record column count"));
            }
            let text = |i: usize| match &row.values[i] {
                Value::Text(text) => text.to_string(),
                _ => Err(Error::Corrupt("schema text field")),
            };
            let root_page = match row.values[3] {
                Value::Integer(n) => {
                    u32::try_from(n).map_err(|_| Error::Corrupt("schema root page"))?
                }
                _ => return Err(Error::Corrupt("schema root page")),
            };
            if root_page > self.header.page_count {
                return Err(Error::Corrupt("schema root page beyond database"));
            }
            entries.push(SchemaEntry {
                kind: text(0)?,
                name: text(1)?,
                table_name: text(2)?,
                root_page,
                sql: match &row.values[4] {
                    Value::Null => None,
                    _ => Some(text(4)?),
                },
            });
            Ok(())
        })?;
        Ok(entries)
    }
}

pub(crate) fn local_payload_size(payload: usize, usable: usize, table_leaf: bool) -> usize {
    let max = if table_leaf {
        usable - 35
    } else {
        (usable - 12) * 64 / 255 - 23
    };
    if payload <= max {
        return payload;
    }
    let min = (usable - 12) * 32 / 255 - 23;
    let candidate = min + (payload - min) % (usable - 4);
    if candidate <= max {
        candidate
    } else {
        min
    }
}
