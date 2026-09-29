//! Independently implemented, memory-safe SQLite storage and SQL components.
//!
//! This is an incomplete rewrite with an experimental in-memory SQL engine.
//! It has no dependency on
//! the legacy translated engine, libc, FFI, native SQLite, or third-party crates.
//! All parsers operate on immutable snapshots and enforce configurable budgets.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod database;
pub mod error;
pub mod header;
pub mod journal;
pub mod pager;
pub mod record;
pub mod sql;
pub mod varint;
pub mod wal;
pub mod writer;

pub use database::{Database, Limits, PageKind, Row, SchemaEntry};
pub use error::{Error, Result};
pub use header::{AutoVacuum, Encoding, Header};
pub use record::{Text, Value};
pub use writer::{ImageBuilder, Table};
