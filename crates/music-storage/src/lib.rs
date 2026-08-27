#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![forbid(unsafe_code)]

//! SQLite migrations, row mappings, repositories, and transaction ownership.
//!
//! This crate owns the small read pool, serialized short-write admission, and
//! exclusive process/offline-writer lock.

mod auth;
mod crypto;
mod devices;
mod error;
mod instance_lock;
mod library;
mod migration;
mod playback;
mod schema;
mod sqlite;

pub use auth::Argon2PasswordVerifier;
pub use crypto::{
    CredentialVault, CryptoError, EncryptedCredential, SecretString, hash_password, verify_password,
};
pub use devices::{
    DeviceExportOutcome, DeviceImportOutcome, LegacyDeviceImportOutcome, LegacyDeviceImportRecord,
    LegacyDeviceImportStatus,
};
pub use error::StorageError;
pub use instance_lock::InstanceLock;
pub use migration::{MigrationBackup, MigrationOutcome};
pub use schema::{
    CURRENT_SCHEMA_VERSION, SchemaCompatibility, SchemaIssue, SchemaIssueLevel, SchemaReport,
    inspect_database,
};
pub use sqlite::{CompareAndSwap, SqliteStorage, SqliteStorageOptions, StoredPlaybackSnapshot};
