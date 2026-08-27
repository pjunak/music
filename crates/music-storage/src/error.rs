use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::io;
use std::path::PathBuf;

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
    InvalidStorageRevision(i64),
    StorageRevisionOverflow,
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
            Self::InvalidStorageRevision(revision) => {
                write!(
                    formatter,
                    "storage revision must be non-negative, got {revision}"
                )
            }
            Self::StorageRevisionOverflow => formatter.write_str("storage revision overflow"),
        }
    }
}

impl Error for StorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::LockUnavailable { source, .. } => Some(source),
            Self::Database(source) => Some(source),
            Self::InvalidDatabasePath(_)
            | Self::InvalidOption(_)
            | Self::InvalidStorageRevision(_)
            | Self::StorageRevisionOverflow => None,
        }
    }
}

impl From<sqlx::Error> for StorageError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
