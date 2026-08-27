#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![forbid(unsafe_code)]

//! Public HTTP and WebSocket data-transfer contracts.
//!
//! Wire compatibility remains independent of internal domain invariants and
//! is translated explicitly at transport boundaries.
