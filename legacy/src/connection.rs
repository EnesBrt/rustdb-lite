use crate::engine as ffi;
use std::ffi::{CStr, CString};
use std::marker::PhantomData;
use std::path::Path;
use std::ptr;
use std::rc::Rc;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: i32,
    pub message: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (SQLite code {})", self.message, self.code)
    }
}
impl std::error::Error for Error {}

fn error(code: i32, message: impl Into<String>) -> Error {
    Error {
        code,
        message: message.into(),
    }
}

fn c_string(s: impl AsRef<[u8]>) -> Result<CString> {
    CString::new(s.as_ref())
        .map_err(|_| error(ffi::SQLITE_MISUSE, "interior NUL in SQL or filename"))
}

/// Owned SQLite values. Text must be valid UTF-8; SQL CAST(... AS BLOB) can be
/// used to retrieve arbitrary text bytes losslessly.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

/// An owned connection to the Rust engine. Connections and their statements
/// stay on their creating thread; distinct connections may be used in parallel.
pub struct Connection {
    db: *mut ffi::sqlite3,
    _not_send_sync: PhantomData<Rc<()>>,
}

impl Connection {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        use std::os::unix::ffi::OsStrExt;
        let filename = c_string(path.as_ref().as_os_str().as_bytes())?;
        let mut db = ptr::null_mut();
        let rc = unsafe {
            ffi::sqlite3_open_v2(
                filename.as_ptr(),
                &mut db,
                ffi::SQLITE_OPEN_READWRITE
                    | ffi::SQLITE_OPEN_CREATE
                    | ffi::SQLITE_OPEN_URI
                    | ffi::SQLITE_OPEN_FULLMUTEX,
                ptr::null(),
            )
        };
        if db.is_null() {
            return Err(error(rc, "could not allocate SQLite connection"));
        }
        let connection = Self {
            db,
            _not_send_sync: PhantomData,
        };
        connection.check(rc)?;
        unsafe {
            ffi::sqlite3_extended_result_codes(db, 1);
        }
        Ok(connection)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::open(":memory:")
    }

    pub fn version() -> &'static str {
        unsafe {
            CStr::from_ptr(ffi::sqlite3_libversion())
                .to_str()
                .expect("ASCII SQLite version")
        }
    }

    fn check(&self, rc: i32) -> Result<()> {
        if rc == ffi::SQLITE_OK {
            Ok(())
        } else {
            Err(error(
                rc,
                unsafe { CStr::from_ptr(ffi::sqlite3_errmsg(self.db)) }.to_string_lossy(),
            ))
        }
    }

    /// Execute a SQL script, discarding result rows. Earlier statements remain
    /// committed if a later statement fails, following SQLite's exec behavior.
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        let sql = c_string(sql)?;
        self.check(unsafe {
            ffi::sqlite3_exec(
                self.db,
                sql.as_ptr(),
                None,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        })
    }

    /// Prepare exactly one statement; trailing comments and whitespace are allowed.
    pub fn prepare(&self, sql: &str) -> Result<Statement<'_>> {
        let sql = c_string(sql)?;
        let (statement, mut tail) = unsafe { self.prepare_at(sql.as_ptr())? };
        let statement =
            statement.ok_or_else(|| error(ffi::SQLITE_MISUSE, "SQL contains no statement"))?;
        while unsafe { *tail } != 0 {
            let (extra, next) = unsafe { self.prepare_at(tail)? };
            if extra.is_some() {
                return Err(error(
                    ffi::SQLITE_MISUSE,
                    "prepare accepts exactly one SQL statement",
                ));
            }
            if tail == next {
                break;
            }
            tail = next;
        }
        Ok(statement)
    }

    // The caller keeps the NUL-terminated SQL allocation alive until preparation
    // returns. SQLite copies the statement text for prepare_v2.
    unsafe fn prepare_at(
        &self,
        sql: *const libc::c_char,
    ) -> Result<(Option<Statement<'_>>, *const libc::c_char)> {
        let mut raw = ptr::null_mut();
        let mut tail = ptr::null();
        let rc = ffi::sqlite3_prepare_v2(self.db, sql, -1, &mut raw, &mut tail);
        if rc != ffi::SQLITE_OK {
            if !raw.is_null() {
                ffi::sqlite3_finalize(raw);
            }
            self.check(rc)?;
        }
        Ok((
            if raw.is_null() {
                None
            } else {
                Some(Statement {
                    raw,
                    connection: self,
                    done: false,
                })
            },
            tail,
        ))
    }

    pub fn query(&self, sql: &str, params: &[Value]) -> Result<QueryResult> {
        let mut statement = self.prepare(sql)?;
        statement.bind_all(params)?;
        statement.collect()
    }

    pub fn execute(&self, sql: &str, params: &[Value]) -> Result<i64> {
        let mut statement = self.prepare(sql)?;
        statement.bind_all(params)?;
        while statement.step()?.is_some() {}
        Ok(self.changes())
    }

    /// Execute all statements and return results for those having columns.
    pub fn run(&self, sql: &str) -> Result<Vec<QueryResult>> {
        let sql = c_string(sql)?;
        let mut cursor = sql.as_ptr();
        let mut results = Vec::new();
        while unsafe { *cursor } != 0 {
            let (statement, next) = unsafe { self.prepare_at(cursor)? };
            if let Some(mut statement) = statement {
                let result = statement.collect()?;
                if !result.columns.is_empty() {
                    results.push(result);
                }
            }
            if cursor == next {
                break;
            }
            cursor = next;
        }
        Ok(results)
    }

    pub fn changes(&self) -> i64 {
        unsafe { ffi::sqlite3_changes64(self.db) }
    }
    pub fn last_insert_rowid(&self) -> i64 {
        unsafe { ffi::sqlite3_last_insert_rowid(self.db) }
    }
    pub fn is_autocommit(&self) -> bool {
        unsafe { ffi::sqlite3_get_autocommit(self.db) != 0 }
    }

    pub fn busy_timeout(&self, milliseconds: i32) -> Result<()> {
        if milliseconds < 0 {
            return Err(error(ffi::SQLITE_MISUSE, "negative busy timeout"));
        }
        self.check(unsafe { ffi::sqlite3_busy_timeout(self.db, milliseconds) })
    }

    /// Enable or disable statement execution counters. The upstream shell turns
    /// these off by default, while the engine's default is on in this build.
    pub fn statement_scan_status(&self, enabled: bool) -> Result<()> {
        self.check(unsafe {
            ffi::sqlite3_db_config(
                self.db,
                ffi::SQLITE_DBCONFIG_STMT_SCANSTATUS,
                enabled as i32,
                ptr::null_mut::<i32>(),
            )
        })
    }

    pub fn transaction(&mut self) -> Result<Transaction<'_>> {
        self.execute_batch("BEGIN")?;
        Ok(Transaction {
            connection: self,
            finished: false,
        })
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // close_v2 also handles a deliberately forgotten statement without freeing
        // a live handle; SQLite defers destruction until that statement is finalized.
        unsafe {
            ffi::sqlite3_close_v2(self.db);
        }
    }
}

pub struct Statement<'connection> {
    raw: *mut ffi::sqlite3_stmt,
    connection: &'connection Connection,
    done: bool,
}

impl Statement<'_> {
    pub fn parameter_count(&self) -> usize {
        unsafe { ffi::sqlite3_bind_parameter_count(self.raw) as usize }
    }

    pub fn bind_all(&mut self, values: &[Value]) -> Result<()> {
        if values.len() != self.parameter_count() {
            return Err(error(
                ffi::SQLITE_RANGE,
                format!(
                    "expected {} parameters, got {}",
                    self.parameter_count(),
                    values.len()
                ),
            ));
        }
        for (i, value) in values.iter().enumerate() {
            self.bind(i + 1, value)?;
        }
        Ok(())
    }

    /// Bind a one-based parameter index. SQLite copies text and blob bytes.
    pub fn bind(&mut self, index: usize, value: &Value) -> Result<()> {
        if index == 0 || index > self.parameter_count() {
            return Err(error(ffi::SQLITE_RANGE, "parameter index out of range"));
        }
        let index = index as i32;
        unsafe {
            let transient: ffi::sqlite3_destructor_type = std::mem::transmute(-1isize);
            let rc = match value {
                Value::Null => ffi::sqlite3_bind_null(self.raw, index),
                Value::Integer(v) => ffi::sqlite3_bind_int64(self.raw, index, *v),
                Value::Real(v) => ffi::sqlite3_bind_double(self.raw, index, *v),
                Value::Text(v) => ffi::sqlite3_bind_text64(
                    self.raw,
                    index,
                    v.as_ptr().cast(),
                    v.len() as u64,
                    transient,
                    ffi::SQLITE_UTF8 as u8,
                ),
                Value::Blob(v) => ffi::sqlite3_bind_blob64(
                    self.raw,
                    index,
                    v.as_ptr().cast(),
                    v.len() as u64,
                    transient,
                ),
            };
            self.connection.check(rc)
        }
    }

    pub fn reset(&mut self) -> Result<()> {
        self.done = false;
        self.connection
            .check(unsafe { ffi::sqlite3_reset(self.raw) })
    }

    pub fn clear_bindings(&mut self) -> Result<()> {
        self.connection
            .check(unsafe { ffi::sqlite3_clear_bindings(self.raw) })
    }

    pub fn columns(&self) -> Vec<String> {
        let n = unsafe { ffi::sqlite3_column_count(self.raw) };
        (0..n)
            .map(|i| {
                let name = unsafe { ffi::sqlite3_column_name(self.raw, i) };
                if name.is_null() {
                    String::new()
                } else {
                    unsafe { CStr::from_ptr(name) }
                        .to_string_lossy()
                        .into_owned()
                }
            })
            .collect()
    }

    pub fn step(&mut self) -> Result<Option<Vec<Value>>> {
        if self.done {
            return Ok(None);
        }
        match unsafe { ffi::sqlite3_step(self.raw) } {
            ffi::SQLITE_ROW => {
                let count = unsafe { ffi::sqlite3_column_count(self.raw) };
                (0..count)
                    .map(|i| unsafe { self.column(i) })
                    .collect::<Result<Vec<_>>>()
                    .map(Some)
            }
            ffi::SQLITE_DONE => {
                self.done = true;
                Ok(None)
            }
            rc => {
                self.done = true;
                self.connection.check(rc)?;
                unreachable!()
            }
        }
    }

    unsafe fn column(&self, i: i32) -> Result<Value> {
        Ok(match ffi::sqlite3_column_type(self.raw, i) {
            ffi::SQLITE_NULL => Value::Null,
            ffi::SQLITE_INTEGER => Value::Integer(ffi::sqlite3_column_int64(self.raw, i)),
            ffi::SQLITE_FLOAT => Value::Real(ffi::sqlite3_column_double(self.raw, i)),
            ffi::SQLITE_TEXT => {
                let p = ffi::sqlite3_column_text(self.raw, i);
                if p.is_null() {
                    return Err(error(ffi::SQLITE_NOMEM, "could not read text value"));
                }
                let len = ffi::sqlite3_column_bytes(self.raw, i) as usize;
                let s = std::str::from_utf8(std::slice::from_raw_parts(p, len)).map_err(|_| {
                    error(
                        ffi::SQLITE_MISMATCH,
                        "TEXT is not valid UTF-8; retrieve CAST(value AS BLOB) for raw bytes",
                    )
                })?;
                Value::Text(s.to_owned())
            }
            ffi::SQLITE_BLOB => {
                let p = ffi::sqlite3_column_blob(self.raw, i);
                let len = ffi::sqlite3_column_bytes(self.raw, i) as usize;
                if p.is_null() && ffi::sqlite3_errcode(self.connection.db) == ffi::SQLITE_NOMEM {
                    return Err(error(ffi::SQLITE_NOMEM, "could not read blob value"));
                }
                Value::Blob(if len == 0 {
                    Vec::new()
                } else {
                    if p.is_null() {
                        return Err(error(ffi::SQLITE_NOMEM, "could not read blob value"));
                    }
                    std::slice::from_raw_parts(p.cast::<u8>(), len).to_vec()
                })
            }
            _ => return Err(error(ffi::SQLITE_MISMATCH, "unknown SQLite value type")),
        })
    }

    pub fn collect(&mut self) -> Result<QueryResult> {
        let mut rows = Vec::new();
        while let Some(row) = self.step()? {
            rows.push(row);
        }
        Ok(QueryResult {
            columns: self.columns(),
            rows,
        })
    }
}

impl Drop for Statement<'_> {
    fn drop(&mut self) {
        unsafe {
            ffi::sqlite3_finalize(self.raw);
        }
    }
}

/// Rolls back on drop unless explicitly committed. Borrowing the connection
/// mutably prevents two safe transaction guards on the same connection.
pub struct Transaction<'connection> {
    connection: &'connection mut Connection,
    finished: bool,
}
impl Transaction<'_> {
    pub fn commit(mut self) -> Result<()> {
        self.connection.execute_batch("COMMIT")?;
        self.finished = true;
        Ok(())
    }
    pub fn rollback(mut self) -> Result<()> {
        self.connection.execute_batch("ROLLBACK")?;
        self.finished = true;
        Ok(())
    }
}
impl std::ops::Deref for Transaction<'_> {
    type Target = Connection;
    fn deref(&self) -> &Self::Target {
        self.connection
    }
}
impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.connection.execute_batch("ROLLBACK");
        }
    }
}
