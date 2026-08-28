#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![forbid(unsafe_code)]

//! Application commands, queries, use cases, and coordinator ports.
//!
//! This layer may depend on the pure domain but never on concrete storage,
//! media, analysis, or transport adapters.

pub mod auth;
pub mod cleanup;
pub mod devices;
pub mod library;
pub mod modes;
pub mod playback;
pub mod playlists;
pub mod recovery;
