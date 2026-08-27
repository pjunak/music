use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::io;
use std::path::PathBuf;

use crate::schema::SchemaReport;

#[derive(Debug)]
pub enum StorageError {
    InvalidDatabasePath(PathBuf),
    InvalidOption(&'static str),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    LockUnavailable {
        path: PathBuf,
        source: io::Error,
    },
    Database(sqlx::Error),
    Migration(sqlx::migrate::MigrateError),
    IncompatibleSchema(Box<SchemaReport>),
    BackupPathNotUnicode(PathBuf),
    BackupVerificationFailed {
        path: PathBuf,
    },
    BackgroundTask(tokio::task::JoinError),
    ManifestSerialization(serde_json::Error),
    InvalidStorageRevision(i64),
    StorageRevisionOverflow,
    InvalidTimestamp,
}

impl Display for StorageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDatabasePath(path) => {
                write!(
                    formatter,
                    "database path has no file name: {}",
                    path.display()
                )
            }
            Self::InvalidOption(option) => write!(formatter, "invalid storage option: {option}"),
            Self::Io {
                operation, path, ..
            } => write!(formatter, "failed to {operation} {}", path.display()),
            Self::LockUnavailable { path, .. } => {
                write!(formatter, "storage is already owned: {}", path.display())
            }
            Self::Database(_) => formatter.write_str("SQLite operation failed"),
            Self::Migration(_) => formatter.write_str("SQLite migration failed"),
            Self::IncompatibleSchema(report) => {
                if let Some(issue) = report.errors().next() {
                    write!(
                        formatter,
                        "database schema is incompatible: {}",
                        issue.detail
                    )
                } else {
                    formatter.write_str("database schema is incompatible")
                }
            }
            Self::BackupPathNotUnicode(path) => write!(
                formatter,
                "migration backup path is not valid Unicode: {}",
                path.display()
            ),
            Self::BackupVerificationFailed { path } => write!(
                formatter,
                "migration backup failed verification: {}",
                path.display()
            ),
            Self::BackgroundTask(_) => {
                formatter.write_str("blocking storage operation did not complete")
            }
            Self::ManifestSerialization(_) => {
                formatter.write_str("failed to encode the migration backup manifest")
            }
            Self::InvalidStorageRevision(revision) => {
                write!(
                    formatter,
                    "storage revision must be non-negative, got {revision}"
                )
            }
            Self::StorageRevisionOverflow => formatter.write_str("storage revision overflow"),
            Self::InvalidTimestamp => formatter.write_str("stored timestamp is invalid"),
        }
    }
}

impl Error for StorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::LockUnavailable { source, .. } => Some(source),
            Self::Database(source) => Some(source),
            Self::Migration(source) => Some(source),
            Self::BackgroundTask(source) => Some(source),
            Self::ManifestSerialization(source) => Some(source),
            Self::InvalidDatabasePath(_)
            | Self::InvalidOption(_)
            | Self::IncompatibleSchema(_)
            | Self::BackupPathNotUnicode(_)
            | Self::BackupVerificationFailed { .. }
            | Self::InvalidStorageRevision(_)
            | Self::StorageRevisionOverflow
            | Self::InvalidTimestamp => None,
        }
    }
}

impl From<sqlx::Error> for StorageError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl From<sqlx::migrate::MigrateError> for StorageError {
    fn from(error: sqlx::migrate::MigrateError) -> Self {
        Self::Migration(error)
    }
}

impl From<tokio::task::JoinError> for StorageError {
    fn from(error: tokio::task::JoinError) -> Self {
        Self::BackgroundTask(error)
    }
}

impl From<serde_json::Error> for StorageError {
    fn from(error: serde_json::Error) -> Self {
        Self::ManifestSerialization(error)
    }
}
