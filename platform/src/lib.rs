//! OS storage adapters for the safe SQLite core. This crate also forbids unsafe
//! code; rustix and the standard library are trusted syscall boundaries.
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
))]
pub mod unix;
