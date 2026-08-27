#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![forbid(unsafe_code)]

//! Pure domain types and deterministic rules.
//!
//! This crate must remain independent of async runtimes, databases, HTTP,
//! filesystems, processes, and public wire compatibility types.
