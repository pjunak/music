#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![forbid(unsafe_code)]

//! SQLite migrations, row mappings, repositories, and transaction ownership.
//!
//! This crate owns the small read pool, serialized short-write admission, and
//! exclusive process/offline-writer lock.

mod error;
mod instance_lock;
mod sqlite;

pub use error::StorageError;
pub use instance_lock::InstanceLock;
pub use sqlite::{CompareAndSwap, SqliteStorage, SqliteStorageOptions, StoredPlaybackSnapshot};
