use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sqlx::{Row, SqlitePool};
use tokio::sync::Mutex;

use crate::migration::{MigrationOutcome, bootstrap};
use crate::{InstanceLock, SchemaReport, StorageError, inspect_database};

const DEFAULT_MAX_CONNECTIONS: u32 = 3;
const MAX_CONNECTIONS: u32 = 4;
const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct SqliteStorageOptions {
    database_path: PathBuf,
    max_connections: u32,
    busy_timeout: Duration,
}

impl SqliteStorageOptions {
    #[must_use]
    pub fn new(database_path: impl Into<PathBuf>) -> Self {
        Self {
            database_path: database_path.into(),
            max_connections: DEFAULT_MAX_CONNECTIONS,
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        }
    }

    pub fn with_max_connections(mut self, max_connections: u32) -> Result<Self, StorageError> {
        if !(1..=MAX_CONNECTIONS).contains(&max_connections) {
            return Err(StorageError::InvalidOption(
                "max_connections must be between 1 and 4",
            ));
        }
        self.max_connections = max_connections;
        Ok(self)
    }

    #[must_use]
    pub const fn with_busy_timeout(mut self, busy_timeout: Duration) -> Self {
        self.busy_timeout = busy_timeout;
        self
    }

    #[must_use]
    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub(crate) const fn max_connections(&self) -> u32 {
        self.max_connections
    }

    pub(crate) const fn busy_timeout(&self) -> Duration {
        self.busy_timeout
    }
}

#[derive(Debug)]
pub struct SqliteStorage {
    pool: SqlitePool,
    write_gate: Arc<Mutex<()>>,
    instance_lock: InstanceLock,
    migration_outcome: MigrationOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPlaybackSnapshot {
    pub state_json: String,
    pub storage_revision: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareAndSwap {
    Updated { storage_revision: i64 },
    Conflict,
}

impl SqliteStorage {
    pub async fn open(options: SqliteStorageOptions) -> Result<Self, StorageError> {
        ensure_parent_exists(&options.database_path)?;
        let instance_lock = InstanceLock::acquire(&options.database_path)?;
        let (pool, migration_outcome) = bootstrap(&options).await?;
        Ok(Self {
            pool,
            write_gate: Arc::new(Mutex::new(())),
            instance_lock,
            migration_outcome,
        })
    }

    pub async fn doctor(database_path: &Path) -> Result<SchemaReport, StorageError> {
        inspect_database(database_path).await
    }

    #[must_use]
    pub fn lock_path(&self) -> &Path {
        self.instance_lock.path()
    }

    #[must_use]
    pub const fn migration_outcome(&self) -> &MigrationOutcome {
        &self.migration_outcome
    }

    pub async fn healthcheck(&self) -> Result<(), StorageError> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    pub async fn sqlite_version(&self) -> Result<String, StorageError> {
        let row = sqlx::query("SELECT sqlite_version() AS version")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("version")?)
    }

    pub async fn load_playback_snapshot(
        &self,
        id: i64,
    ) -> Result<Option<StoredPlaybackSnapshot>, StorageError> {
        let row =
            sqlx::query("SELECT state_json, storage_revision FROM playback_state WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        row.map(|row| {
            Ok(StoredPlaybackSnapshot {
                state_json: row.try_get("state_json")?,
                storage_revision: row.try_get("storage_revision")?,
            })
        })
        .transpose()
    }

    pub async fn insert_playback_snapshot_if_missing(
        &self,
        id: i64,
        state_json: &str,
    ) -> Result<bool, StorageError> {
        let _admission = self.write_gate.lock().await;
        let result = sqlx::query(
            "INSERT OR IGNORE INTO playback_state \
             (id, state_json, storage_revision, updated_at) VALUES (?, ?, 0, CURRENT_TIMESTAMP)",
        )
        .bind(id)
        .bind(state_json)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn compare_and_swap_playback_snapshot(
        &self,
        id: i64,
        expected_storage_revision: i64,
        state_json: &str,
    ) -> Result<CompareAndSwap, StorageError> {
        if expected_storage_revision < 0 {
            return Err(StorageError::InvalidStorageRevision(
                expected_storage_revision,
            ));
        }
        let next_revision = expected_storage_revision
            .checked_add(1)
            .ok_or(StorageError::StorageRevisionOverflow)?;
        let _admission = self.write_gate.lock().await;
        let result = sqlx::query(
            "UPDATE playback_state SET state_json = ?, storage_revision = ?, \
             updated_at = CURRENT_TIMESTAMP WHERE id = ? AND storage_revision = ?",
        )
        .bind(state_json)
        .bind(next_revision)
        .bind(id)
        .bind(expected_storage_revision)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 1 {
            Ok(CompareAndSwap::Updated {
                storage_revision: next_revision,
            })
        } else {
            Ok(CompareAndSwap::Conflict)
        }
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }
}

fn ensure_parent_exists(database_path: &Path) -> Result<(), StorageError> {
    let Some(parent) = database_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    else {
        return Ok(());
    };
    std::fs::create_dir_all(parent).map_err(|source| StorageError::Io {
        operation: "create storage directory",
        path: parent.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::path::Path;

    use sqlx::Row;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use tempfile::tempdir;

    use super::{CompareAndSwap, SqliteStorage, SqliteStorageOptions};
    use crate::StorageError;

    const PYTHON_SQLITE_FIXTURE: &str =
        include_str!("../../../contracts/reference/v1/sqlite-fixture.sql");

    async fn open_test_storage() -> Result<(tempfile::TempDir, SqliteStorage), StorageError> {
        let directory = tempdir().map_err(|source| StorageError::Io {
            operation: "create test directory",
            path: std::env::temp_dir(),
            source,
        })?;
        let storage =
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?;
        Ok((directory, storage))
    }

    async fn create_python_fixture(path: &Path) -> Result<(), Box<dyn Error>> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        sqlx::raw_sql(PYTHON_SQLITE_FIXTURE).execute(&pool).await?;
        pool.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn configures_wal_foreign_keys_and_bounded_pool() -> Result<(), Box<dyn Error>> {
        let (_directory, storage) = open_test_storage().await?;
        storage.healthcheck().await?;
        assert!(!storage.sqlite_version().await?.is_empty());

        let foreign_keys: i64 = sqlx::query("PRAGMA foreign_keys")
            .fetch_one(&storage.pool)
            .await?
            .try_get(0)?;
        let journal_mode: String = sqlx::query("PRAGMA journal_mode")
            .fetch_one(&storage.pool)
            .await?
            .try_get(0)?;
        assert_eq!(foreign_keys, 1);
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        assert!(storage.pool.options().get_max_connections() <= 4);
        assert_eq!(
            storage.migration_outcome.schema_after.compatibility,
            crate::SchemaCompatibility::Current
        );
        Ok(())
    }

    #[tokio::test]
    async fn playback_snapshot_compare_and_swap_rejects_stale_writes() -> Result<(), Box<dyn Error>>
    {
        let (_directory, storage) = open_test_storage().await?;
        assert!(
            storage
                .insert_playback_snapshot_if_missing(1, r#"{"revision":0}"#)
                .await?
        );
        assert!(
            !storage
                .insert_playback_snapshot_if_missing(1, r#"{"revision":99}"#)
                .await?
        );

        assert_eq!(
            storage
                .compare_and_swap_playback_snapshot(1, 0, r#"{"revision":1}"#)
                .await?,
            CompareAndSwap::Updated {
                storage_revision: 1
            }
        );
        assert_eq!(
            storage
                .compare_and_swap_playback_snapshot(1, 0, r#"{"revision":2}"#)
                .await?,
            CompareAndSwap::Conflict
        );

        let snapshot = storage.load_playback_snapshot(1).await?;
        assert_eq!(
            snapshot,
            Some(super::StoredPlaybackSnapshot {
                state_json: r#"{"revision":1}"#.to_owned(),
                storage_revision: 1,
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_compare_and_swap_has_one_winner() -> Result<(), Box<dyn Error>> {
        let (_directory, storage) = open_test_storage().await?;
        storage.insert_playback_snapshot_if_missing(1, "{}").await?;
        let storage = std::sync::Arc::new(storage);
        let mut tasks = Vec::new();
        for candidate in 0..16 {
            let storage = std::sync::Arc::clone(&storage);
            tasks.push(tokio::spawn(async move {
                storage
                    .compare_and_swap_playback_snapshot(
                        1,
                        0,
                        &format!(r#"{{"candidate":{candidate}}}"#),
                    )
                    .await
            }));
        }

        let mut winners = 0;
        for task in tasks {
            if matches!(task.await??, CompareAndSwap::Updated { .. }) {
                winners += 1;
            }
        }
        assert_eq!(winners, 1);
        Ok(())
    }

    #[tokio::test]
    async fn migrates_python_schema_after_verified_backup() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let database_path = directory.path().join("app.db");
        create_python_fixture(&database_path).await?;
        let storage = SqliteStorage::open(SqliteStorageOptions::new(&database_path)).await?;

        let table_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
               AND name != '_sqlx_migrations'",
        )
        .fetch_one(&storage.pool)
        .await?;
        assert_eq!(table_count, 21);

        let foreign_key_failures = sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&storage.pool)
            .await?;
        assert!(foreign_key_failures.is_empty());

        let row = sqlx::query("SELECT username, created_at FROM users WHERE id = 1")
            .fetch_one(&storage.pool)
            .await?;
        assert_eq!(row.try_get::<String, _>("username")?, "fixture-user");
        assert_eq!(
            row.try_get::<String, _>("created_at")?,
            "2026-08-27 12:34:56.000000"
        );
        let storage_revision: i64 =
            sqlx::query_scalar("SELECT storage_revision FROM playback_state WHERE id = 1")
                .fetch_one(&storage.pool)
                .await?;
        assert_eq!(storage_revision, 0);

        let outcome = storage.migration_outcome();
        assert!(outcome.migration_applied);
        assert_eq!(
            outcome.schema_before.compatibility,
            crate::SchemaCompatibility::CompatibleLegacy
        );
        assert_eq!(
            outcome.schema_after.compatibility,
            crate::SchemaCompatibility::Current
        );
        let backup = outcome.backup.as_ref().ok_or("expected migration backup")?;
        assert!(backup.database_path.is_file());
        assert!(backup.manifest_path.is_file());
        assert_eq!(backup.sha256.len(), 64);
        assert!(backup.bytes > 0);

        let backup_report = SqliteStorage::doctor(&backup.database_path).await?;
        assert_eq!(backup_report.table_count, 18);
        assert_eq!(
            backup_report.compatibility,
            crate::SchemaCompatibility::CompatibleLegacy
        );
        let backup_options = SqliteConnectOptions::new()
            .filename(&backup.database_path)
            .read_only(true);
        let backup_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(backup_options)
            .await?;
        let migrated_column_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('playback_state') \
             WHERE name = 'storage_revision'",
        )
        .fetch_one(&backup_pool)
        .await?;
        assert_eq!(migrated_column_count, 0);
        backup_pool.close().await;

        storage.close().await;
        drop(storage);
        let reopened = SqliteStorage::open(SqliteStorageOptions::new(&database_path)).await?;
        assert!(!reopened.migration_outcome().migration_applied);
        assert!(reopened.migration_outcome().backup.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn refuses_unknown_schema_without_creating_a_backup() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let database_path = directory.path().join("app.db");
        let options = SqliteConnectOptions::new()
            .filename(&database_path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        sqlx::query("CREATE TABLE surprise (value TEXT)")
            .execute(&pool)
            .await?;
        pool.close().await;

        let result = SqliteStorage::open(SqliteStorageOptions::new(&database_path)).await;

        assert!(matches!(result, Err(StorageError::IncompatibleSchema(_))));
        let sibling_names = std::fs::read_dir(directory.path())?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<Vec<_>, _>>()?;
        assert!(
            !sibling_names
                .iter()
                .any(|name| { name.to_string_lossy().contains(".pre-rust-v") })
        );
        Ok(())
    }
}
