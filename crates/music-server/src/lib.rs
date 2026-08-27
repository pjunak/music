#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![forbid(unsafe_code)]

//! Transport adapters and explicit application composition.
//!
//! The future `AppRuntime` is constructed once here. HTTP handlers translate
//! and authorize; they never become alternate owners of mutable application
//! state.
