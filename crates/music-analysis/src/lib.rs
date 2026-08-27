#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![forbid(unsafe_code)]

//! Streaming signal analysis and bounded voice-inference adapters.
//!
//! CPU work runs only on the dedicated fixed analysis executor; model weights
//! remain optional, checksum-pinned, operator supplied, and non-fatal.
