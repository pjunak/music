use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use crate::StorageError;

#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}

impl InstanceLock {
    pub fn acquire(database_path: &Path) -> Result<Self, StorageError> {
        let path = lock_path(database_path)?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| StorageError::Io {
                operation: "open storage lock",
                path: path.clone(),
                source,
            })?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(StorageError::LockUnavailable {
                    path,
                    source: io::ErrorKind::WouldBlock.into(),
                });
            }
            Err(TryLockError::Error(source)) => {
                return Err(StorageError::Io {
                    operation: "acquire storage lock",
                    path,
                    source,
                });
            }
        }
        Ok(Self { _file: file, path })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn lock_path(database_path: &Path) -> Result<PathBuf, StorageError> {
    let Some(file_name) = database_path.file_name() else {
        return Err(StorageError::InvalidDatabasePath(
            database_path.to_path_buf(),
        ));
    };
    let mut lock_name = OsString::from(file_name);
    lock_name.push(".lock");
    Ok(database_path.with_file_name(lock_name))
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use tempfile::tempdir;

    use super::InstanceLock;
    use crate::StorageError;

    #[test]
    fn refuses_a_second_owner_and_releases_on_drop() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let database_path = directory.path().join("app.db");
        let first = InstanceLock::acquire(&database_path)?;

        assert!(matches!(
            InstanceLock::acquire(&database_path),
            Err(StorageError::LockUnavailable { .. })
        ));

        drop(first);
        let reacquired = InstanceLock::acquire(&database_path)?;
        assert_eq!(reacquired.path(), directory.path().join("app.db.lock"));
        Ok(())
    }
}
