//! Safe, bounded SQL execution with private-memory and experimental journaled APIs.
//! OS adapters live in the separate platform crate; the C ABI is unfinished.
#![doc = include_str!("../../SQL.md")]
mod connection;
mod eval;
mod journaled;
mod lexer;
mod parser;
mod scalar;
mod sort;

use crate::Error;
use alloc::string::String;
pub use connection::{Connection, QueryResult};
pub use journaled::JournaledConnection;
pub use parser::PreparedStatement;

#[derive(Debug, Clone, Copy)]
pub struct SqlLimits {
    pub max_sql_bytes: usize,
    pub max_tokens: usize,
    pub max_expr_depth: usize,
    pub max_rows: usize,
    pub max_value_bytes: usize,
    pub max_database_bytes: usize,
    pub max_steps: usize,
    pub max_parameters: usize,
    pub max_savepoints: usize,
}
impl Default for SqlLimits {
    fn default() -> Self {
        Self {
            max_sql_bytes: 1024 * 1024,
            max_tokens: 32768,
            max_expr_depth: 64,
            max_rows: 100_000,
            max_value_bytes: 16 * 1024 * 1024,
            max_database_bytes: 64 * 1024 * 1024,
            max_steps: 2_000_000,
            max_parameters: 32766,
            max_savepoints: 16,
        }
    }
}
fn error(message: impl Into<String>) -> Error {
    Error::Sql(message.into())
}
