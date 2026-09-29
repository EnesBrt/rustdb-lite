//! AUTOINCREMENT tracks attempted rowids in a statement and persists on success.
use super::*;

pub(super) const NAME: &str = "sqlite_sequence";
pub(super) const SQL: &str = "CREATE TABLE sqlite_sequence(name,seq)";

pub(super) struct Tracker {
    index: usize,
    name: String,
    rowid: Option<i64>,
    original: Option<i64>,
    maximum: i64,
}
impl Tracker {
    pub(super) fn allocate(&self, largest: Option<i64>) -> Result<i64> {
        let next = largest
            .map_or(Some(1), |n| n.checked_add(1))
            .ok_or(Error::Full)?;
        Ok(next.max(self.maximum.checked_add(1).ok_or(Error::Full)?))
    }
    pub(super) fn step(&mut self, id: i64) {
        self.maximum = self.maximum.max(id);
    }
}
fn named(values: &[Value], name: &str) -> bool {
    matches!(values.first(), Some(Value::Text(text)) if text.bytes == name.as_bytes())
}
impl Connection {
    pub(super) fn ensure_sequence(&mut self) -> Result<usize> {
        if let Ok(index) = self.index(NAME) {
            return Ok(index);
        }
        let prepared = self.prepare(SQL)?;
        let Statement::Create { columns, .. } = prepared.statement else {
            return Err(Error::Corrupt("internal sequence declaration"));
        };
        let index = self.state.tables.len();
        self.state.tables.push(StoredTable {
            name: NAME.into(),
            columns,
            sql: SQL.into(),
            rows: BTreeMap::new(),
            indexes: Vec::new(),
            checks: Vec::new(),
            rowid_alias: None,
            primary_key: Vec::new(),
            key_conflict: Conflict::Default,
            autoincrement: false,
        });
        Ok(index)
    }
    pub(super) fn start_sequence(&self, table: usize, fuel: &mut Fuel) -> Result<Option<Tracker>> {
        let table = &self.state.tables[table];
        if !table.autoincrement {
            return Ok(None);
        }
        let index = self
            .index(NAME)
            .map_err(|_| Error::Corrupt("missing sqlite_sequence"))?;
        let mut tracker = Tracker {
            index,
            name: table.name.clone(),
            rowid: None,
            original: None,
            maximum: 0,
        };
        for (id, values) in &self.state.tables[index].rows {
            fuel.spend()?;
            if named(values, &table.name) {
                let n = scalar::integer(values.get(1).ok_or(Error::Corrupt("sequence columns"))?)?;
                tracker.rowid = Some(*id);
                tracker.original = Some(n);
                tracker.maximum = n;
                break;
            }
        }
        Ok(Some(tracker))
    }
    pub(super) fn finish_sequence(&mut self, tracker: Tracker) -> Result<()> {
        if tracker.original.is_some_and(|n| tracker.maximum <= n) {
            return Ok(());
        }
        let table = &mut self.state.tables[tracker.index];
        let id = match tracker.rowid {
            Some(id) => id,
            None => table.rows.last_key_value().map_or(Ok(1), |(n, _)| {
                n.checked_add(1).ok_or(Error::Unsupported(
                    "random sequence rowid allocation after i64::MAX",
                ))
            })?,
        };
        table.rows.insert(
            id,
            vec![
                Value::Text(Text::utf8(&tracker.name)),
                Value::Integer(tracker.maximum),
            ],
        );
        Ok(())
    }
    pub(super) fn remove_sequence(&mut self, name: &str, fuel: &mut Fuel) -> Result<()> {
        let index = self
            .index(NAME)
            .map_err(|_| Error::Corrupt("missing sqlite_sequence"))?;
        let table = &mut self.state.tables[index];
        let mut removed = Vec::new();
        for (id, values) in &table.rows {
            fuel.spend()?;
            if named(values, name) {
                removed.push(*id);
            }
        }
        for id in removed {
            table.rows.remove(&id);
        }
        Ok(())
    }
}
