//! Stored view definitions and materialized CREATE TABLE AS SELECT results.
use super::*;
mod keywords;

impl Connection {
    pub(super) fn view_index(&self, name: &str) -> Option<usize> {
        self.state
            .views
            .iter()
            .position(|v| v.name.eq_ignore_ascii_case(name))
    }
    pub(super) fn new_relation(&self, name: &str, if_not_exists: bool) -> Result<bool> {
        if name.to_ascii_lowercase().starts_with("sqlite_") {
            return Err(error("reserved schema name"));
        }
        if self.named_index(name).is_some() {
            return Err(error(format!("index {name} already exists")));
        }
        if self.index(name).is_ok() || self.view_index(name).is_some() {
            if if_not_exists {
                return Ok(false);
            }
            return Err(error(format!("table or view {name} already exists")));
        }
        Ok(true)
    }
    pub(super) fn create_as(&mut self, name: &str, data: query::Data) -> Result<()> {
        let fields = data.fields("");
        let affinities: Vec<_> = fields.iter().map(|f| f.affinity).collect();
        let columns: Vec<_> = fields
            .into_iter()
            .map(|field| {
                let typ = eval::affinity_type(field.affinity);
                Column {
                    location: Default::default(),
                    generated: None,
                    single_type_token: !typ.is_empty(),
                    name: field.name,
                    declared_type: typ.into(),
                    affinity: Affinity::from_type(typ),
                    collation: Collation::Binary,
                    collation_name: "BINARY".into(),
                    not_null: false,
                    not_null_conflict: Conflict::Default,
                    primary: false,
                    primary_desc: false,
                    unique: false,
                    primary_conflict: Conflict::Default,
                    unique_conflict: Conflict::Default,
                    index_desc: false,
                    default: None,
                    default_sql: None,
                    checks: Vec::new(),
                }
            })
            .collect();
        let sql = table_sql(name, &columns);
        let mut rows = BTreeMap::new();
        for (i, mut values) in data.result.rows.into_iter().enumerate() {
            for (value, affinity) in values.iter_mut().zip(&affinities) {
                *value = scalar::affinity(core::mem::replace(value, Value::Null), *affinity)?;
            }
            rows.insert(
                i64::try_from(i + 1).map_err(|_| Error::Limit("rowid"))?,
                values,
            );
        }
        self.state.tables.push(StoredTable {
            generated: None,
            name: name.into(),
            columns,
            sql,
            rows,
            indexes: Vec::new(),
            checks: Vec::new(),
            rowid_alias: None,
            primary_key: Vec::new(),
            key_conflict: Conflict::Default,
            autoincrement: false,
            strict: false,
            without_rowid: false,
        });
        Ok(())
    }
}
fn identifier(name: &str) -> String {
    if !name.is_empty()
        && !name.as_bytes()[0].is_ascii_digit()
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !keywords::KEYWORDS
            .iter()
            .any(|k| k.eq_ignore_ascii_case(name))
    {
        name.into()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}
fn table_sql(name: &str, columns: &[Column]) -> String {
    let estimate = |s: &str| s.len() + 2 + s.bytes().filter(|b| *b == b'"').count();
    let multiline =
        estimate(name) + columns.iter().map(|c| estimate(&c.name) + 5).sum::<usize>() >= 50;
    let mut sql = format!("CREATE TABLE {}(", identifier(name));
    for (i, column) in columns.iter().enumerate() {
        if i != 0 {
            sql.push(',');
        }
        if multiline {
            sql.push_str("\n  ");
        }
        sql.push_str(&identifier(&column.name));
        if !column.declared_type.is_empty() {
            sql.push(' ');
            sql.push_str(&column.declared_type);
        }
    }
    if multiline {
        sql.push('\n');
    }
    sql.push(')');
    sql
}
