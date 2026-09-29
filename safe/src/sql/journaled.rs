use super::{parser::Statement, Connection, PreparedStatement, QueryResult};
use crate::{
    journal::JournalLimits,
    pager::{Pager, Storage},
    Database, Error, ImageBuilder, Result, Value,
};

/// Experimental SQL connection using the rollback pager and a caller-provided
/// storage adapter. The separate platform crate provides OS adapters. Holds an exclusive lock for the
/// connection lifetime; successful autocommit writes and COMMIT/outer RELEASE
/// flush the whole image. BEGIN/savepoints operate in private memory until then.
/// A constraint FAIL also flushes retained autocommit changes before returning
/// its error; a constraint ROLLBACK discards the pending transaction.
///
/// Any export or persistence failure requires closing and reopening: the failed
/// call may have committed, so inspect the reopened database before retrying.
pub struct JournaledConnection<S: Storage> {
    connection: Connection,
    pager: Pager<S>,
    page_size: usize,
    dirty: bool,
    unavailable: bool,
}
impl<S: Storage> JournaledConnection<S> {
    /// `new_page_size` applies only when the existing main file is empty.
    pub fn open(storage: S, new_page_size: usize, limits: JournalLimits) -> Result<Self> {
        ImageBuilder::new(new_page_size)?;
        let pager = Pager::open(storage, limits)?;
        let image = pager.image()?;
        let (connection, page_size) = if image.is_empty() {
            (Connection::new(), new_page_size)
        } else {
            let header = Database::parse(image)?.header().clone();
            if header.reserved_bytes != 0 {
                return Err(Error::Unsupported("journaled SQL with reserved bytes"));
            }
            (Connection::from_image(image)?, header.page_size)
        };
        Ok(Self {
            connection,
            pager,
            page_size,
            dirty: false,
            unavailable: false,
        })
    }
    fn ready(&self) -> Result<()> {
        if self.unavailable {
            return Err(Error::Storage(
                "SQL connection requires reopen after persistence failure".into(),
            ));
        }
        self.pager.image()?;
        Ok(())
    }
    pub fn prepare(&self, sql: &str) -> Result<PreparedStatement> {
        self.ready()?;
        self.connection.prepare(sql)
    }
    pub fn execute(&mut self, sql: &str, parameters: &[Value]) -> Result<QueryResult> {
        let prepared = self.prepare(sql)?;
        self.execute_prepared(&prepared, parameters)
    }
    pub fn execute_prepared(
        &mut self,
        prepared: &PreparedStatement,
        parameters: &[Value],
    ) -> Result<QueryResult> {
        self.ready()?;
        let was_autocommit = self.connection.is_autocommit();
        let result = self.connection.execute_prepared(prepared, parameters);
        let rolled_back = result.is_ok() && matches!(prepared.statement, Statement::Rollback(None))
            || result.is_err() && !was_autocommit && self.connection.is_autocommit();
        if rolled_back {
            self.dirty = false;
        } else if prepared.statement.mutating()
            && (result.is_ok() || self.connection.retained_failure_changes())
        {
            self.dirty = true;
        }
        if self.dirty && self.connection.is_autocommit() {
            self.unavailable = true;
            let image = self.connection.to_image(self.page_size)?;
            self.pager.commit(&image)?;
            self.dirty = false;
            self.unavailable = false;
        }
        result
    }
    pub fn is_autocommit(&self) -> Result<bool> {
        self.ready()?;
        Ok(self.connection.is_autocommit())
    }
    pub fn changes(&self) -> Result<usize> {
        self.ready()?;
        Ok(self.connection.changes())
    }
    pub fn total_changes(&self) -> Result<u64> {
        self.ready()?;
        Ok(self.connection.total_changes())
    }
    pub fn last_insert_rowid(&self) -> Result<i64> {
        self.ready()?;
        Ok(self.connection.last_insert_rowid())
    }
}
