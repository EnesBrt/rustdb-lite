//! Construct a new SQLite database image in memory. This is an offline storage
//! builder, not a transactional pager. It never modifies an existing file.
use crate::{
    database::local_payload_size, record, varint, AutoVacuum, Encoding, Error, Limits, Result,
    Text, Value,
};
use alloc::{
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};

#[derive(Debug, Clone)]
pub struct Table {
    pub name: String,
    /// Columns are emitted without a declared type or constraints (BLOB affinity).
    pub columns: Vec<String>,
    pub rows: Vec<(i64, Vec<Value>)>,
}
impl Table {
    pub fn new(name: &str, columns: &[&str]) -> Self {
        Self {
            name: name.to_string(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
            rows: Vec::new(),
        }
    }
}

pub struct ImageBuilder {
    page_size: usize,
    encoding: Encoding,
    auto_vacuum: AutoVacuum,
    max_image_bytes: usize,
    limits: Limits,
    tables: Vec<Table>,
    schema_sql: alloc::collections::BTreeMap<String, String>,
    indexes: Vec<Index>,
    views: Vec<(String, String)>,
}
/// Records supplied by the SQL layer in SQLite index order, including the rowid
/// suffix. Interior index keys are real records, not duplicate separators.
pub(crate) struct Index {
    pub name: String,
    pub table: String,
    pub sql: Option<String>,
    pub entries: Vec<Vec<Value>>,
}
impl ImageBuilder {
    pub fn new(page_size: usize) -> Result<Self> {
        if !(512..=65536).contains(&page_size) || !page_size.is_power_of_two() {
            return Err(Error::InvalidInput(
                "page size must be a power of two from 512 to 65536",
            ));
        }
        Ok(Self {
            page_size,
            encoding: Encoding::Utf8,
            auto_vacuum: AutoVacuum::None,
            max_image_bytes: 256 * 1024 * 1024,
            limits: Limits::default(),
            tables: Vec::new(),
            schema_sql: alloc::collections::BTreeMap::new(),
            indexes: Vec::new(),
            views: Vec::new(),
        })
    }
    pub fn limits(mut self, limits: Limits, max_image_bytes: usize) -> Self {
        self.limits = limits;
        self.max_image_bytes = max_image_bytes;
        self
    }
    /// Set the record encoding before adding tables. Text values must already
    /// use this encoding; schema strings are converted automatically.
    pub fn encoding(mut self, encoding: Encoding) -> Result<Self> {
        if !self.tables.is_empty() || !self.indexes.is_empty() || !self.views.is_empty() {
            return Err(Error::InvalidInput("set encoding before adding data"));
        }
        self.encoding = encoding;
        Ok(self)
    }
    /// Emit pointer maps and the selected header mode. Every image is freshly
    /// packed: this builder does not retain free pages for incremental vacuum.
    pub fn auto_vacuum(mut self, mode: AutoVacuum) -> Self {
        self.auto_vacuum = mode;
        self
    }
    pub fn add_table(&mut self, table: Table) -> Result<()> {
        self.add_table_internal(table, false)
    }
    fn add_table_internal(&mut self, table: Table, sequence: bool) -> Result<()> {
        if table.name.contains('\0')
            || !sequence && table.name.to_ascii_lowercase().starts_with("sqlite_")
        {
            return Err(Error::InvalidInput("invalid or reserved table name"));
        }
        if self
            .tables
            .iter()
            .any(|t| t.name.eq_ignore_ascii_case(&table.name))
        {
            return Err(Error::InvalidInput("duplicate table name"));
        }
        if table.columns.is_empty() || table.columns.len() > self.limits.max_columns {
            return Err(Error::Limit("table columns"));
        }
        for (i, column) in table.columns.iter().enumerate() {
            if column.contains('\0') {
                return Err(Error::InvalidInput("invalid column name"));
            }
            if table.columns[..i]
                .iter()
                .any(|previous| previous.eq_ignore_ascii_case(column))
            {
                return Err(Error::InvalidInput("duplicate column name"));
            }
        }
        if table.rows.len() > self.limits.max_rows {
            return Err(Error::Limit("table rows"));
        }
        let mut last = None;
        for (rowid, values) in &table.rows {
            if last.is_some_and(|last| last >= *rowid) {
                return Err(Error::InvalidInput("rowids must be strictly increasing"));
            }
            last = Some(*rowid);
            if values.len() != table.columns.len() {
                return Err(Error::InvalidInput("row column count"));
            }
            if values
                .iter()
                .any(|v| matches!(v, Value::Text(text) if text.encoding != self.encoding))
            {
                return Err(Error::Unsupported("text encoding differs from database"));
            }
        }
        self.tables
            .try_reserve(1)
            .map_err(|_| Error::Limit("allocation"))?;
        self.tables.push(table);
        Ok(())
    }

    pub(crate) fn add_table_with_sql(&mut self, table: Table, sql: String) -> Result<()> {
        let name = table.name.clone();
        let sequence = name == "sqlite_sequence"
            && table.columns == ["name", "seq"]
            && sql == "CREATE TABLE sqlite_sequence(name,seq)";
        self.add_table_internal(table, sequence)?;
        self.schema_sql.insert(name, sql);
        Ok(())
    }

    pub(crate) fn add_index(&mut self, index: Index) -> Result<()> {
        if index.entries.len() > self.limits.max_rows {
            return Err(Error::Limit("index entries"));
        }
        if !self.tables.iter().any(|t| t.name == index.table) {
            return Err(Error::InvalidInput("index table missing"));
        }
        if index
            .entries
            .iter()
            .flatten()
            .any(|v| matches!(v, Value::Text(t) if t.encoding != self.encoding))
        {
            return Err(Error::InvalidInput(
                "index text encoding differs from database",
            ));
        }
        self.indexes.push(index);
        Ok(())
    }
    pub(crate) fn add_view(&mut self, name: String, sql: String) -> Result<()> {
        if name.contains('\0')
            || name.to_ascii_lowercase().starts_with("sqlite_")
            || self
                .tables
                .iter()
                .any(|t| t.name.eq_ignore_ascii_case(&name))
            || self
                .indexes
                .iter()
                .any(|i| i.name.eq_ignore_ascii_case(&name))
            || self
                .views
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case(&name))
        {
            return Err(Error::InvalidInput("invalid or duplicate view name"));
        }
        self.views.push((name, sql));
        Ok(())
    }

    pub fn finish(self) -> Result<Vec<u8>> {
        if self.limits.max_depth == 0 {
            return Err(Error::Limit("B-tree depth"));
        }
        let mut pages = Pages {
            pages: Vec::new(),
            size: self.page_size,
            limit: self.max_image_bytes,
            max_pages: self.limits.max_pages,
            pointer_maps: self.auto_vacuum != AutoVacuum::None,
        };
        pages.allocate()?; // Page one is always the schema root.
        let mut roots = Vec::new();
        for _ in &self.tables {
            roots.push(pages.allocate()?);
        }
        let mut index_roots = Vec::new();
        for _ in &self.indexes {
            index_roots.push(pages.allocate()?);
        }
        // Auto-vacuum requires every root to precede all non-root pages.
        let largest_root = index_roots.last().or(roots.last()).copied().unwrap_or(1);
        for &root in roots.iter().chain(&index_roots) {
            pages.backlink(root, 1, 0)?;
        }
        let mut schema = Vec::new();
        let mut payload_total = 0usize;
        for (i, (table, root)) in self.tables.into_iter().zip(roots).enumerate() {
            let sql = self
                .schema_sql
                .get(&table.name)
                .cloned()
                .unwrap_or_else(|| {
                    format!(
                        "CREATE TABLE {}({})",
                        quote(&table.name),
                        table
                            .columns
                            .iter()
                            .map(|s| quote(s))
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                });
            let record = vec![
                Value::Text(Text::utf8("table")),
                Value::Text(Text::utf8(&table.name)),
                Value::Text(Text::utf8(&table.name)),
                Value::Integer(i64::from(root)),
                Value::Text(Text::utf8(&sql)),
            ];
            schema.push((
                i64::try_from(i + 1).map_err(|_| Error::Limit("schema entries"))?,
                record,
            ));
            pages.tree(root, &table.rows, self.limits, &mut payload_total)?;
        }
        for (index, root) in self.indexes.into_iter().zip(index_roots) {
            schema.push((
                i64::try_from(schema.len() + 1).map_err(|_| Error::Limit("schema entries"))?,
                vec![
                    Value::Text(Text::utf8("index")),
                    Value::Text(Text::utf8(&index.name)),
                    Value::Text(Text::utf8(&index.table)),
                    Value::Integer(i64::from(root)),
                    index
                        .sql
                        .map(|s| Value::Text(Text::utf8(&s)))
                        .unwrap_or(Value::Null),
                ],
            ));
            pages.index_tree(root, &index.entries, self.limits, &mut payload_total)?;
        }
        for (name, sql) in self.views {
            schema.push((
                i64::try_from(schema.len() + 1).map_err(|_| Error::Limit("schema entries"))?,
                vec![
                    Value::Text(Text::utf8("view")),
                    Value::Text(Text::utf8(&name)),
                    Value::Text(Text::utf8(&name)),
                    Value::Integer(0),
                    Value::Text(Text::utf8(&sql)),
                ],
            ));
        }
        for (_, values) in &mut schema {
            for value in values {
                if let Value::Text(text) = value {
                    *text = text.transcode(self.encoding)?;
                }
            }
        }
        pages.tree(1, &schema, self.limits, &mut payload_total)?;
        let page_count =
            u32::try_from(pages.pages.len()).map_err(|_| Error::Limit("page count"))?;
        let first = &mut pages.pages[0];
        first[..16].copy_from_slice(b"SQLite format 3\0");
        let size = if self.page_size == 65536 {
            1
        } else {
            self.page_size as u16
        };
        first[16..18].copy_from_slice(&size.to_be_bytes());
        first[18] = 1;
        first[19] = 1;
        first[21..24].copy_from_slice(&[64, 32, 32]);
        first[24..28].copy_from_slice(&1u32.to_be_bytes());
        first[28..32].copy_from_slice(&page_count.to_be_bytes());
        first[40..44].copy_from_slice(&1u32.to_be_bytes());
        first[44..48].copy_from_slice(&4u32.to_be_bytes());
        if self.auto_vacuum != AutoVacuum::None {
            first[52..56].copy_from_slice(&largest_root.to_be_bytes());
            first[64..68].copy_from_slice(
                &u32::from(self.auto_vacuum == AutoVacuum::Incremental).to_be_bytes(),
            );
        }
        let encoding: u32 = match self.encoding {
            Encoding::Utf8 => 1,
            Encoding::Utf16Le => 2,
            Encoding::Utf16Be => 3,
        };
        first[56..60].copy_from_slice(&encoding.to_be_bytes());
        first[92..96].copy_from_slice(&1u32.to_be_bytes());
        let mut image = Vec::new();
        let len = pages
            .pages
            .len()
            .checked_mul(self.page_size)
            .ok_or(Error::Limit("image size"))?;
        image
            .try_reserve_exact(len)
            .map_err(|_| Error::Limit("allocation"))?;
        for page in pages.pages {
            image.extend_from_slice(&page);
        }
        Ok(image)
    }
}
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
struct Pages {
    pages: Vec<Vec<u8>>,
    size: usize,
    limit: usize,
    max_pages: usize,
    pointer_maps: bool,
}
#[derive(Clone, Copy)]
struct Node {
    page: u32,
    last: i64,
}
struct Cell {
    key: i64,
    bytes: Vec<u8>,
    overflow: u32,
}

impl Pages {
    fn allocate(&mut self) -> Result<u32> {
        loop {
            let id = self.allocate_raw()?;
            // The lock-byte page is never a B-tree, overflow, or ptrmap page.
            if id == self.lock_page() || (self.pointer_maps && id > 1 && self.map_page(id) == id) {
                continue;
            }
            return Ok(id);
        }
    }
    fn lock_page(&self) -> u32 {
        (0x4000_0000 / self.size + 1) as u32
    }
    fn map_page(&self, id: u32) -> u32 {
        let stride = (self.size / 5 + 1) as u32;
        let page = (id - 2) / stride * stride + 2;
        page + u32::from(page == self.lock_page())
    }
    fn backlink(&mut self, id: u32, kind: u8, parent: u32) -> Result<()> {
        if !self.pointer_maps || id == 1 {
            return Ok(());
        }
        let map = self.map_page(id);
        if id <= map || id == self.lock_page() {
            return Err(Error::InvalidInput("invalid pointer-map target"));
        }
        let offset = (id - map - 1) as usize * 5;
        let entry = self
            .pages
            .get_mut(map as usize - 1)
            .and_then(|page| page.get_mut(offset..offset + 5))
            .ok_or(Error::InvalidInput("pointer-map entry out of range"))?;
        if entry != [0; 5] {
            return Err(Error::InvalidInput("duplicate pointer-map owner"));
        }
        entry[0] = kind;
        entry[1..].copy_from_slice(&parent.to_be_bytes());
        Ok(())
    }
    fn allocate_raw(&mut self) -> Result<u32> {
        let count = self
            .pages
            .len()
            .checked_add(1)
            .ok_or(Error::Limit("page count"))?;
        if count > self.max_pages
            || count
                .checked_mul(self.size)
                .ok_or(Error::Limit("image size"))?
                > self.limit
        {
            return Err(Error::Limit("image size or page count"));
        }
        let id = u32::try_from(count).map_err(|_| Error::Limit("page count"))?;
        if id == u32::MAX {
            return Err(Error::Limit("page count"));
        }
        let mut page = Vec::new();
        page.try_reserve_exact(self.size)
            .map_err(|_| Error::Limit("allocation"))?;
        page.resize(self.size, 0);
        self.pages
            .try_reserve(1)
            .map_err(|_| Error::Limit("allocation"))?;
        self.pages.push(page);
        Ok(id)
    }
    fn overflow(&mut self, bytes: &[u8]) -> Result<u32> {
        let mut first = 0;
        let mut previous = None;
        for chunk in bytes.chunks(self.size - 4) {
            let id = self.allocate()?;
            if first == 0 {
                first = id;
            }
            if let Some(previous) = previous {
                self.pages[previous as usize - 1][..4].copy_from_slice(&id.to_be_bytes());
                self.backlink(id, 4, previous)?;
            }
            self.pages[id as usize - 1][4..4 + chunk.len()].copy_from_slice(chunk);
            previous = Some(id);
        }
        Ok(first)
    }
    fn leaf_cell(
        &mut self,
        rowid: i64,
        values: &[Value],
        limits: Limits,
        total: &mut usize,
    ) -> Result<Cell> {
        let payload = record::encode(values, limits.max_payload_bytes)?;
        *total = total
            .checked_add(payload.len())
            .ok_or(Error::Limit("total payload"))?;
        if *total > limits.max_total_payload_bytes {
            return Err(Error::Limit("total payload"));
        }
        let local = local_payload_size(payload.len(), self.size, true);
        let mut cell = Vec::new();
        varint::encode(payload.len() as u64, &mut cell)?;
        varint::encode(rowid as u64, &mut cell)?;
        cell.extend_from_slice(&payload[..local]);
        let overflow = if local != payload.len() {
            let id = self.overflow(&payload[local..])?;
            cell.extend_from_slice(&id.to_be_bytes());
            id
        } else {
            0
        };
        Ok(Cell {
            key: rowid,
            bytes: cell,
            overflow,
        })
    }
    fn fits(&self, id: u32, header: usize, cells: &[Cell]) -> bool {
        let base = if id == 1 { 100 } else { 0 };
        cells
            .iter()
            .try_fold(base + header, |size, c| size.checked_add(2 + c.bytes.len()))
            .is_some_and(|size| size <= self.size)
    }
    fn write_page(&mut self, id: u32, cells: &[Cell], rightmost: Option<u32>) -> Result<()> {
        self.write_btree_page(id, cells, rightmost, false)
    }
    fn write_btree_page(
        &mut self,
        id: u32,
        cells: &[Cell],
        rightmost: Option<u32>,
        index: bool,
    ) -> Result<()> {
        let header = if rightmost.is_some() { 12 } else { 8 };
        if !self.fits(id, header, cells) {
            return Err(Error::InvalidInput("page exceeds capacity"));
        }
        let count = u16::try_from(cells.len()).map_err(|_| Error::Limit("cells per page"))?;
        for cell in cells {
            if cell.overflow != 0 {
                self.backlink(cell.overflow, 3, id)?;
            }
            if rightmost.is_some() {
                self.backlink(crate::error::u32be(&cell.bytes, 0)?, 5, id)?;
            }
        }
        if let Some(right) = rightmost {
            self.backlink(right, 5, id)?;
        }
        let page = &mut self.pages[id as usize - 1];
        let base = if id == 1 { 100 } else { 0 };
        page[base..].fill(0);
        page[base] = match (index, rightmost.is_some()) {
            (false, false) => 13,
            (false, true) => 5,
            (true, false) => 10,
            (true, true) => 2,
        };
        page[base + 3..base + 5].copy_from_slice(&count.to_be_bytes());
        if let Some(right) = rightmost {
            page[base + 8..base + 12].copy_from_slice(&right.to_be_bytes());
        }
        let mut end = self.size;
        for (i, cell) in cells.iter().enumerate() {
            end -= cell.bytes.len();
            page[end..end + cell.bytes.len()].copy_from_slice(&cell.bytes);
            let pointer = base + header + i * 2;
            page[pointer..pointer + 2].copy_from_slice(&(end as u16).to_be_bytes());
        }
        page[base + 5..base + 7].copy_from_slice(&(end as u16).to_be_bytes());
        Ok(())
    }
    fn index_tree(
        &mut self,
        root: u32,
        rows: &[Vec<Value>],
        limits: Limits,
        total: &mut usize,
    ) -> Result<()> {
        let mut cells = Vec::new();
        let mut largest = 0;
        for values in rows {
            if values.len() > limits.max_columns {
                return Err(Error::Limit("index columns"));
            }
            let payload = record::encode(values, limits.max_payload_bytes)?;
            *total = total
                .checked_add(payload.len())
                .ok_or(Error::Limit("total payload"))?;
            if *total > limits.max_total_payload_bytes {
                return Err(Error::Limit("total payload"));
            }
            let local = local_payload_size(payload.len(), self.size, false);
            let mut bytes = Vec::new();
            varint::encode(payload.len() as u64, &mut bytes)?;
            bytes.extend_from_slice(&payload[..local]);
            let overflow = if local < payload.len() {
                let id = self.overflow(&payload[local..])?;
                bytes.extend_from_slice(&id.to_be_bytes());
                id
            } else {
                0
            };
            largest = largest.max(bytes.len());
            cells.push(Cell {
                key: 0,
                bytes,
                overflow,
            });
        }
        // A conservative common fanout keeps every leaf at the same depth even
        // when payload sizes vary. Four bytes per interior cell hold its child.
        let capacity = (self.size - 12) / (largest + 6);
        if capacity < 2 {
            return Err(Error::InvalidInput("index cell exceeds page"));
        }
        let mut height = 0;
        let mut maximum = capacity;
        while cells.len() > maximum {
            height += 1;
            if height >= limits.max_depth {
                return Err(Error::Limit("B-tree depth"));
            }
            maximum = maximum
                .saturating_mul(capacity + 1)
                .saturating_add(capacity);
        }
        self.index_node(root, &cells, height, capacity)
    }
    fn index_node(
        &mut self,
        root: u32,
        cells: &[Cell],
        height: usize,
        capacity: usize,
    ) -> Result<()> {
        if height == 0 {
            return self.write_btree_page(root, cells, None, true);
        }
        let minimum_child = (1usize
            .checked_shl(height as u32)
            .ok_or(Error::Limit("B-tree depth"))?)
            - 1;
        let children = (capacity + 1).min((cells.len() + 1) / (minimum_child + 1));
        if children < 2 {
            return Err(Error::InvalidInput("index tree occupancy"));
        }
        let remaining = cells.len() - (children - 1);
        let mut at = 0;
        let mut parent = Vec::new();
        let mut rightmost = 0;
        for i in 0..children {
            let count = remaining / children + usize::from(i < remaining % children);
            let child = self.allocate()?;
            self.index_node(child, &cells[at..at + count], height - 1, capacity)?;
            at += count;
            if i + 1 < children {
                let mut bytes = child.to_be_bytes().to_vec();
                bytes.extend_from_slice(&cells[at].bytes);
                parent.push(Cell {
                    key: 0,
                    bytes,
                    overflow: cells[at].overflow,
                });
                at += 1;
            } else {
                rightmost = child;
            }
        }
        self.write_btree_page(root, &parent, Some(rightmost), true)
    }
    fn parent_cells(children: &[Node]) -> Result<Vec<Cell>> {
        let mut cells = Vec::new();
        for child in &children[..children.len().saturating_sub(1)] {
            let mut bytes = child.page.to_be_bytes().to_vec();
            varint::encode(child.last as u64, &mut bytes)?;
            cells.push(Cell {
                key: child.last,
                bytes,
                overflow: 0,
            });
        }
        Ok(cells)
    }
    fn tree(
        &mut self,
        root: u32,
        rows: &[(i64, Vec<Value>)],
        limits: Limits,
        total: &mut usize,
    ) -> Result<()> {
        if rows.len() > limits.max_rows {
            return Err(Error::Limit("row count"));
        }
        let mut cells = Vec::new();
        for (key, values) in rows {
            cells.push(self.leaf_cell(*key, values, limits, total)?);
        }
        if self.fits(root, 8, &cells) {
            return self.write_page(root, &cells, None);
        }
        let mut nodes = Vec::new();
        let mut start = 0;
        while start < cells.len() {
            let id = self.allocate()?;
            let mut end = start;
            let mut used = 8;
            while end < cells.len() && used + 2 + cells[end].bytes.len() <= self.size {
                used += 2 + cells[end].bytes.len();
                end += 1;
            }
            if start == end {
                return Err(Error::InvalidInput("cell exceeds page"));
            }
            self.write_page(id, &cells[start..end], None)?;
            nodes.push(Node {
                page: id,
                last: cells[end - 1].key,
            });
            start = end;
        }
        let mut depth = 1;
        loop {
            if depth >= limits.max_depth {
                return Err(Error::Limit("B-tree depth"));
            }
            let parent = Self::parent_cells(&nodes)?;
            if self.fits(root, 12, &parent) {
                return self.write_page(
                    root,
                    &parent,
                    Some(
                        nodes
                            .last()
                            .ok_or(Error::InvalidInput("empty interior"))?
                            .page,
                    ),
                );
            }
            let max_children = (self.size - 12) / 15 + 1;
            let mut parents = Vec::new();
            let mut start = 0;
            while start < nodes.len() {
                let mut end = (start + max_children).min(nodes.len());
                if nodes.len() - end == 1 {
                    end -= 1;
                }
                let group = &nodes[start..end];
                let id = self.allocate()?;
                self.write_page(
                    id,
                    &Self::parent_cells(group)?,
                    Some(group[group.len() - 1].page),
                )?;
                parents.push(Node {
                    page: id,
                    last: group[group.len() - 1].last,
                });
                start = end;
            }
            nodes = parents;
            depth += 1;
        }
    }
}
