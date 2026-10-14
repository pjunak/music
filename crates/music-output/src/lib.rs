#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![forbid(unsafe_code)]

//! Headless output appliance using the shared wire protocol and mpv IPC.
//!
//! Baton, the Android output, is a separate repository.

pub mod client;
pub mod config;
pub mod control;
pub mod mpv;
pub mod reconcile;
pub mod runtime;
