//! Rust source port of SQLite. The SQL engine is implemented in Rust; no C
//! SQLite library is built or linked by Cargo. See README.md for port status.
#![feature(c_variadic, extern_types)]

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
compile_error!("This generated VFS targets aarch64-apple-darwin. Regenerate and validate the port for another target before building.");

#[macro_use]
extern crate c2rust_bitfields;

mod compat;
#[rustfmt::skip]
#[allow(warnings)]
#[allow(dangerous_implicit_autorefs)]
pub mod engine;

mod connection;
pub use connection::{Connection, Error, QueryResult, Result, Statement, Transaction, Value};
