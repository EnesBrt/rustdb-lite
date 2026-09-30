use super::{
    error,
    eval::{self, Eval, Field, Fuel},
    parser::{
        self, Column, Conflict, Expr, ExprKind, InsertSource, Ordering, PreparedStatement, Select,
        Statement, TableConstraint,
    },
    scalar::{self, Affinity, Collation},
    SqlLimits,
};
use crate::{Database, Encoding, Error, ImageBuilder, Result, Table, Text, Value};
use alloc::{collections::BTreeMap, format, string::String, vec, vec::Vec};
use core::cmp::Ordering as Compare;
mod catalog;
mod conflict;
mod generated;
mod indexes;
mod joins;
mod query;
mod returning;
mod schema;
mod sequence;
mod strict;
mod upsert;
mod without_rowid;

#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub changes: usize,
}
impl QueryResult {
    fn changed(n: usize) -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            changes: n,
        }
    }
}
#[derive(Clone)]
struct StoredTable {
    generated: Option<alloc::rc::Rc<eval::generated::Schema>>,
    name: String,
    columns: Vec<Column>,
    sql: String,
    rows: BTreeMap<i64, Vec<Value>>,
    indexes: Vec<StoredIndex>,
    checks: Vec<Expr>,
    rowid_alias: Option<usize>,
    primary_key: Vec<usize>,
    key_conflict: Conflict,
    autoincrement: bool,
    strict: bool,
    without_rowid: bool,
}
#[derive(Clone)]
struct StoredView {
    name: String,
    columns: Option<Vec<String>>,
    query: alloc::rc::Rc<parser::Query>,
    sql: String,
}
#[derive(Clone)]
struct IndexTerm {
    key: IndexKey,
    collation: Collation,
    collation_name: String,
    descending: bool,
}
#[derive(Clone, PartialEq)]
enum IndexKey {
    Column(usize),
    Expression(alloc::boxed::Box<IndexExpression>),
}
#[derive(Clone, PartialEq)]
struct IndexExpression {
    value: Expr,
    signature: Expr,
}
#[derive(Clone)]
struct StoredIndex {
    predicate: Option<IndexExpression>,
    name: String,
    sql: Option<String>,
    terms: Vec<IndexTerm>,
    unique: bool,
    primary: bool,
    conflict: Conflict,
}
impl StoredTable {
    fn fields(&self, alias: &str) -> Vec<Field> {
        let mut fields: Vec<_> =
            self.columns
                .iter()
                .enumerate()
                .map(|(i, c)| Field {
                    nested: None,
                    merged: Vec::new(),
                    generated: self.generated.as_ref().filter(|_| c.virtual_column()).map(
                        |schema| eval::generated::Reference {
                            schema: schema.clone(),
                            column: i,
                            offset: 0,
                            outer: None,
                        },
                    ),
                    table: alias.into(),
                    name: c.name.clone(),
                    affinity: c.affinity,
                    collation: c.collation,
                    hidden: false,
                    qualified_only: false,
                    unqualified_hidden: false,
                    declared_type: c.declared_type.clone(),
                })
                .collect();
        if !self.without_rowid {
            fields.push(Field {
                nested: None,
                merged: Vec::new(),
                generated: None,
                table: alias.into(),
                name: "rowid".into(),
                affinity: Affinity::Integer,
                collation: Collation::Binary,
                hidden: true,
                qualified_only: false,
                unqualified_hidden: false,
                declared_type: "INTEGER".into(),
            });
        }
        fields
    }
    fn row(&self, id: i64, values: &[Value]) -> Vec<Value> {
        let mut row = values.to_vec();
        if !self.without_rowid {
            row.push(Value::Integer(id));
        }
        row
    }
    fn column_index(&self, name: &str) -> Result<usize> {
        if let Some(n) = self
            .columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
        {
            return Ok(n);
        }
        if !self.without_rowid
            && ["rowid", "_rowid_", "oid"]
                .iter()
                .any(|n| name.eq_ignore_ascii_case(n))
        {
            return Ok(self.columns.len());
        }
        Err(error(format!(
            "table {} has no column named {name}",
            self.name
        )))
    }
    fn alias(&self) -> Option<usize> {
        self.rowid_alias.filter(|_| !self.without_rowid)
    }
}
#[derive(Clone, Default)]
struct State {
    tables: Vec<StoredTable>,
    views: Vec<StoredView>,
    user_version: u32,
    application_id: u32,
    encoding: Option<Encoding>,
    auto_vacuum: crate::AutoVacuum,
}
struct Savepoint {
    name: String,
    state: State,
}
/// A private, in-memory database. Transaction operations only roll back owned
/// memory; they do not imply filesystem locking, crash recovery, or durability.
pub struct Connection {
    state: State,
    limits: SqlLimits,
    transaction: Option<State>,
    savepoints: Vec<Savepoint>,
    savepoint_transaction: bool,
    changes: usize,
    total_changes: u64,
    last_rowid: i64,
    failure_action: Conflict,
    datatype_journal: bool,
    retained_datatype: bool,
    dml_evaluating: bool,
}
impl Default for Connection {
    fn default() -> Self {
        Self::new()
    }
}
impl Connection {
    pub fn new() -> Self {
        Self::with_limits(SqlLimits::default())
    }
    pub fn with_limits(limits: SqlLimits) -> Self {
        Self {
            state: State::default(),
            limits,
            transaction: None,
            savepoints: Vec::new(),
            savepoint_transaction: false,
            changes: 0,
            total_changes: 0,
            last_rowid: 0,
            failure_action: Conflict::Default,
            datatype_journal: true,
            retained_datatype: false,
            dml_evaluating: false,
        }
    }
    pub fn changes(&self) -> usize {
        self.changes
    }
    pub fn total_changes(&self) -> u64 {
        self.total_changes
    }
    pub fn last_insert_rowid(&self) -> i64 {
        self.last_rowid
    }
    pub fn is_autocommit(&self) -> bool {
        self.transaction.is_none()
    }
    pub(super) fn retained_failure_changes(&self) -> bool {
        (self.failure_action == Conflict::Fail && self.changes != 0) || self.retained_datatype
    }
    fn conflict_error(&mut self, policy: Conflict, message: String, count: usize) -> Error {
        self.failure_action = policy;
        if policy == Conflict::Fail {
            self.changes = count;
            self.total_changes = self.total_changes.saturating_add(count as u64);
        }
        Error::Constraint(message)
    }
    pub fn prepare(&self, sql: &str) -> Result<PreparedStatement> {
        let mut parsed = parser::parse(sql, self.limits)?;
        if parsed.len() != 1 {
            return Err(error("prepare requires exactly one statement"));
        }
        Ok(parsed.remove(0))
    }
    pub fn execute(&mut self, sql: &str, parameters: &[Value]) -> Result<QueryResult> {
        let prepared = self.prepare(sql)?;
        self.execute_prepared(&prepared, parameters)
    }
    /// Parse a script before running its statements in order. Runtime errors
    /// abort the statement unless a constraint selects FAIL (retain its prefix)
    /// or ROLLBACK (undo the transaction). A datatype error in a transaction
    /// can retain a prefix without counting changes when no statement journal
    /// was required by the constraints. Execution stops at the first error.
    /// A syntax error anywhere prevents execution of the entire script.
    pub fn execute_batch(&mut self, sql: &str) -> Result<Vec<QueryResult>> {
        let parsed = parser::parse(sql, self.limits)?;
        let mut results = Vec::new();
        let mut bytes = 0usize;
        let mut row_count = 0usize;
        for prepared in parsed {
            let result = self.execute_prepared(&prepared, &[])?;
            for name in &result.columns {
                bytes = bytes
                    .checked_add(name.len())
                    .ok_or(Error::Limit("batch result bytes"))?;
            }
            row_count = row_count
                .checked_add(result.rows.len())
                .ok_or(Error::Limit("batch result rows"))?;
            for row in &result.rows {
                bytes = bytes
                    .checked_add(values_size(row)?)
                    .ok_or(Error::Limit("batch result bytes"))?;
            }
            if bytes > self.limits.max_database_bytes || row_count > self.limits.max_rows {
                return Err(Error::Limit("batch result rows/bytes"));
            }
            results.push(result);
        }
        Ok(results)
    }
    pub fn execute_prepared(
        &mut self,
        prepared: &PreparedStatement,
        parameters: &[Value],
    ) -> Result<QueryResult> {
        self.failure_action = Conflict::Default;
        self.datatype_journal = true;
        self.retained_datatype = false;
        self.dml_evaluating = false;
        if parameters.len() > prepared.parameter_count() {
            return Err(error("too many bound parameters"));
        }
        let mut normalized = Vec::new();
        let mut bytes = 0usize;
        for value in parameters {
            if scalar::size(value) > self.limits.max_value_bytes {
                return Err(Error::Limit("SQL value size"));
            }
            let value = normalize(value.clone())?;
            if scalar::size(&value) > self.limits.max_value_bytes {
                return Err(Error::Limit("SQL value size"));
            }
            push_value(&mut normalized, value, &mut bytes, self.limits)?;
        }
        let parameters = normalized;
        let mut fuel = Fuel::new(self.limits.max_steps);
        let mut context = Eval {
            encoding: self.state.encoding.unwrap_or(Encoding::Utf8),
            params: &parameters,
            limits: self.limits,
            fuel: &mut fuel,
            changes: self.changes,
            total_changes: self.total_changes,
            last_rowid: self.last_rowid,
        };
        let mutating = prepared.statement.mutating();
        let backup = if mutating {
            Some(self.state.clone())
        } else {
            None
        };
        let previous_total_changes = self.total_changes;
        let mut result = self.run(
            &prepared.statement,
            &mut context,
            None,
            &mut query::Runtime::default(),
        );
        if matches!(result, Err(Error::Full)) {
            self.failure_action = Conflict::Rollback;
        }
        self.retained_datatype = matches!(result, Err(Error::Datatype(_)))
            && !self.datatype_journal
            && !self.is_autocommit();
        if result.is_ok() || self.failure_action == Conflict::Fail || self.retained_datatype {
            if let Err(error) = self.check_budget() {
                self.failure_action = Conflict::Abort;
                self.retained_datatype = false;
                result = Err(error);
            }
        }
        if result.is_err() && self.failure_action != Conflict::Fail {
            self.total_changes = previous_total_changes;
            if self.failure_action == Conflict::Rollback {
                self.state = self
                    .transaction
                    .take()
                    .or(backup)
                    .unwrap_or_else(|| self.state.clone());
                self.savepoints.clear();
                self.savepoint_transaction = false;
            } else if let Some(backup) = backup.filter(|_| !self.retained_datatype) {
                self.state = backup;
            }
            if prepared.statement.changes_rows() && self.dml_evaluating {
                self.changes = 0;
            }
        }
        result
    }
    fn index(&self, name: &str) -> Result<usize> {
        self.state
            .tables
            .iter()
            .position(|t| t.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| error(format!("no such table: {name}")))
    }
    fn named_index(&self, name: &str) -> Option<(usize, usize)> {
        self.state.tables.iter().enumerate().find_map(|(t, table)| {
            table
                .indexes
                .iter()
                .position(|i| i.name.eq_ignore_ascii_case(name))
                .map(|i| (t, i))
        })
    }
    fn usage(&self) -> Result<(usize, usize)> {
        let mut bytes = 0usize;
        let mut rows = 0usize;
        for view in &self.state.views {
            bytes = bytes
                .checked_add(view.sql.len() + view.name.len())
                .ok_or(Error::Limit("database bytes"))?;
        }
        for table in &self.state.tables {
            bytes = bytes
                .checked_add(table.sql.len() + table.name.len())
                .ok_or(Error::Limit("database bytes"))?;
            for index in &table.indexes {
                bytes = bytes
                    .checked_add(
                        index.name.len()
                            + index.sql.as_ref().map_or(0, String::len)
                            + index.terms.len() * core::mem::size_of::<IndexTerm>(),
                    )
                    .ok_or(Error::Limit("database bytes"))?;
                for term in &index.terms {
                    bytes = bytes
                        .checked_add(term.collation_name.len())
                        .ok_or(Error::Limit("database bytes"))?;
                }
            }
            rows = rows
                .checked_add(table.rows.len())
                .ok_or(Error::Limit("database rows"))?;
            for values in table.rows.values() {
                for v in values {
                    bytes = bytes
                        .checked_add(scalar::size(v) + core::mem::size_of::<Value>())
                        .ok_or(Error::Limit("database bytes"))?;
                }
            }
        }
        Ok((bytes, rows))
    }
    fn check_budget(&self) -> Result<()> {
        let (bytes, rows) = self.usage()?;
        if bytes > self.limits.max_database_bytes {
            return Err(Error::Limit("database bytes"));
        }
        if rows > self.limits.max_rows {
            return Err(Error::Limit("database rows"));
        }
        if self.state.tables.len() + self.state.views.len() > 1024 {
            return Err(Error::Limit("table/view count"));
        }
        Ok(())
    }
    fn run(
        &mut self,
        statement: &Statement,
        context: &mut Eval<'_>,
        scope: Option<alloc::rc::Rc<query::Scope>>,
        runtime: &mut query::Runtime,
    ) -> Result<QueryResult> {
        let returned;
        let changes = match statement {
            Statement::CreateView {
                name,
                columns,
                query,
                sql,
                if_not_exists,
            } => {
                if !self.new_relation(name, *if_not_exists)? {
                    return Ok(QueryResult::changed(0));
                }
                self.state.views.push(StoredView {
                    name: name.clone(),
                    columns: columns.clone(),
                    query: query.clone(),
                    sql: sql.clone(),
                });
                return Ok(QueryResult::changed(0));
            }
            Statement::DropView { name, if_exists } => {
                if self.index(name).is_ok() {
                    return Err(error(format!("use DROP TABLE for {name}")));
                }
                if let Some(i) = self.view_index(name) {
                    self.state.views.remove(i);
                } else if !if_exists {
                    return Err(error(format!("no such view: {name}")));
                }
                return Ok(QueryResult::changed(0));
            }
            Statement::CreateAs {
                name,
                query,
                if_not_exists,
            } => {
                if !self.new_relation(name, *if_not_exists)? {
                    return Ok(QueryResult::changed(0));
                }
                let data = self.query(query, context, scope, runtime, false)?;
                self.create_as(name, data)?;
                return Ok(QueryResult::changed(0));
            }
            Statement::With { tables, statement } => {
                let scope = query::Scope::extend(scope, tables, runtime)?;
                return self.run(statement, context, scope, runtime);
            }
            Statement::Select(query) => {
                return Ok(self.query(query, context, scope, runtime, false)?.result)
            }
            Statement::CreateIndex {
                name,
                table,
                columns,
                predicate,
                unique,
                if_not_exists,
                sql,
            } => {
                if self.index(name).is_ok() || self.view_index(name).is_some() {
                    return Err(error(format!("table {name} already exists")));
                }
                if self.named_index(name).is_some() {
                    if *if_not_exists {
                        return Ok(QueryResult::changed(0));
                    }
                    return Err(error(format!("index {name} already exists")));
                }
                if name.to_ascii_lowercase().starts_with("sqlite_") {
                    return Err(error("invalid or reserved index name"));
                }
                if columns.len() > 2000 {
                    return Err(Error::Limit("index columns"));
                }
                let table_id = self.index(table)?;
                if table.eq_ignore_ascii_case(sequence::NAME) {
                    return Err(error("system tables may not be indexed"));
                }
                let table = &mut self.state.tables[table_id];
                if table.indexes.len() >= 2000 {
                    return Err(Error::Limit("indexes per table"));
                }
                let index = indexes::declaration(
                    table,
                    name,
                    columns,
                    predicate.as_ref(),
                    *unique,
                    sql,
                    context.fuel,
                )?;
                index_entries(
                    table,
                    &index,
                    context.limits,
                    context.fuel,
                    context.encoding,
                )?;
                table.indexes.push(index);
                return Ok(QueryResult::changed(0));
            }
            Statement::DropIndex { name, if_exists } => {
                match self.named_index(name) {
                    Some((t, i)) => {
                        if self.state.tables[t].indexes[i].sql.is_none() {
                            return Err(error("index associated with UNIQUE or PRIMARY KEY constraint cannot be dropped"));
                        }
                        self.state.tables[t].indexes.remove(i);
                    }
                    None if *if_exists => {}
                    None => return Err(error(format!("no such index: {name}"))),
                }
                return Ok(QueryResult::changed(0));
            }
            Statement::Create {
                name,
                columns,
                constraints,
                autoincrement,
                strict,
                without_rowid,
                if_not_exists,
                sql,
            } => {
                if name.eq_ignore_ascii_case(sequence::NAME) {
                    return Err(error("invalid or reserved table name"));
                }
                if self.named_index(name).is_some() {
                    return Err(error(format!("index {name} already exists")));
                }
                if self.index(name).is_ok() || self.view_index(name).is_some() {
                    if *if_not_exists {
                        return Ok(QueryResult::changed(0));
                    }
                    return Err(error(format!("table {name} already exists")));
                }
                if name.to_ascii_lowercase().starts_with("sqlite_") {
                    return Err(error("invalid or reserved table name"));
                }
                if columns.iter().filter(|c| c.primary).count()
                    + constraints
                        .iter()
                        .filter(|c| matches!(c, TableConstraint::Key { primary: true, .. }))
                        .count()
                    > 1
                {
                    return Err(error("multiple primary keys"));
                }
                if columns.len() > 2000 {
                    return Err(Error::Limit("table columns"));
                }
                let mut table = StoredTable {
                    generated: None,
                    name: name.clone(),
                    columns: columns.clone(),
                    sql: sql.clone(),
                    rows: BTreeMap::new(),
                    indexes: Vec::new(),
                    checks: Vec::new(),
                    rowid_alias: columns.iter().position(Column::rowid_alias),
                    primary_key: columns
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| c.primary)
                        .map(|(i, _)| i)
                        .collect(),
                    key_conflict: columns
                        .iter()
                        .find(|c| c.rowid_alias())
                        .map_or(Conflict::Default, |c| c.primary_conflict),
                    autoincrement: *autoincrement,
                    strict: *strict,
                    without_rowid: *without_rowid,
                };
                let mut primary_desc = false;
                strict::declaration(&mut table)?;
                let fields = table.fields(name);
                for (i, column) in columns.iter().enumerate() {
                    if columns[..i]
                        .iter()
                        .any(|c| c.name.eq_ignore_ascii_case(&column.name))
                    {
                        return Err(error(format!("duplicate column name: {}", column.name)));
                    }
                    if let Some(default) = &column.default {
                        if contains_parameter(default) {
                            return Err(error("parameters prohibited in schema"));
                        }
                        eval::bind(default, &[], &[], false)?;
                    }
                    for check in &column.checks {
                        if contains_parameter(check) {
                            return Err(error("parameters prohibited in schema"));
                        }
                        eval::bind(check, &fields, &[], false)?;
                    }
                    if column.unique || (column.primary && !column.rowid_alias()) {
                        table.indexes.push(StoredIndex {
                            predicate: None,
                            name: format!("sqlite_autoindex_{}_{}", name, table.indexes.len() + 1),
                            sql: None,
                            unique: true,
                            primary: column.primary && !column.rowid_alias(),
                            conflict: if column.primary && !column.rowid_alias() {
                                column.primary_conflict.merge(column.unique_conflict)?
                            } else {
                                column.unique_conflict
                            },
                            terms: vec![IndexTerm {
                                key: IndexKey::Column(i),
                                collation: column.collation,
                                collation_name: column.collation_name.clone(),
                                descending: column.index_desc,
                            }],
                        });
                    }
                }
                for constraint in constraints {
                    match constraint {
                        TableConstraint::Check(check) => {
                            if contains_parameter(check) {
                                return Err(error("parameters prohibited in schema"));
                            }
                            eval::bind(check, &fields, &[], false)?;
                            table.checks.push(check.clone());
                        }
                        TableConstraint::Key {
                            columns: keys,
                            primary,
                            conflict,
                        } => {
                            let terms = keys
                                .iter()
                                .map(|key| {
                                    let column = table
                                        .columns
                                        .iter()
                                        .position(|c| c.name.eq_ignore_ascii_case(&key.name))
                                        .ok_or_else(|| {
                                            error(format!("no such column: {}", key.name))
                                        })?;
                                    Ok(IndexTerm {
                                        key: IndexKey::Column(column),
                                        collation: key
                                            .collation
                                            .unwrap_or(table.columns[column].collation),
                                        collation_name: key
                                            .collation_name
                                            .as_ref()
                                            .unwrap_or(&table.columns[column].collation_name)
                                            .clone(),
                                        descending: key.descending,
                                    })
                                })
                                .collect::<Result<Vec<_>>>()?;
                            if *primary {
                                table.primary_key =
                                    terms.iter().map(IndexTerm::column).collect::<Result<_>>()?;
                            }
                            // Unlike the column form INTEGER PRIMARY KEY DESC,
                            // a single-column table PRIMARY KEY aliases rowid
                            // even when its indexed-column sort order is DESC.
                            if *primary
                                && terms.len() == 1
                                && table.columns[terms[0].column()?].single_type_token
                                && table.columns[terms[0].column()?]
                                    .declared_type
                                    .eq_ignore_ascii_case("INTEGER")
                            {
                                table.rowid_alias = Some(terms[0].column()?);
                                table.key_conflict = *conflict;
                                primary_desc = terms[0].descending;
                                continue;
                            }
                            // SQLite coalesces automatic constraints with equal
                            // column/collation sequences, ignoring sort order.
                            if let Some(index) =
                                table.indexes.iter_mut().find(|index| {
                                    index.terms.len() == terms.len()
                                        && index.terms.iter().zip(&terms).all(|(a, b)| {
                                            a.key == b.key && a.collation == b.collation
                                        })
                                })
                            {
                                index.primary |= *primary;
                                index.conflict = index.conflict.merge(*conflict)?;
                                continue;
                            }
                            if table.indexes.len() >= 2000 {
                                return Err(Error::Limit("indexes per table"));
                            }
                            table.indexes.push(StoredIndex {
                                predicate: None,
                                name: format!(
                                    "sqlite_autoindex_{}_{}",
                                    name,
                                    table.indexes.len() + 1
                                ),
                                sql: None,
                                unique: true,
                                primary: *primary,
                                conflict: *conflict,
                                terms,
                            });
                        }
                    }
                }
                without_rowid::declaration(&mut table, primary_desc)?;
                if table.autoincrement && table.alias().is_none() {
                    return Err(error("AUTOINCREMENT requires an INTEGER PRIMARY KEY"));
                }
                strict::primary_key(&mut table);
                generated::declaration(&mut table, context.fuel)?;
                self.state.tables.push(table);
                if *autoincrement {
                    self.ensure_sequence()?;
                }
                return Ok(QueryResult::changed(0));
            }
            Statement::Drop { name, if_exists } => {
                if name.eq_ignore_ascii_case(sequence::NAME) && self.index(name).is_ok() {
                    return Err(error("sqlite_sequence may not be dropped"));
                }
                if self.view_index(name).is_some() {
                    return Err(error(format!("use DROP VIEW for {name}")));
                }
                match self.index(name) {
                    Ok(i) => {
                        if self.state.tables[i].autoincrement {
                            let name = self.state.tables[i].name.clone();
                            self.remove_sequence(&name, context.fuel)?;
                        }
                        self.state.tables.remove(i);
                    }
                    Err(_) if *if_exists => {}
                    Err(e) => return Err(e),
                }
                return Ok(QueryResult::changed(0));
            }
            Statement::Insert {
                name,
                alias,
                columns,
                source,
                conflict,
                upserts,
                returning,
            } => {
                let index = self.index(name)?;
                let table = &self.state.tables[index];
                let mut output = returning::Output::bind(
                    self,
                    table,
                    returning,
                    scope.clone(),
                    runtime,
                    context,
                )?;
                let upserts = self.bind_upserts(
                    table,
                    alias.as_deref().unwrap_or(name),
                    upserts,
                    scope.clone(),
                    runtime,
                    context,
                )?;
                table.generated_plan()?;
                let mut autoincrement = self.start_sequence(index, context.fuel)?;
                let destinations = if let Some(columns) = columns {
                    columns
                        .iter()
                        .map(|c| table.write_column(c))
                        .collect::<Result<Vec<_>>>()?
                } else {
                    table
                        .columns
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| c.generated.is_none())
                        .map(|(i, _)| i)
                        .collect()
                };
                if destinations
                    .iter()
                    .enumerate()
                    .any(|(i, n)| destinations[..i].contains(n))
                {
                    return Err(error("duplicate INSERT column"));
                }
                let input: Vec<InputRow> = match source {
                    InsertSource::Default => vec![InputRow::Default],
                    InsertSource::Values(rows) => rows
                        .iter()
                        .map(|row| {
                            row.iter()
                                .map(|e| {
                                    self.expressions(scope.clone(), runtime).bind(
                                        e,
                                        &[],
                                        &[],
                                        false,
                                        context,
                                    )
                                })
                                .collect::<Result<Vec<_>>>()
                                .map(InputRow::Expressions)
                        })
                        .collect::<Result<_>>()?,
                    InsertSource::Select(query) => {
                        self.dml_evaluating = true;
                        self.query(query, context, scope.clone(), runtime, false)?
                            .result
                            .rows
                            .into_iter()
                            .map(InputRow::Values)
                            .collect()
                    }
                };
                // Binding/target errors leave the previous changes() count;
                // evaluation and constraint errors have statement semantics.
                self.dml_evaluating = true;
                if input.len() > self.limits.max_rows {
                    return Err(Error::Limit("insert rows"));
                }
                fn reads_target(expr: &Expr, name: &str) -> bool {
                    matches!(&expr.kind, ExprKind::BoundSubquery(q) if q.tables.iter().any(|t| t.eq_ignore_ascii_case(name)))
                        || expr.children().iter().any(|e| reads_target(e, name))
                }
                let reads_target = input.iter().any(|row| matches!(row, InputRow::Expressions(expr) if expr.iter().any(|e| reads_target(e, name))));
                // SQLite snapshots VALUES input when a subquery reads the
                // destination, so later input rows cannot see earlier inserts.
                let input = if reads_target {
                    let mut materialized = Vec::new();
                    let mut bytes = 0usize;
                    for row in input {
                        let row = match row {
                            InputRow::Expressions(expr) => self
                                .expressions(scope.clone(), runtime)
                                .values(expr.iter(), &[], None, context)
                                .map(InputRow::Values)?,
                            row => row,
                        };
                        if let InputRow::Values(values) = &row {
                            bytes = bytes
                                .checked_add(values_size(values)?)
                                .ok_or(Error::Limit("insert input bytes"))?;
                            if bytes > context.limits.max_database_bytes {
                                return Err(Error::Limit("insert input bytes"));
                            }
                        }
                        materialized.push(row);
                    }
                    materialized
                } else {
                    input
                };
                let (mut database_bytes, mut database_rows) = self.usage()?;
                let table = &self.state.tables[index];
                let defaults = defaults(table)?;
                let checks = checks(table)?;
                let change = conflict::Change {
                    columns: None,
                    rowid: !matches!(source, InsertSource::Default)
                        && destinations
                            .iter()
                            .any(|i| *i == table.columns.len() || Some(*i) == table.alias()),
                };
                self.datatype_journal =
                    strict::journal(table, change, *conflict, &checks, &upserts, context.fuel)?;
                let mut count = 0;
                for input in input {
                    context.fuel.spend()?;
                    let default_row = matches!(input, InputRow::Default);
                    let input = match input {
                        InputRow::Default => Vec::new(),
                        InputRow::Values(values) => values,
                        InputRow::Expressions(expr) => self
                            .expressions(scope.clone(), runtime)
                            .values(expr.iter(), &[], None, context)?,
                    };
                    let table = &self.state.tables[index];
                    if !default_row && input.len() != destinations.len() {
                        return Err(error("INSERT value count does not match columns"));
                    }
                    let mut values = Vec::new();
                    let mut row_bytes = 0;
                    for (index, default) in defaults.iter().enumerate() {
                        let value = if Some(index) != table.alias()
                            && (default_row || !destinations.contains(&index))
                        {
                            default
                                .as_ref()
                                .map(|d| context.eval(d, &[], None))
                                .transpose()?
                                .unwrap_or(Value::Null)
                        } else {
                            Value::Null
                        };
                        push_value(&mut values, value, &mut row_bytes, context.limits)?;
                    }
                    let mut id = Value::Null;
                    for (destination, value) in destinations.iter().zip(input) {
                        if *destination == values.len() {
                            id = value;
                        } else {
                            values[*destination] = value;
                        }
                    }
                    if let Some(alias) = table.alias() {
                        if !scalar::null(&values[alias]) {
                            if !scalar::null(&id) {
                                return Err(error("rowid assigned twice"));
                            }
                            id = values[alias].clone();
                        }
                    }
                    let id = if scalar::null(&id) {
                        let largest = table.rows.last_key_value().map(|(n, _)| *n);
                        if let Some(sequence) = &autoincrement {
                            sequence.allocate(largest)?
                        } else {
                            largest.map_or(Ok(1), |n| {
                                n.checked_add(1).ok_or(Error::Unsupported(
                                    "random rowid allocation after i64::MAX",
                                ))
                            })?
                        }
                    } else {
                        rowid(id)?
                    };
                    if let Some(sequence) = &mut autoincrement {
                        sequence.step(id);
                    }
                    if let Some(alias) = table.alias() {
                        values[alias] = Value::Integer(id);
                    }
                    for (value, column) in values.iter_mut().zip(&table.columns) {
                        *value = scalar::affinity(
                            core::mem::replace(value, Value::Null),
                            column.affinity,
                        )?;
                    }
                    table.compute_generated(&mut values, context)?;
                    let replaced = match conflict::check(
                        table,
                        conflict::Row {
                            id,
                            values: &mut values,
                            old: None,
                            change,
                        },
                        *conflict,
                        &checks,
                        &defaults,
                        &upserts,
                        context,
                    )? {
                        conflict::Decision::Ignore => continue,
                        conflict::Decision::Error(policy, message) => {
                            return Err(self.conflict_error(policy, message, count))
                        }
                        conflict::Decision::Write(ids) => ids,
                        conflict::Decision::Upsert { clause, id: old_id } => {
                            if let Some((id, values)) = self.upsert_row(
                                table,
                                &upserts[clause],
                                upsert::Incoming {
                                    old_id,
                                    id,
                                    values: &values,
                                },
                                scope.clone(),
                                runtime,
                                context,
                            )? {
                                database_bytes = database_bytes
                                    .checked_sub(values_size(&table.rows[&old_id])?)
                                    .and_then(|n| n.checked_add(values_size(&values).ok()?))
                                    .ok_or(Error::Limit("UPSERT database bytes"))?;
                                if database_bytes > context.limits.max_database_bytes {
                                    return Err(Error::Limit("database bytes"));
                                }
                                let table = &mut self.state.tables[index];
                                table.rows.remove(&old_id);
                                table.rows.insert(id, values);
                                count += 1;
                                output.site(clause + 1);
                                output.emit(
                                    self,
                                    id,
                                    &self.state.tables[index].rows[&id],
                                    runtime,
                                    context,
                                )?;
                                output.site(0);
                            }
                            continue;
                        }
                    };
                    for id in &replaced {
                        database_bytes = database_bytes
                            .checked_sub(values_size(&table.rows[id])?)
                            .ok_or(Error::Limit("database bytes"))?;
                    }
                    database_rows -= replaced.len();
                    database_bytes = database_bytes
                        .checked_add(values_size(&values)?)
                        .ok_or(Error::Limit("database bytes"))?;
                    database_rows = database_rows
                        .checked_add(1)
                        .ok_or(Error::Limit("database rows"))?;
                    if database_bytes > context.limits.max_database_bytes
                        || database_rows > context.limits.max_rows
                    {
                        return Err(Error::Limit("database rows/bytes"));
                    }
                    let table = &mut self.state.tables[index];
                    for id in replaced {
                        table.rows.remove(&id);
                    }
                    table.rows.insert(id, values);
                    if !table.without_rowid {
                        self.last_rowid = id;
                        context.last_rowid = id;
                    }
                    count += 1;
                    if table.rows.len() > self.limits.max_rows {
                        return Err(Error::Limit("table rows"));
                    }
                    output.emit(
                        self,
                        id,
                        &self.state.tables[index].rows[&id],
                        runtime,
                        context,
                    )?;
                }
                if let Some(sequence) = autoincrement {
                    self.finish_sequence(sequence)?;
                }
                returned = output.finish(count);
                count
            }
            Statement::Update {
                name,
                alias,
                assignments,
                filter,
                conflict,
                returning,
            } => {
                let index = self.index(name)?;
                let (mut database_bytes, _) = self.usage()?;
                let table = &self.state.tables[index];
                let mut output = returning::Output::bind(
                    self,
                    table,
                    returning,
                    scope.clone(),
                    runtime,
                    context,
                )?;
                table.generated_plan()?;
                let fields = table.fields(alias.as_deref().unwrap_or(name));
                let assignments = assignments
                    .iter()
                    .map(|(name, expr)| {
                        Ok((
                            table.write_column(name)?,
                            self.expressions(scope.clone(), runtime).bind(
                                expr,
                                &fields,
                                &[],
                                false,
                                context,
                            )?,
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let filter = filter
                    .as_ref()
                    .map(|e| {
                        self.expressions(scope.clone(), runtime).bind(
                            e,
                            &fields,
                            &[],
                            false,
                            context,
                        )
                    })
                    .transpose()?;
                let checks = checks(table)?;
                let defaults = defaults(table)?;
                let columns = table
                    .changed_columns(assignments.iter().map(|(i, _)| *i).collect(), context.fuel)?;
                let change = conflict::Change::update(table, &columns);
                self.datatype_journal =
                    strict::journal(table, change, *conflict, &checks, &[], context.fuel)?;
                let mut selected = Vec::new();
                let mut selected_bytes = 0;
                self.dml_evaluating = true;
                for id in table.scan_ids(context)? {
                    let values = &table.rows[&id];
                    context.fuel.spend()?;
                    if self.expressions(scope.clone(), runtime).filter(
                        filter.as_ref(),
                        &table.row(id, values),
                        context,
                    )? {
                        selected.push((
                            id,
                            if table.without_rowid {
                                table.primary_value(values, &mut selected_bytes, context.limits)?
                            } else {
                                Vec::new()
                            },
                        ));
                    }
                }
                let mut count = 0;
                for (mut old_id, key) in selected {
                    context.fuel.spend()?;
                    let table = &self.state.tables[index];
                    if table.without_rowid {
                        let Some(id) = table.find_primary(&key, context)? else {
                            continue;
                        };
                        old_id = id;
                    }
                    // An earlier REPLACE may have deleted a selected row or
                    // moved another row into its rowid. Read its current value.
                    let Some(old_values) = table.rows.get(&old_id).cloned() else {
                        continue;
                    };
                    let row = table.row(old_id, &old_values);
                    let old_bytes = values_size(&old_values)?;
                    let mut values = old_values;
                    let mut id = old_id;
                    for (column, expr) in &assignments {
                        let value = self
                            .expressions(scope.clone(), runtime)
                            .eval(expr, &row, None, context)?;
                        if *column == values.len() || Some(*column) == table.alias() {
                            id = rowid(value)?;
                        } else {
                            values[*column] = value;
                        }
                        if values_size(&values)? > context.limits.max_database_bytes {
                            return Err(Error::Limit("updated row bytes"));
                        }
                    }
                    if let Some(alias) = table.alias() {
                        values[alias] = Value::Integer(id);
                    }
                    for (value, column) in values.iter_mut().zip(&table.columns) {
                        *value = scalar::affinity(
                            core::mem::replace(value, Value::Null),
                            column.affinity,
                        )?;
                    }
                    table.compute_generated(&mut values, context)?;
                    let replaced = match conflict::check(
                        table,
                        conflict::Row {
                            id,
                            values: &mut values,
                            old: Some(old_id),
                            change,
                        },
                        *conflict,
                        &checks,
                        &defaults,
                        &[],
                        context,
                    )? {
                        conflict::Decision::Ignore => continue,
                        conflict::Decision::Error(policy, message) => {
                            return Err(self.conflict_error(policy, message, count))
                        }
                        conflict::Decision::Write(ids) => ids,
                        conflict::Decision::Upsert { .. } => {
                            return Err(Error::Corrupt("UPSERT during ordinary UPDATE"))
                        }
                    };
                    for id in &replaced {
                        database_bytes = database_bytes
                            .checked_sub(values_size(&table.rows[id])?)
                            .ok_or(Error::Limit("database bytes"))?;
                    }
                    database_bytes = database_bytes
                        .checked_sub(old_bytes)
                        .and_then(|n| n.checked_add(values_size(&values).ok()?))
                        .ok_or(Error::Limit("database bytes"))?;
                    if database_bytes > context.limits.max_database_bytes {
                        return Err(Error::Limit("database bytes"));
                    }
                    let table = &mut self.state.tables[index];
                    for id in replaced {
                        table.rows.remove(&id);
                    }
                    table.rows.remove(&old_id);
                    table.rows.insert(id, values);
                    count += 1;
                    output.emit(
                        self,
                        id,
                        &self.state.tables[index].rows[&id],
                        runtime,
                        context,
                    )?;
                }
                returned = output.finish(count);
                count
            }
            Statement::Delete {
                name,
                alias,
                filter,
                returning,
            } => {
                let index = self.index(name)?;
                let table = &self.state.tables[index];
                let mut output = returning::Output::bind(
                    self,
                    table,
                    returning,
                    scope.clone(),
                    runtime,
                    context,
                )?;
                let fields = table.fields(alias.as_deref().unwrap_or(name));
                let filter = filter
                    .as_ref()
                    .map(|e| {
                        self.expressions(scope.clone(), runtime).bind(
                            e,
                            &fields,
                            &[],
                            false,
                            context,
                        )
                    })
                    .transpose()?;
                let mut deleted = Vec::new();
                self.dml_evaluating = true;
                for id in table.scan_ids(context)? {
                    let values = &table.rows[&id];
                    context.fuel.spend()?;
                    if self.expressions(scope.clone(), runtime).filter(
                        filter.as_ref(),
                        &table.row(id, values),
                        context,
                    )? {
                        deleted.push(id);
                    }
                }
                for id in &deleted {
                    let values = self.state.tables[index]
                        .rows
                        .remove(id)
                        .ok_or(Error::Corrupt("deleted row missing"))?;
                    output.emit(self, *id, &values, runtime, context)?;
                }
                returned = output.finish(deleted.len());
                deleted.len()
            }
            Statement::Begin => {
                if self.transaction.is_some() {
                    return Err(error("cannot start a transaction within a transaction"));
                }
                self.transaction = Some(self.state.clone());
                self.savepoint_transaction = false;
                return Ok(QueryResult::changed(0));
            }
            Statement::Commit => {
                if self.transaction.take().is_none() {
                    return Err(error("cannot commit: no transaction is active"));
                }
                self.savepoints.clear();
                self.savepoint_transaction = false;
                return Ok(QueryResult::changed(0));
            }
            Statement::Rollback(None) => {
                self.state = self
                    .transaction
                    .take()
                    .ok_or_else(|| error("cannot rollback: no transaction is active"))?;
                self.savepoints.clear();
                self.savepoint_transaction = false;
                return Ok(QueryResult::changed(0));
            }
            Statement::Savepoint(name) => {
                if self.savepoints.len() >= self.limits.max_savepoints {
                    return Err(Error::Limit("savepoints"));
                }
                if self.transaction.is_none() {
                    self.transaction = Some(self.state.clone());
                    self.savepoint_transaction = true;
                }
                self.savepoints.push(Savepoint {
                    name: name.clone(),
                    state: self.state.clone(),
                });
                return Ok(QueryResult::changed(0));
            }
            Statement::Rollback(Some(name)) => {
                let i = self
                    .savepoints
                    .iter()
                    .rposition(|s| s.name.eq_ignore_ascii_case(name))
                    .ok_or_else(|| error(format!("no such savepoint: {name}")))?;
                self.state = self.savepoints[i].state.clone();
                self.savepoints.truncate(i + 1);
                return Ok(QueryResult::changed(0));
            }
            Statement::Release(name) => {
                let i = self
                    .savepoints
                    .iter()
                    .rposition(|s| s.name.eq_ignore_ascii_case(name))
                    .ok_or_else(|| error(format!("no such savepoint: {name}")))?;
                self.savepoints.truncate(i);
                if i == 0 && self.savepoint_transaction {
                    self.transaction = None;
                    self.savepoint_transaction = false;
                }
                return Ok(QueryResult::changed(0));
            }
            Statement::Pragma {
                schema,
                name,
                value,
                argument,
            } => {
                if name.eq_ignore_ascii_case("table_list") {
                    return self.table_list(schema.as_deref(), argument.as_deref(), context);
                }
                if value.is_none() {
                    if let Some(result) = self.metadata(name, argument.as_deref(), context)? {
                        return Ok(result);
                    }
                }
                if argument.is_none() && value.is_none() && name.eq_ignore_ascii_case("auto_vacuum")
                {
                    let mode = match self.state.auto_vacuum {
                        crate::AutoVacuum::None => 0,
                        crate::AutoVacuum::Full => 1,
                        crate::AutoVacuum::Incremental => 2,
                    };
                    return Ok(QueryResult {
                        columns: vec!["auto_vacuum".into()],
                        rows: vec![vec![Value::Integer(mode)]],
                        changes: 0,
                    });
                }
                if argument.is_none()
                    && (name.eq_ignore_ascii_case("user_version")
                        || name.eq_ignore_ascii_case("application_id"))
                {
                    let (name, field) = if name.eq_ignore_ascii_case("user_version") {
                        ("user_version", &mut self.state.user_version)
                    } else {
                        ("application_id", &mut self.state.application_id)
                    };
                    if let Some(value) = value {
                        *field = i32::try_from(*value).unwrap_or(0) as u32;
                        return Ok(QueryResult::changed(0));
                    }
                    return Ok(QueryResult {
                        columns: vec![name.into()],
                        rows: vec![vec![Value::Integer(i64::from(*field as i32))]],
                        changes: 0,
                    });
                }
                return Err(Error::Unsupported("PRAGMA"));
            }
        };
        self.changes = changes;
        self.total_changes = self.total_changes.saturating_add(changes as u64);
        Ok(returned)
    }
    fn metadata(
        &self,
        name: &str,
        argument: Option<&str>,
        context: &mut Eval<'_>,
    ) -> Result<Option<QueryResult>> {
        let name = name.to_ascii_lowercase();
        let columns = match name.as_str() {
            "table_info" => vec!["cid", "name", "type", "notnull", "dflt_value", "pk"],
            "table_xinfo" => vec![
                "cid",
                "name",
                "type",
                "notnull",
                "dflt_value",
                "pk",
                "hidden",
            ],
            "index_list" => vec!["seq", "name", "unique", "origin", "partial"],
            "index_info" => vec!["seqno", "cid", "name"],
            "index_xinfo" => vec!["seqno", "cid", "name", "desc", "coll", "key"],
            _ => return Ok(None),
        };
        let argument = argument.unwrap_or("");
        let view_fields = if name == "table_info" || name == "table_xinfo" {
            self.view_index(argument)
                .map(|i| self.view_fields(i, context))
                .transpose()?
        } else {
            None
        };
        let text = |value: &str| Value::Text(Text::utf8(value));
        let integer = |value: usize| Value::Integer(value as i64);
        let mut rows = Vec::new();
        let mut bytes = 0;
        let mut push = |row: Vec<Value>| -> Result<()> {
            context.fuel.spend()?;
            if row
                .iter()
                .any(|v| scalar::size(v) > context.limits.max_value_bytes)
            {
                return Err(Error::Limit("metadata value bytes"));
            }
            push_row(&mut rows, row, &mut bytes, context.limits)
        };
        if name == "table_info" || name == "table_xinfo" {
            if argument.eq_ignore_ascii_case("sqlite_schema")
                || argument.eq_ignore_ascii_case("sqlite_master")
            {
                for (i, (column, typ)) in [
                    ("type", "TEXT"),
                    ("name", "TEXT"),
                    ("tbl_name", "TEXT"),
                    ("rootpage", "INT"),
                    ("sql", "TEXT"),
                ]
                .iter()
                .enumerate()
                {
                    let mut row = vec![
                        integer(i),
                        text(column),
                        text(typ),
                        integer(0),
                        Value::Null,
                        integer(0),
                    ];
                    if name == "table_xinfo" {
                        row.push(integer(0));
                    }
                    push(row)?;
                }
            } else if let Some(fields) = &view_fields {
                for (i, field) in fields.iter().enumerate() {
                    let mut row = vec![
                        integer(i),
                        text(&field.name),
                        text(&field.declared_type),
                        integer(0),
                        Value::Null,
                        integer(0),
                    ];
                    if name == "table_xinfo" {
                        row.push(integer(0));
                    }
                    push(row)?;
                }
            } else if let Ok(id) = self.index(argument) {
                let table = &self.state.tables[id];
                for (cid, (i, column)) in table
                    .columns
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| name == "table_xinfo" || c.generated.is_none())
                    .enumerate()
                {
                    let pk = table
                        .primary_key
                        .iter()
                        .position(|&c| c == i)
                        .map_or(0, |p| p + 1);
                    let mut row = vec![
                        integer(cid),
                        text(&column.name),
                        text(&column.declared_type),
                        integer(usize::from(column.not_null)),
                        column
                            .default_sql
                            .as_deref()
                            .map(text)
                            .unwrap_or(Value::Null),
                        integer(pk),
                    ];
                    if name == "table_xinfo" {
                        row.push(integer(
                            column
                                .generated
                                .as_ref()
                                .map_or(0, |(_, stored)| if *stored { 3 } else { 2 }),
                        ));
                    }
                    push(row)?;
                }
            }
        } else if name == "index_list" {
            if let Ok(id) = self.index(argument) {
                for (i, index) in self.state.tables[id].indexes.iter().rev().enumerate() {
                    let origin = if index.sql.is_some() {
                        "c"
                    } else if index.primary {
                        "pk"
                    } else {
                        "u"
                    };
                    push(vec![
                        integer(i),
                        text(&index.name),
                        integer(usize::from(index.unique)),
                        text(origin),
                        integer(usize::from(index.predicate.is_some())),
                    ])?;
                }
            }
        } else if let Some((t, i)) = self.named_index(argument).or_else(|| {
            let t = self.index(argument).ok()?;
            let table = &self.state.tables[t];
            if !table.without_rowid {
                return None;
            }
            Some((t, table.indexes.iter().position(|i| i.primary)?))
        }) {
            let table = &self.state.tables[t];
            let index = &table.indexes[i];
            let terms = table.storage_terms(index)?;
            for (i, term) in terms.iter().enumerate() {
                if name == "index_info" && i >= index.terms.len() {
                    break;
                }
                let mut row = vec![
                    integer(i),
                    term.column().map(integer).unwrap_or(Value::Integer(-2)),
                    term.column()
                        .map(|i| text(&table.columns[i].name))
                        .unwrap_or(Value::Null),
                ];
                if name == "index_xinfo" {
                    row.extend([
                        integer(usize::from(term.descending)),
                        text(&term.collation_name),
                        integer(usize::from(i < index.terms.len())),
                    ]);
                }
                push(row)?;
            }
            if name == "index_xinfo" && !table.without_rowid {
                push(vec![
                    integer(index.terms.len()),
                    Value::Integer(-1),
                    Value::Null,
                    integer(0),
                    text("BINARY"),
                    integer(0),
                ])?;
            }
        }
        Ok(Some(QueryResult {
            columns: columns.into_iter().map(String::from).collect(),
            rows,
            changes: 0,
        }))
    }
    fn simple_select(
        &self,
        query: &Select,
        context: &mut Eval<'_>,
        scope: Option<alloc::rc::Rc<query::Scope>>,
        runtime: &mut query::Runtime,
        schema_only: bool,
    ) -> Result<query::Data> {
        if query.sources.len() > 64 {
            return Err(Error::Limit("joined table count"));
        }
        let mut fields = Vec::new();
        let mut sources = Vec::new();
        let mut field_bytes = 0usize;
        let limit = self.expressions(scope.clone(), runtime).limit(
            query.limit.as_ref(),
            false,
            schema_only,
            context,
        )?;
        let offset = self
            .expressions(scope.clone(), runtime)
            .limit(query.offset.as_ref(), true, schema_only, context)?
            .unwrap_or(0);
        let schema_only = schema_only || limit == Some(0);
        let existence = runtime.existence_here();
        let source_cap = if query.sources.len() == 1
            && query.filter.is_none()
            && query.order.is_empty()
            && query.group.is_empty()
            && query.having.is_none()
            && !query.distinct
            && !query
                .items
                .iter()
                .filter_map(|i| i.expr.as_ref())
                .any(eval::has_aggregate)
        {
            limit.map(|n| n.saturating_add(offset))
        } else {
            None
        };
        for source in &query.sources {
            let data = self.query_source(
                source,
                context,
                scope.clone(),
                runtime,
                schema_only,
                source_cap,
            )?;
            let mut source_fields = data.fields(&source.alias);
            for field in &mut source_fields {
                if let Some(nested) = &mut field.nested {
                    nested.group = fields.len();
                }
            }
            for column in &source_fields {
                field_bytes = field_bytes
                    .checked_add(
                        source.alias.len()
                            + column.name.len()
                            + column.declared_type.len()
                            + core::mem::size_of::<Field>()
                            + column.extra_bytes(),
                    )
                    .ok_or(Error::Limit("query field bytes"))?;
            }
            if field_bytes > context.limits.max_database_bytes {
                return Err(Error::Limit("query field bytes"));
            }
            let start = fields.len();
            let width = source_fields.len();
            fields.extend(source_fields);
            let constraints = joins::using(
                &mut fields,
                start,
                source,
                query.sources.iter().any(|s| s.right),
                context.fuel,
            )?;
            field_bytes = field_bytes
                .checked_add(
                    fields
                        .iter()
                        .map(|f| f.merged.len() * core::mem::size_of::<usize>())
                        .sum::<usize>(),
                )
                .ok_or(Error::Limit("query field bytes"))?;
            if field_bytes > context.limits.max_database_bytes {
                return Err(Error::Limit("query field bytes"));
            }
            sources.push((source, data, constraints, start, width));
        }
        let mut projection = Vec::new();
        let mut columns = Vec::new();
        let mut aliases = Vec::new();
        let nested = if query.nested_from {
            let parts = sources
                .iter()
                .map(|(source, _, _, start, width)| (*source, *start, *width))
                .collect::<Vec<_>>();
            let output = joins::nested_projection(&fields, &parts, context)?;
            columns = output.columns;
            projection = output.expressions;
            Some(output.origins)
        } else {
            None
        };
        for item in &query.items {
            if projection.len() >= 2000 {
                return Err(Error::Limit("result columns"));
            }
            if let Some(expr) = &item.expr {
                let bound = self.expressions(scope.clone(), runtime).bind(
                    expr,
                    &fields,
                    &[],
                    true,
                    context,
                )?;
                let label = item.alias.clone().unwrap_or_else(|| {
                    if let ExprKind::Column { name, .. } = &expr.kind {
                        if let ExprKind::Slot(slot, ..) = &bound.kind {
                            if let Some(f) = fields.get(*slot).filter(|f| f.nested.is_some()) {
                                return f.name.clone();
                            }
                        }
                        name.clone()
                    } else {
                        item.label.clone()
                    }
                });
                if let Some(alias) = &item.alias {
                    aliases.push((alias.clone(), bound.clone()));
                }
                columns.push(label);
                projection.push(bound);
            } else {
                let mut count = 0;
                for (i, f) in fields.iter().enumerate() {
                    if f.wildcard(item.star.as_deref()) {
                        if projection.len() >= 2000 {
                            return Err(Error::Limit("result columns"));
                        }
                        columns.push(f.name.clone());
                        let merge_star = f.nested.is_none()
                            && sources
                                .iter()
                                .enumerate()
                                .find(|(_, (_, _, _, start, width))| {
                                    i >= *start && i < start + width
                                })
                                .is_some_and(|(source_index, (_, _, _, start, width))| {
                                    query.sources[source_index + 1..].iter().any(|s| s.right)
                                        && fields[start + width..].iter().any(|right| {
                                            right.qualified_only
                                                && right.name.eq_ignore_ascii_case(&f.name)
                                        })
                                });
                        // SQLite also expands a qualified wildcard through the
                        // merged name when that source precedes a RIGHT/FULL join
                        // and a later USING clause includes the column.
                        projection.push(
                            if merge_star || (query.sources.len() == 1 && f.nested.is_some()) {
                                eval::bind(
                                    &Expr {
                                        depth: 1,
                                        token: None,
                                        kind: ExprKind::Column {
                                            qualifier: None,
                                            name: f.name.clone(),
                                            quoted: false,
                                        },
                                    },
                                    &fields,
                                    &[],
                                    true,
                                )?
                            } else {
                                eval::field(&fields, i, None, item.star.is_some())?
                            },
                        );
                        count += 1;
                    }
                }
                if count == 0 {
                    return Err(error("wildcard has no matching table"));
                }
            }
        }
        let mut deferred_on = Vec::new();
        for (source, _, constraints, start, width) in &mut sources {
            if let Some(on) = &source.on {
                let on = self
                    .expressions(scope.clone(), runtime)
                    .bind(on, &fields, &aliases, false, context)?;
                let forward = joins::references_after(&on, *start + *width);
                if forward && (source.left || source.right || query.sources.iter().any(|s| s.right))
                {
                    return Err(error("ON clause references tables to its right"));
                }
                if forward {
                    deferred_on.push(on);
                } else {
                    constraints.push(on);
                }
            }
        }
        let filter = query
            .filter
            .as_ref()
            .map(|x| {
                self.expressions(scope.clone(), runtime)
                    .bind(x, &fields, &aliases, false, context)
            })
            .transpose()?;
        let having = query
            .having
            .as_ref()
            .map(|x| {
                self.expressions(scope.clone(), runtime)
                    .bind(x, &fields, &aliases, true, context)
            })
            .transpose()?;
        let mut resolve = |expr: &Expr, aggregate: bool| -> Result<Expr> {
            if let Some(n) = positional_integer(expr) {
                if n < 1 || n as usize > projection.len() {
                    return Err(error("ORDER/GROUP BY term out of range"));
                }
                let expr = projection[n as usize - 1].clone();
                if !aggregate && eval::has_aggregate(&expr) {
                    return Err(error("aggregate in GROUP BY"));
                }
                return Ok(expr);
            }
            if aggregate {
                if let ExprKind::Column {
                    qualifier: None,
                    name,
                    ..
                } = &expr.kind
                {
                    if let Some((_, value)) =
                        aliases.iter().find(|(a, _)| a.eq_ignore_ascii_case(name))
                    {
                        return Ok(value.clone());
                    }
                }
            }
            self.expressions(scope.clone(), runtime)
                .bind(expr, &fields, &aliases, aggregate, context)
        };
        let group = query
            .group
            .iter()
            .map(|x| resolve(x, false))
            .collect::<Result<Vec<_>>>()?;
        let order = query
            .order
            .iter()
            .map(|o| {
                Ok(Ordering {
                    expr: resolve(&o.expr, true)?,
                    descending: o.descending,
                    nulls_first: o.nulls_first,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let aggregate = projection.iter().any(eval::has_aggregate)
            || having.as_ref().is_some_and(eval::has_aggregate)
            || order.iter().any(|o| eval::has_aggregate(&o.expr));
        if having.is_some() && !aggregate && group.is_empty() {
            return Err(error("HAVING on a non-aggregate query"));
        }
        if schema_only {
            return Ok(query::Data {
                nested,
                fields,
                projection,
                column_types: None,
                result: QueryResult {
                    columns,
                    rows: Vec::new(),
                    changes: 0,
                },
            });
        }
        let mut rows = vec![Vec::new()];
        for (source, data, constraints, start, width) in sources {
            let mut joined = Vec::new();
            let mut bytes = 0;
            let mut right_matches = if source.right {
                vec![false; data.len()]
            } else {
                Vec::new()
            };
            for left in rows {
                let mut matched = false;
                for (i, values) in data
                    .rows()
                    .take(source_cap.unwrap_or(usize::MAX))
                    .enumerate()
                {
                    context.fuel.spend()?;
                    let mut row = left.clone();
                    row.extend(values);
                    let mut included = true;
                    for on in &constraints {
                        if !self.expressions(scope.clone(), runtime).filter(
                            Some(on),
                            &row,
                            context,
                        )? {
                            included = false;
                            break;
                        }
                    }
                    if included {
                        matched = true;
                        if source.right {
                            right_matches[i] = true;
                        }
                        push_row(&mut joined, row, &mut bytes, context.limits)?;
                    }
                }
                if source.left && !matched {
                    let mut row = left;
                    row.extend(core::iter::repeat_n(Value::Null, width));
                    push_row(&mut joined, row, &mut bytes, context.limits)?;
                }
            }
            if source.right {
                for (i, values) in data.rows().enumerate() {
                    context.fuel.spend()?;
                    if !right_matches[i] {
                        let mut row = vec![Value::Null; start];
                        row.extend(values);
                        push_row(&mut joined, row, &mut bytes, context.limits)?;
                    }
                }
            }
            rows = joined;
        }
        let mut filtered = Vec::new();
        let mut bytes = 0;
        for row in rows {
            context.fuel.spend()?;
            let mut included = true;
            for on in &deferred_on {
                if !self
                    .expressions(scope.clone(), runtime)
                    .filter(Some(on), &row, context)?
                {
                    included = false;
                    break;
                }
            }
            if !included {
                continue;
            }
            if self
                .expressions(scope.clone(), runtime)
                .filter(filter.as_ref(), &row, context)?
            {
                push_row(&mut filtered, row, &mut bytes, context.limits)?;
            }
        }
        let mut groups: Vec<(Vec<Value>, Vec<Vec<Value>>)> = Vec::new();
        if !group.is_empty() {
            for row in filtered {
                let key = self.expressions(scope.clone(), runtime).values(
                    group.iter(),
                    &row,
                    None,
                    context,
                )?;
                let mut found = None;
                for (i, (prior, _)) in groups.iter().enumerate() {
                    context.fuel.spend()?;
                    if same_values(&key, prior, &group, context.encoding)? {
                        found = Some(i);
                        break;
                    }
                }
                if let Some(i) = found {
                    groups[i].1.push(row);
                } else {
                    groups.push((key, vec![row]));
                }
            }
        } else if aggregate {
            groups.push((Vec::new(), filtered));
        } else {
            groups = filtered
                .into_iter()
                .map(|r| (Vec::new(), vec![r]))
                .collect();
        }
        let aggregates = projection
            .iter()
            .flat_map(aggregate_calls)
            .collect::<Vec<_>>();
        let mut output: Vec<Candidate> = Vec::new();
        let mut output_bytes = 0usize;
        for (_, members) in groups {
            let mut representative = members
                .first()
                .cloned()
                .unwrap_or_else(|| vec![Value::Null; fields.len()]);
            if aggregates.len() == 1 && (!existence || having.is_some()) {
                if let ExprKind::Call { name, args, .. } = &aggregates[0].kind {
                    if name == "min" || name == "max" {
                        let mut best = Value::Null;
                        for member in &members {
                            let value = self
                                .expressions(scope.clone(), runtime)
                                .eval(&args[0], member, None, context)?;
                            if scalar::null(&value) {
                                continue;
                            }
                            let cmp = scalar::compare_encoded(
                                &value,
                                &best,
                                eval::collation(&args[0]),
                                context.encoding,
                            )?;
                            if scalar::null(&best)
                                || (name == "min" && cmp == Compare::Less)
                                || (name == "max" && cmp == Compare::Greater)
                            {
                                representative = member.clone();
                                best = value;
                            }
                        }
                    }
                }
            }
            let grouped = if aggregate || !group.is_empty() {
                Some(members.as_slice())
            } else {
                None
            };
            if let Some(having) = &having {
                if scalar::truth(&self.expressions(scope.clone(), runtime).eval(
                    having,
                    &representative,
                    grouped,
                    context,
                )?)? != Some(true)
                {
                    continue;
                }
            }
            let values = if existence {
                // EXISTS discards projected values, but aggregate inputs still
                // run while forming groups and can fail (e.g. SUM overflow).
                for expr in &aggregates {
                    self.expressions(scope.clone(), runtime).eval(
                        expr,
                        &representative,
                        grouped,
                        context,
                    )?;
                }
                vec![Value::Null; projection.len()]
            } else {
                self.expressions(scope.clone(), runtime).values(
                    projection.iter(),
                    &representative,
                    grouped,
                    context,
                )?
            };
            if query.distinct && !existence {
                let mut duplicate = false;
                for previous in &output {
                    context.fuel.spend()?;
                    if same_values(&values, &previous.values, &projection, context.encoding)? {
                        duplicate = true;
                        break;
                    }
                }
                if duplicate {
                    continue;
                }
            }
            let keys = self.expressions(scope.clone(), runtime).values(
                order.iter().map(|o| &o.expr),
                &representative,
                grouped,
                context,
            )?;
            for v in values.iter().chain(&keys) {
                output_bytes = output_bytes
                    .checked_add(scalar::size(v) + core::mem::size_of::<Value>())
                    .ok_or(Error::Limit("query output bytes"))?;
            }
            if output.len() >= context.limits.max_rows
                || output_bytes > context.limits.max_database_bytes
            {
                return Err(Error::Limit("query output"));
            }
            output.push(Candidate { values, keys });
            if order.is_empty() && limit.is_some_and(|n| output.len() >= n.saturating_add(offset)) {
                break;
            }
        }
        if !order.is_empty() {
            output = sort(output, &order, context.fuel, context.encoding)?;
        }
        Ok(query::Data {
            nested,
            fields,
            projection,
            column_types: None,
            result: QueryResult {
                columns,
                rows: output
                    .into_iter()
                    .skip(offset)
                    .take(limit.unwrap_or(usize::MAX))
                    .map(|r| r.values)
                    .collect(),
                changes: 0,
            },
        })
    }
    /// Import a consistent offline image. Reject schemas not understood by this
    /// engine so constraints, triggers, views and indexes are never silently lost.
    pub fn from_image(image: &[u8]) -> Result<Self> {
        let limits = SqlLimits::default();
        let db = Database::with_limits(
            image,
            crate::Limits {
                max_rows: limits.max_rows,
                max_columns: 2000,
                max_total_payload_bytes: limits.max_database_bytes,
                ..crate::Limits::default()
            },
        )?;
        let schema = db.schema()?;
        let sequence_count = schema
            .iter()
            .filter(|e| e.kind == "table" && e.name == sequence::NAME)
            .count();
        if sequence_count > 1 {
            return Err(Error::Corrupt("duplicate sqlite_sequence schema"));
        }
        let mut connection = Self::new();
        for entry in &schema {
            if entry.kind == "index" {
                continue;
            }
            if entry.kind == "view" {
                if entry.root_page != 0 || entry.table_name != entry.name {
                    return Err(Error::Corrupt("view schema metadata"));
                }
                let sql = entry
                    .sql
                    .as_deref()
                    .ok_or(Error::Corrupt("view has no CREATE SQL"))?;
                let prepared = connection.prepare(sql)?;
                if !matches!(&prepared.statement, Statement::CreateView { name, .. } if name == &entry.name)
                {
                    return Err(Error::Corrupt("view schema does not match CREATE SQL"));
                }
                connection.execute_prepared(&prepared, &[])?;
                continue;
            }
            if entry.kind != "table" {
                return Err(Error::Unsupported(
                    "SQL image import with triggers or other schema objects",
                ));
            }
            let sql = entry
                .sql
                .as_deref()
                .ok_or_else(|| error("table has no CREATE SQL"))?;
            let prepared = connection.prepare(sql)?;
            if !matches!(&prepared.statement, Statement::Create { name, .. } if name == &entry.name)
                || entry.table_name != entry.name
                || entry.root_page == 0
            {
                return Err(Error::Corrupt("table schema does not match CREATE SQL"));
            }
            if entry.name == sequence::NAME {
                if sql != sequence::SQL {
                    return Err(Error::Corrupt("sqlite_sequence declaration"));
                }
                connection.ensure_sequence()?;
            } else {
                connection.execute_prepared(&prepared, &[])?;
            }
            let index = connection.index(&entry.name)?;
            let mut fuel = Fuel::new(connection.limits.max_steps);
            let mut context = Eval {
                encoding: db.header().encoding,
                params: &[],
                limits: connection.limits,
                fuel: &mut fuel,
                changes: 0,
                total_changes: 0,
                last_rowid: 0,
            };
            let table = &mut connection.state.tables[index];
            let defaults = defaults(table)?;
            let layout: Vec<usize> = if table.without_rowid {
                table
                    .storage_terms(table.primary_index()?)?
                    .iter()
                    .map(IndexTerm::column)
                    .collect::<Result<_>>()?
            } else {
                table
                    .columns
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| !c.virtual_column())
                    .map(|(i, _)| i)
                    .collect()
            };
            let mut imported_bytes = 0usize;
            db.visit_rows(entry.root_page, |row| {
                let id = match (table.without_rowid, row.rowid) {
                    (false, Some(id)) => id,
                    (true, None) => i64::try_from(table.rows.len() + 1)
                        .map_err(|_| Error::Limit("internal row identity"))?,
                    _ => return Err(Error::Corrupt("table B-tree kind does not match schema")),
                };
                if row.values.len() > layout.len() {
                    return Err(Error::Corrupt("record has too many columns"));
                }
                let mut values = vec![Value::Null; table.columns.len()];
                let mut present = vec![false; table.columns.len()];
                for (value, column) in row.values.into_iter().zip(&layout) {
                    let value = normalize(value)?;
                    if present[*column] && values[*column] != value {
                        return Err(Error::Corrupt("inconsistent repeated primary-key column"));
                    }
                    values[*column] = value;
                    present[*column] = true;
                }
                for (i, value) in values.iter_mut().enumerate() {
                    if !present[i] {
                        *value = defaults[i]
                            .as_ref()
                            .map(|d| context.eval(d, &[], None))
                            .transpose()?
                            .unwrap_or(Value::Null);
                    }
                }
                if table.without_rowid
                    && table.primary_key.iter().any(|i| scalar::null(&values[*i]))
                {
                    return Err(Error::Corrupt("NULL WITHOUT ROWID primary key"));
                }
                if let Some(alias) = table.alias() {
                    values[alias] = Value::Integer(id);
                }
                for (value, column) in values.iter_mut().zip(&table.columns) {
                    if column.affinity == Affinity::Real {
                        if let Value::Integer(n) = value {
                            *value = Value::Real(*n as f64);
                        }
                    }
                }
                imported_bytes = imported_bytes
                    .checked_add(values_size(&values)?)
                    .ok_or(Error::Limit("imported table bytes"))?;
                if imported_bytes > context.limits.max_database_bytes
                    || table.rows.len() >= context.limits.max_rows
                {
                    return Err(Error::Limit("imported table rows/bytes"));
                }
                table.rows.insert(id, values);
                Ok(())
            })?;
            connection.check_budget()?;
        }
        if connection.state.tables.iter().any(|t| t.autoincrement) && sequence_count == 0 {
            return Err(Error::Corrupt("missing sqlite_sequence schema"));
        }
        for entry in schema.iter().filter(|e| e.kind == "index") {
            if let Some(sql) = &entry.sql {
                let prepared = connection.prepare(sql)?;
                match &prepared.statement {
                    Statement::CreateIndex { name, table, .. }
                        if name == &entry.name && table.eq_ignore_ascii_case(&entry.table_name) => {
                    }
                    _ => return Err(Error::Corrupt("index schema does not match CREATE SQL")),
                }
                connection.execute_prepared(&prepared, &[])?;
            } else {
                let (t, i) = connection
                    .named_index(&entry.name)
                    .ok_or(Error::Corrupt("unexpected automatic index"))?;
                if connection.state.tables[t].name != entry.table_name
                    || connection.state.tables[t].indexes[i].sql.is_some()
                    || (connection.state.tables[t].without_rowid
                        && connection.state.tables[t].indexes[i].primary)
                {
                    return Err(Error::Corrupt("automatic index schema mismatch"));
                }
            }
        }
        for table in &connection.state.tables {
            for index in &table.indexes {
                if table.without_rowid && index.primary {
                    continue;
                }
                if !schema
                    .iter()
                    .any(|e| e.kind == "index" && e.name == index.name)
                {
                    return Err(Error::Corrupt("missing index schema entry"));
                }
            }
        }
        connection.state.user_version = db.header().user_version;
        connection.state.application_id = db.header().application_id;
        connection.state.encoding = Some(db.header().encoding);
        connection.state.auto_vacuum = db.header().auto_vacuum;
        Ok(connection)
    }
    /// Export a new image, with table and index B-trees, without modifying any file.
    pub fn to_image(&self, page_size: usize) -> Result<Vec<u8>> {
        let encoding = self.state.encoding.unwrap_or(Encoding::Utf8);
        let mut builder = ImageBuilder::new(page_size)?
            .encoding(encoding)?
            .auto_vacuum(self.state.auto_vacuum);
        let mut fuel = Fuel::new(self.limits.max_steps);
        let mut exported_bytes = self.usage()?.0;
        for source in &self.state.tables {
            let physical: Vec<_> = source
                .columns
                .iter()
                .enumerate()
                .filter(|(_, c)| !c.virtual_column())
                .map(|(i, _)| i)
                .collect();
            let names: Vec<_> = physical
                .iter()
                .map(|i| source.columns[*i].name.as_str())
                .collect();
            let mut table = Table::new(&source.name, &names);
            for (id, values) in source.rows.iter().filter(|_| !source.without_rowid) {
                let mut values: Vec<_> = physical
                    .iter()
                    .map(|i| {
                        if Some(*i) == source.alias() {
                            Value::Null
                        } else {
                            values[*i].clone()
                        }
                    })
                    .collect();
                let before = values_size(&values)?;
                transcode_values(&mut values, encoding)?;
                exported_bytes = exported_bytes
                    .checked_add(values_size(&values)?.saturating_sub(before))
                    .ok_or(Error::Limit("exported table bytes"))?;
                if exported_bytes > self.limits.max_database_bytes {
                    return Err(Error::Limit("exported table bytes"));
                }
                table.rows.push((*id, values));
            }
            if source.without_rowid {
                let mut entries = index_entries(
                    source,
                    source.primary_index()?,
                    self.limits,
                    &mut fuel,
                    encoding,
                )?;
                for row in &mut entries {
                    transcode_values(row, encoding)?;
                    exported_bytes = exported_bytes
                        .checked_add(values_size(row)?)
                        .ok_or(Error::Limit("exported table bytes"))?;
                }
                if exported_bytes > self.limits.max_database_bytes {
                    return Err(Error::Limit("exported table bytes"));
                }
                builder.add_without_rowid_table(table, source.sql.clone(), entries)?;
            } else {
                builder.add_table_with_sql(table, source.sql.clone())?;
            }
            for index in &source.indexes {
                if source.without_rowid && index.primary {
                    continue;
                }
                let mut entries = index_entries(source, index, self.limits, &mut fuel, encoding)?;
                for row in &mut entries {
                    transcode_values(row, encoding)?;
                }
                for row in &entries {
                    exported_bytes = exported_bytes
                        .checked_add(values_size(row)?)
                        .ok_or(Error::Limit("exported index bytes"))?;
                }
                if exported_bytes > self.limits.max_database_bytes {
                    return Err(Error::Limit("exported index bytes"));
                }
                builder.add_index(crate::writer::Index {
                    name: index.name.clone(),
                    table: source.name.clone(),
                    sql: index.sql.clone(),
                    entries,
                })?;
            }
        }
        for view in &self.state.views {
            builder.add_view(view.name.clone(), view.sql.clone())?;
        }
        let mut image = builder.finish()?;
        image[60..64].copy_from_slice(&self.state.user_version.to_be_bytes());
        image[68..72].copy_from_slice(&self.state.application_id.to_be_bytes());
        Ok(image)
    }
}
fn index_entries(
    table: &StoredTable,
    index: &StoredIndex,
    limits: SqlLimits,
    fuel: &mut Fuel,
    encoding: Encoding,
) -> Result<Vec<Vec<Value>>> {
    let terms = table.storage_terms(index)?;
    let mut rows = Vec::new();
    let mut bytes = 0;
    for (id, values) in &table.rows {
        fuel.spend()?;
        let mut context = Eval {
            encoding,
            params: &[],
            limits,
            fuel,
            changes: 0,
            total_changes: 0,
            last_rowid: 0,
        };
        if !index.includes(table, *id, values, &mut context)? {
            continue;
        }
        let mut row = Vec::new();
        let mut row_bytes = 0usize;
        for term in &terms {
            let value = term.value(table, *id, values, &mut context)?;
            push_value(&mut row, value, &mut row_bytes, limits)?;
        }
        if !table.without_rowid {
            row.push(Value::Integer(*id));
        }
        push_row(&mut rows, row, &mut bytes, limits)?;
    }
    let rows = sort_by(rows, fuel, |a, b| {
        let order = if table.without_rowid && index.primary {
            &index.terms
        } else {
            &terms
        };
        for ((a, b), term) in a.iter().zip(b).zip(order) {
            let cmp = scalar::compare_encoded(a, b, term.collation, encoding)?;
            if cmp != Compare::Equal {
                return Ok(if term.descending { cmp.reverse() } else { cmp });
            }
        }
        if table.without_rowid {
            return Ok(Compare::Equal);
        }
        scalar::compare(
            &a[index.terms.len()],
            &b[index.terms.len()],
            Collation::Binary,
        )
    })?;
    if index.unique {
        for pair in rows.windows(2) {
            let mut equal = true;
            for ((a, b), term) in pair[0].iter().zip(&pair[1]).zip(&index.terms) {
                fuel.spend()?;
                if scalar::null(a)
                    || scalar::null(b)
                    || scalar::compare(a, b, term.collation)? != Compare::Equal
                {
                    equal = false;
                    break;
                }
            }
            if equal {
                return Err(Error::Constraint(format!("UNIQUE: {}", index.name)));
            }
        }
    }
    Ok(rows)
}
fn transcode_values(values: &mut [Value], encoding: Encoding) -> Result<()> {
    for value in values {
        if let Value::Text(text) = value {
            *text = text.transcode(encoding)?;
        }
    }
    Ok(())
}
fn contains_parameter(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::Parameter(_)) || e.children().iter().any(|x| contains_parameter(x))
}
fn normalize(value: Value) -> Result<Value> {
    match value {
        Value::Real(n) => Ok(scalar::real(n)),
        Value::Text(t) if t.encoding != Encoding::Utf8 => {
            Ok(Value::Text(Text::utf8(&t.to_string()?)))
        }
        v => Ok(v),
    }
}
fn rowid(value: Value) -> Result<i64> {
    match scalar::affinity(value, Affinity::Integer)? {
        Value::Integer(n) => Ok(n),
        _ => Err(Error::Datatype("rowid requires an integer".into())),
    }
}
fn defaults(table: &StoredTable) -> Result<Vec<Option<Expr>>> {
    table
        .columns
        .iter()
        .map(|c| {
            c.default
                .as_ref()
                .map(|x| eval::bind(x, &[], &[], false))
                .transpose()
        })
        .collect()
}
fn checks(table: &StoredTable) -> Result<Vec<Expr>> {
    let fields = table.fields(&table.name);
    table
        .columns
        .iter()
        .flat_map(|c| c.checks.iter())
        .chain(&table.checks)
        .map(|e| eval::bind(e, &fields, &[], false))
        .collect()
}
fn same_values(a: &[Value], b: &[Value], expr: &[Expr], encoding: Encoding) -> Result<bool> {
    for ((a, b), e) in a.iter().zip(b).zip(expr) {
        if scalar::compare_encoded(a, b, eval::collation(e), encoding)? != Compare::Equal {
            return Ok(false);
        }
    }
    Ok(true)
}
fn push_row(
    rows: &mut Vec<Vec<Value>>,
    row: Vec<Value>,
    bytes: &mut usize,
    limits: SqlLimits,
) -> Result<()> {
    for v in &row {
        *bytes = bytes
            .checked_add(scalar::size(v) + core::mem::size_of::<Value>())
            .ok_or(Error::Limit("query intermediate bytes"))?;
    }
    if rows.len() >= limits.max_rows || *bytes > limits.max_database_bytes {
        return Err(Error::Limit("query intermediate rows/bytes"));
    }
    rows.push(row);
    Ok(())
}
fn aggregate_calls(expr: &Expr) -> Vec<&Expr> {
    if matches!(&expr.kind,ExprKind::Call{name,args,..} if eval::aggregate(name,args.len())) {
        vec![expr]
    } else {
        expr.children()
            .into_iter()
            .flat_map(aggregate_calls)
            .collect()
    }
}
struct Candidate {
    values: Vec<Value>,
    keys: Vec<Value>,
}
fn sort(
    rows: Vec<Candidate>,
    order: &[Ordering],
    fuel: &mut Fuel,
    encoding: Encoding,
) -> Result<Vec<Candidate>> {
    sort_by(rows, fuel, |a, b| {
        compare_keys(&a.keys, &b.keys, order, encoding)
    })
}
fn compare_keys(
    a: &[Value],
    b: &[Value],
    order: &[Ordering],
    encoding: Encoding,
) -> Result<Compare> {
    for ((a, b), term) in a.iter().zip(b).zip(order) {
        let cmp = if scalar::null(a) != scalar::null(b) {
            let nulls_first = term.nulls_first.unwrap_or(!term.descending);
            if scalar::null(a) == nulls_first {
                Compare::Less
            } else {
                Compare::Greater
            }
        } else {
            let cmp = scalar::compare_encoded(a, b, eval::collation(&term.expr), encoding)?;
            if term.descending {
                cmp.reverse()
            } else {
                cmp
            }
        };
        if cmp != Compare::Equal {
            return Ok(cmp);
        }
    }
    Ok(Compare::Equal)
}
fn sort_by<T>(
    rows: Vec<T>,
    fuel: &mut Fuel,
    compare: impl Fn(&T, &T) -> Result<Compare>,
) -> Result<Vec<T>> {
    let mut indices: Vec<usize> = (0..rows.len()).collect();
    let mut buffer = indices.clone();
    let mut width = 1;
    while width < indices.len() {
        let mut start = 0;
        while start < indices.len() {
            let middle = (start + width).min(indices.len());
            let end = (middle + width).min(indices.len());
            let (mut a, mut b, mut at) = (start, middle, start);
            while a < middle || b < end {
                fuel.spend()?;
                if b == end
                    || (a < middle
                        && compare(&rows[indices[a]], &rows[indices[b]])? != Compare::Greater)
                {
                    buffer[at] = indices[a];
                    a += 1;
                } else {
                    buffer[at] = indices[b];
                    b += 1;
                }
                at += 1;
            }
            start = end;
        }
        core::mem::swap(&mut indices, &mut buffer);
        width = width.saturating_mul(2);
    }
    let mut rows: Vec<Option<T>> = rows.into_iter().map(Some).collect();
    indices
        .into_iter()
        .map(|i| {
            rows[i]
                .take()
                .ok_or_else(|| error("invalid sort permutation"))
        })
        .collect()
}

fn positional_integer(expr: &Expr) -> Option<i64> {
    match &expr.kind {
        ExprKind::Literal(Value::Integer(n)) => Some(*n),
        ExprKind::Unary(parser::Unary::Plus, x) => positional_integer(x),
        ExprKind::Unary(parser::Unary::Minus, x) => positional_integer(x)?.checked_neg(),
        _ => None,
    }
}
fn values_size(values: &[Value]) -> Result<usize> {
    values.iter().try_fold(0usize, |n, v| {
        n.checked_add(scalar::size(v) + core::mem::size_of::<Value>())
            .ok_or(Error::Limit("row bytes"))
    })
}
fn push_value(
    values: &mut Vec<Value>,
    value: Value,
    bytes: &mut usize,
    limits: SqlLimits,
) -> Result<()> {
    *bytes = bytes
        .checked_add(scalar::size(&value) + core::mem::size_of::<Value>())
        .ok_or(Error::Limit("row bytes"))?;
    if *bytes > limits.max_database_bytes {
        return Err(Error::Limit("row bytes"));
    }
    values.push(value);
    Ok(())
}
fn evaluate_list<'a>(
    expr: impl Iterator<Item = &'a Expr>,
    row: &[Value],
    group: Option<&[Vec<Value>]>,
    context: &mut Eval<'_>,
) -> Result<Vec<Value>> {
    let mut values = Vec::new();
    let mut bytes = 0;
    for expr in expr {
        let value = context.eval(expr, row, group)?;
        push_value(&mut values, value, &mut bytes, context.limits)?;
    }
    Ok(values)
}

enum InputRow {
    Default,
    Values(Vec<Value>),
    Expressions(Vec<Expr>),
}
