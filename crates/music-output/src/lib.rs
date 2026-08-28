#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![forbid(unsafe_code)]

//! Rust headless output appliance using the shared wire protocol and mpv IPC.
//!
//! This is the replacement for the Python appliance; Baton remains a separate
//! repository and is not implemented here.

pub mod client;
pub mod config;
pub mod control;
pub mod mpv;
pub mod reconcile;
pub mod runtime;
