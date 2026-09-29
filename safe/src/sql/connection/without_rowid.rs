//! Primary-key storage layouts and row identities for WITHOUT ROWID tables.
//! The row-map integers are private handles, not SQL rowids. Scans order rows
//! by the declared primary key; UPDATE captures keys and looks them up again
//! so a REPLACE that moves a row into a later selected key is visited correctly.
use super::*;

pub(super) fn declaration(table: &mut StoredTable, descending: bool) -> Result<()> {
    if !table.without_rowid {
        return Ok(());
    }
    if table.autoincrement {
        return Err(error("AUTOINCREMENT not allowed on WITHOUT ROWID tables"));
    }
    if table.primary_key.is_empty() {
        return Err(error("WITHOUT ROWID requires a PRIMARY KEY"));
    }
    // SQLite first builds the ordinary schema, then creates/coalesces a primary
    // index for what would otherwise have been an INTEGER PRIMARY KEY alias.
    if let Some(column) = table.rowid_alias.take() {
        let c = &table.columns[column];
        if let Some(index) = table.indexes.iter_mut().find(|index| {
            index.terms.len() == 1
                && index.terms[0].column == column
                && index.terms[0].collation == c.collation
        }) {
            index.primary = true;
            index.conflict = index.conflict.merge(table.key_conflict)?;
        } else {
            table.indexes.push(StoredIndex {
                name: format!(
                    "sqlite_autoindex_{}_{}",
                    table.name,
                    table.indexes.len() + 1
                ),
                sql: None,
                unique: true,
                primary: true,
                conflict: table.key_conflict,
                terms: vec![IndexTerm {
                    column,
                    collation: c.collation,
                    collation_name: c.collation_name.clone(),
                    descending,
                }],
            });
        }
    }
    let index = table
        .indexes
        .iter_mut()
        .find(|index| index.primary)
        .ok_or(Error::Corrupt("WITHOUT ROWID primary index missing"))?;
    let mut terms: Vec<IndexTerm> = Vec::new();
    for term in &index.terms {
        if !terms
            .iter()
            .any(|prior| prior.column == term.column && prior.collation == term.collation)
        {
            terms.push(term.clone());
        }
    }
    index.terms = terms;
    table.primary_key = index.terms.iter().map(|t| t.column).collect();
    for &i in &table.primary_key {
        if !table.columns[i].not_null {
            table.columns[i].not_null = true;
            table.columns[i].not_null_conflict = Conflict::Abort;
        }
    }
    Ok(())
}

impl StoredTable {
    pub(super) fn primary_index(&self) -> Result<&StoredIndex> {
        self.indexes
            .iter()
            .find(|index| index.primary)
            .ok_or(Error::Corrupt("WITHOUT ROWID primary index missing"))
    }
    /// Record columns, including payload/primary-key suffixes. Ordinary index
    /// rowid suffixes are represented separately by the caller.
    pub(super) fn storage_terms(&self, index: &StoredIndex) -> Result<Vec<IndexTerm>> {
        let mut terms = index.terms.clone();
        if !self.without_rowid {
            return Ok(terms);
        }
        if index.primary {
            for (column, c) in self.columns.iter().enumerate() {
                if !c.virtual_column() && !index.terms.iter().any(|t| t.column == column) {
                    terms.push(IndexTerm {
                        column,
                        collation: c.collation,
                        collation_name: c.collation_name.clone(),
                        descending: false,
                    });
                }
            }
        } else {
            for term in &self.primary_index()?.terms {
                if !index
                    .terms
                    .iter()
                    .any(|t| t.column == term.column && t.collation == term.collation)
                {
                    let mut term = term.clone();
                    // Automatic UNIQUE indexes retain SQLite's historical ASC
                    // suffix layout; explicit indexes copy the PK direction.
                    if index.sql.is_none() {
                        term.descending = false;
                    }
                    terms.push(term);
                }
            }
        }
        Ok(terms)
    }
    pub(super) fn scan_ids(&self, context: &mut Eval<'_>) -> Result<Vec<i64>> {
        let ids: Vec<_> = self.rows.keys().copied().collect();
        if !self.without_rowid {
            return Ok(ids);
        }
        let terms = &self.primary_index()?.terms;
        sort_by(ids, context.fuel, |a, b| {
            for term in terms {
                let cmp = scalar::compare_encoded(
                    &self.rows[a][term.column],
                    &self.rows[b][term.column],
                    term.collation,
                    context.encoding,
                )?;
                if cmp != Compare::Equal {
                    return Ok(if term.descending { cmp.reverse() } else { cmp });
                }
            }
            Ok(Compare::Equal)
        })
    }
    pub(super) fn primary_value(
        &self,
        values: &[Value],
        bytes: &mut usize,
        limits: SqlLimits,
    ) -> Result<Vec<Value>> {
        for i in &self.primary_key {
            *bytes = bytes
                .checked_add(scalar::size(&values[*i]) + core::mem::size_of::<Value>())
                .ok_or(Error::Limit("selected primary keys"))?;
        }
        if *bytes > limits.max_database_bytes {
            return Err(Error::Limit("selected primary keys"));
        }
        Ok(self
            .primary_key
            .iter()
            .map(|i| values[*i].clone())
            .collect())
    }
    pub(super) fn find_primary(
        &self,
        key: &[Value],
        context: &mut Eval<'_>,
    ) -> Result<Option<i64>> {
        let terms = &self.primary_index()?.terms;
        for (id, row) in &self.rows {
            let mut equal = true;
            for (term, value) in terms.iter().zip(key) {
                context.fuel.spend()?;
                if scalar::compare_encoded(
                    &row[term.column],
                    value,
                    term.collation,
                    context.encoding,
                )? != Compare::Equal
                {
                    equal = false;
                    break;
                }
            }
            if equal {
                return Ok(Some(*id));
            }
        }
        Ok(None)
    }
}
