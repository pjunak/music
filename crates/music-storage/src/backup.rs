use std::fs;
use std::path::Path;

use crate::migration::hash_and_sync;
use crate::{SqliteStorage, StorageError, inspect_database};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DatabaseSnapshot {
    pub sha256: String,
    pub bytes: u64,
    pub schema_version: Option<i64>,
}

impl SqliteStorage {
    /// Create a SQLite-consistent, verified snapshot at a new destination.
    /// App-managed writes are paused only for `VACUUM INTO`; hashing and
    /// schema verification operate on the immutable snapshot afterward.
    pub async fn create_verified_snapshot(
        &self,
        destination: &Path,
    ) -> Result<DatabaseSnapshot, StorageError> {
        if path_exists(destination)? {
            return Err(StorageError::BackupTargetExists(destination.to_path_buf()));
        }
        let destination_text = destination
            .to_str()
            .ok_or_else(|| StorageError::BackupPathNotUnicode(destination.to_path_buf()))?;
        let vacuum_result = {
            let _admission = self.write_gate.lock().await;
            sqlx::query("VACUUM INTO ?")
                .bind(destination_text)
                .execute(&self.pool)
                .await
        };
        if let Err(error) = vacuum_result {
            let _ = fs::remove_file(destination);
            return Err(error.into());
        }

        let verification = match inspect_database(destination).await {
            Ok(report) => report,
            Err(error) => {
                let _ = fs::remove_file(destination);
                return Err(error);
            }
        };
        let expected = &self.migration_outcome().schema_after;
        if !verification.is_compatible()
            || verification.table_count != expected.table_count
            || verification.migration_version != expected.migration_version
        {
            let _ = fs::remove_file(destination);
            return Err(StorageError::BackupVerificationFailed {
                path: destination.to_path_buf(),
            });
        }

        let hash_path = destination.to_path_buf();
        let (sha256, bytes) =
            tokio::task::spawn_blocking(move || hash_and_sync(&hash_path)).await??;
        Ok(DatabaseSnapshot {
            sha256,
            bytes,
            schema_version: verification.migration_version,
        })
    }
}

fn path_exists(path: &Path) -> Result<bool, StorageError> {
    path.try_exists().map_err(|source| StorageError::Io {
        operation: "inspect database snapshot target",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use music_application::auth::UnixSeconds;
    use tempfile::tempdir;

    use super::SqliteStorage;
    use crate::{SchemaCompatibility, SqliteStorageOptions, StorageError};

    #[tokio::test]
    async fn snapshot_is_current_verified_and_never_overwrites() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let storage =
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?;
        storage
            .create_user("operator", "fixture-hash", UnixSeconds::new(1_800_000_000))
            .await?;
        let destination = directory.path().join("snapshot.db");

        let snapshot = storage.create_verified_snapshot(&destination).await?;

        assert!(snapshot.bytes > 0);
        assert_eq!(snapshot.sha256.len(), 64);
        let report = SqliteStorage::doctor(&destination).await?;
        assert_eq!(report.compatibility, SchemaCompatibility::Current);
        assert_eq!(report.migration_version, snapshot.schema_version);
        assert!(matches!(
            storage.create_verified_snapshot(&destination).await,
            Err(StorageError::BackupTargetExists(path)) if path == destination
        ));
        storage.close().await;
        Ok(())
    }
}
