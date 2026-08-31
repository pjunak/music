use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::migrate::{Migration, MigrationType, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Executor, SqlSafeStr, SqlitePool};

use crate::schema::{CURRENT_SCHEMA_VERSION, SchemaCompatibility, SchemaReport, inspect_database};
use crate::{SqliteStorageOptions, StorageError};

const BASELINE_MIGRATION_SQL: &str = include_str!("../migrations/0001_rust_baseline.sql");
const LIBRARY_STATE_MIGRATION_SQL: &str = include_str!("../migrations/0002_library_state.sql");
const LIBRARY_CATALOG_COUNT_MIGRATION_SQL: &str =
    include_str!("../migrations/0003_library_catalog_count.sql");
const DURABLE_JOBS_MIGRATION_SQL: &str = include_str!("../migrations/0004_durable_jobs.sql");
const LEGACY_ALEMBIC_CLEANUP_MIGRATION_SQL: &str =
    include_str!("../migrations/0005_remove_legacy_alembic_ledger.sql");
const LEGACY_KNOWN_DEVICES_CLEANUP_MIGRATION_SQL: &str =
    include_str!("../migrations/0006_remove_legacy_known_devices.sql");
const CLEANUP_SOURCE_POLICIES_MIGRATION_SQL: &str =
    include_str!("../migrations/0007_cleanup_source_policies.sql");

const BACKUP_KIND: &str = "pre-rust-migration";
const BACKUP_FORMAT_VERSION: u8 = 1;
const BACKUP_NAME_ATTEMPTS: u16 = 100;
const BASELINE_MIGRATION_VERSION: i64 = 1;
const LIBRARY_STATE_MIGRATION_VERSION: i64 = 2;
const LIBRARY_CATALOG_COUNT_MIGRATION_VERSION: i64 = 3;
const DURABLE_JOBS_MIGRATION_VERSION: i64 = 4;
const LEGACY_ALEMBIC_CLEANUP_MIGRATION_VERSION: i64 = 5;
const LEGACY_KNOWN_DEVICES_CLEANUP_MIGRATION_VERSION: i64 = 6;
const CLEANUP_SOURCE_POLICIES_MIGRATION_VERSION: i64 = 7;

const ADDITIVE_MIGRATIONS: &[(&str, &str, &str)] = &[
    (
        "tracks",
        "display_title",
        "ALTER TABLE tracks ADD COLUMN display_title VARCHAR(512) NOT NULL DEFAULT ''",
    ),
    (
        "tracks",
        "origin",
        "ALTER TABLE tracks ADD COLUMN origin VARCHAR(512) NOT NULL DEFAULT ''",
    ),
    (
        "track_analyses",
        "metrics_json",
        "ALTER TABLE track_analyses ADD COLUMN metrics_json TEXT NOT NULL DEFAULT '{}'",
    ),
    (
        "assistant_model_roles",
        "conformance_status",
        "ALTER TABLE assistant_model_roles ADD COLUMN conformance_status VARCHAR(16) NOT NULL DEFAULT 'never'",
    ),
    (
        "assistant_model_roles",
        "conformance_error_code",
        "ALTER TABLE assistant_model_roles ADD COLUMN conformance_error_code VARCHAR(64)",
    ),
    (
        "assistant_model_roles",
        "conformance_fingerprint",
        "ALTER TABLE assistant_model_roles ADD COLUMN conformance_fingerprint VARCHAR(64)",
    ),
    (
        "assistant_model_roles",
        "last_conformance_at",
        "ALTER TABLE assistant_model_roles ADD COLUMN last_conformance_at DATETIME",
    ),
    (
        "assistant_model_roles",
        "thinking_mode",
        "ALTER TABLE assistant_model_roles ADD COLUMN thinking_mode VARCHAR(24) NOT NULL DEFAULT 'provider_default'",
    ),
    (
        "assistant_provider_connections",
        "verified_capabilities_json",
        "ALTER TABLE assistant_provider_connections ADD COLUMN verified_capabilities_json TEXT NOT NULL DEFAULT '[]'",
    ),
    (
        "playlists",
        "automatic_rule_json",
        "ALTER TABLE playlists ADD COLUMN automatic_rule_json TEXT NOT NULL DEFAULT ''",
    ),
    (
        "playlists",
        "automatic_source_signature",
        "ALTER TABLE playlists ADD COLUMN automatic_source_signature VARCHAR(64)",
    ),
    (
        "playlists",
        "automatic_refreshed_at",
        "ALTER TABLE playlists ADD COLUMN automatic_refreshed_at DATETIME",
    ),
    (
        "assistant_tag_vocabularies",
        "seed_version",
        "ALTER TABLE assistant_tag_vocabularies ADD COLUMN seed_version INTEGER NOT NULL DEFAULT 1",
    ),
    (
        "playback_state",
        "storage_revision",
        "ALTER TABLE playback_state ADD COLUMN storage_revision INTEGER NOT NULL DEFAULT 0",
    ),
];

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct MigrationBackup {
    pub database_path: PathBuf,
    pub manifest_path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct MigrationOutcome {
    pub schema_before: SchemaReport,
    pub schema_after: SchemaReport,
    pub backup: Option<MigrationBackup>,
    pub migration_applied: bool,
}

#[derive(Debug, Serialize)]
struct BackupManifest {
    format_version: u8,
    kind: &'static str,
    source_file_name: String,
    backup_file_name: String,
    sha256: String,
    bytes: u64,
    created_unix_seconds: u64,
    schema_compatibility_before: SchemaCompatibility,
    schema_version_before: Option<i64>,
    schema_version_target: i64,
}

pub(crate) async fn bootstrap(
    options: &SqliteStorageOptions,
) -> Result<(SqlitePool, MigrationOutcome), StorageError> {
    let schema_before = inspect_database(options.database_path()).await?;
    if !schema_before.is_compatible() {
        return Err(StorageError::IncompatibleSchema(Box::new(schema_before)));
    }

    let backup = if schema_before.database_exists && schema_before.migration_required {
        Some(create_verified_backup(options.database_path(), &schema_before).await?)
    } else {
        None
    };

    let connect_options = SqliteConnectOptions::new()
        .filename(options.database_path())
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(options.busy_timeout());
    let pool = SqlitePoolOptions::new()
        .max_connections(options.max_connections())
        .connect_with(connect_options)
        .await?;

    let migration_result = async {
        if schema_before.migration_required {
            normalize_legacy_schema(&pool).await?;
        }
        migrator().run(&pool).await?;
        let schema_after = crate::schema::inspect_pool(&pool, true).await?;
        if schema_after.compatibility != SchemaCompatibility::Current {
            return Err(StorageError::IncompatibleSchema(Box::new(schema_after)));
        }
        Ok(schema_after)
    }
    .await;

    match migration_result {
        Ok(schema_after) => Ok((
            pool,
            MigrationOutcome {
                migration_applied: schema_before.migration_required,
                schema_before,
                schema_after,
                backup,
            },
        )),
        Err(error) => {
            pool.close().await;
            Err(error)
        }
    }
}

fn migrator() -> Migrator {
    Migrator::with_migrations(vec![
        Migration::new(
            BASELINE_MIGRATION_VERSION,
            "rust baseline".into(),
            MigrationType::Simple,
            BASELINE_MIGRATION_SQL.into_sql_str(),
            false,
        ),
        Migration::new(
            LIBRARY_STATE_MIGRATION_VERSION,
            "library reconciliation state".into(),
            MigrationType::Simple,
            LIBRARY_STATE_MIGRATION_SQL.into_sql_str(),
            false,
        ),
        Migration::new(
            LIBRARY_CATALOG_COUNT_MIGRATION_VERSION,
            "library catalog count backfill".into(),
            MigrationType::Simple,
            LIBRARY_CATALOG_COUNT_MIGRATION_SQL.into_sql_str(),
            false,
        ),
        Migration::new(
            DURABLE_JOBS_MIGRATION_VERSION,
            "durable job execution leases".into(),
            MigrationType::Simple,
            DURABLE_JOBS_MIGRATION_SQL.into_sql_str(),
            false,
        ),
        Migration::new(
            LEGACY_ALEMBIC_CLEANUP_MIGRATION_VERSION,
            "remove legacy alembic ledger".into(),
            MigrationType::Simple,
            LEGACY_ALEMBIC_CLEANUP_MIGRATION_SQL.into_sql_str(),
            false,
        ),
        Migration::new(
            LEGACY_KNOWN_DEVICES_CLEANUP_MIGRATION_VERSION,
            "remove legacy known devices table".into(),
            MigrationType::Simple,
            LEGACY_KNOWN_DEVICES_CLEANUP_MIGRATION_SQL.into_sql_str(),
            false,
        ),
        Migration::new(
            CLEANUP_SOURCE_POLICIES_MIGRATION_VERSION,
            "cleanup source policies".into(),
            MigrationType::Simple,
            CLEANUP_SOURCE_POLICIES_MIGRATION_SQL.into_sql_str(),
            false,
        ),
    ])
}

async fn normalize_legacy_schema(pool: &SqlitePool) -> Result<(), StorageError> {
    let mut transaction = pool.begin().await?;
    for (table, column, statement) in ADDITIVE_MIGRATIONS {
        let table_exists = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = ?",
        )
        .bind(table)
        .fetch_one(&mut *transaction)
        .await?
            != 0;
        if !table_exists {
            continue;
        }
        let column_exists = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pragma_table_info(?) WHERE name = ?",
        )
        .bind(table)
        .bind(column)
        .fetch_one(&mut *transaction)
        .await?
            != 0;
        if !column_exists {
            transaction.execute(*statement).await?;
        }
    }
    transaction.commit().await?;
    Ok(())
}

async fn create_verified_backup(
    database_path: &Path,
    schema_before: &SchemaReport,
) -> Result<MigrationBackup, StorageError> {
    let paths = reserve_backup_paths(database_path)?;
    let temporary_text = paths
        .temporary_database
        .to_str()
        .ok_or_else(|| StorageError::BackupPathNotUnicode(paths.temporary_database.clone()))?;
    let options = SqliteConnectOptions::new()
        .filename(database_path)
        .read_only(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    let vacuum_result = sqlx::query("VACUUM INTO ?")
        .bind(temporary_text)
        .execute(&pool)
        .await;
    pool.close().await;
    if let Err(error) = vacuum_result {
        let _ = fs::remove_file(&paths.temporary_database);
        return Err(error.into());
    }

    let verification = inspect_database(&paths.temporary_database).await?;
    if !verification.is_compatible()
        || verification.table_count != schema_before.table_count
        || verification.migration_version != schema_before.migration_version
    {
        let _ = fs::remove_file(&paths.temporary_database);
        return Err(StorageError::BackupVerificationFailed {
            path: paths.temporary_database,
        });
    }

    let hash_path = paths.temporary_database.clone();
    let (sha256, bytes) = tokio::task::spawn_blocking(move || hash_and_sync(&hash_path)).await??;
    fs::rename(&paths.temporary_database, &paths.database).map_err(|source| StorageError::Io {
        operation: "publish migration backup",
        path: paths.database.clone(),
        source,
    })?;

    let created_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let manifest = BackupManifest {
        format_version: BACKUP_FORMAT_VERSION,
        kind: BACKUP_KIND,
        source_file_name: file_name_text(database_path),
        backup_file_name: file_name_text(&paths.database),
        sha256: sha256.clone(),
        bytes,
        created_unix_seconds,
        schema_compatibility_before: schema_before.compatibility,
        schema_version_before: schema_before.migration_version,
        schema_version_target: CURRENT_SCHEMA_VERSION,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    let manifest_path = paths.manifest.clone();
    tokio::task::spawn_blocking(move || write_new_synced(&manifest_path, &manifest_bytes))
        .await??;

    Ok(MigrationBackup {
        database_path: paths.database,
        manifest_path: paths.manifest,
        sha256,
        bytes,
    })
}

#[derive(Debug)]
struct BackupPaths {
    temporary_database: PathBuf,
    database: PathBuf,
    manifest: PathBuf,
}

fn reserve_backup_paths(database_path: &Path) -> Result<BackupPaths, StorageError> {
    let Some(file_name) = database_path.file_name() else {
        return Err(StorageError::InvalidDatabasePath(
            database_path.to_path_buf(),
        ));
    };
    let created_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let process_id = std::process::id();
    for attempt in 0..BACKUP_NAME_ATTEMPTS {
        let mut backup_name = OsString::from(file_name);
        backup_name.push(format!(
            ".pre-rust-v{CURRENT_SCHEMA_VERSION}-{created_unix_seconds}-{process_id}-{attempt}.bak"
        ));
        let database = database_path.with_file_name(&backup_name);
        let mut temporary_name = OsString::from(&backup_name);
        temporary_name.push(".tmp");
        let temporary_database = database_path.with_file_name(temporary_name);
        let mut manifest_name = OsString::from(&backup_name);
        manifest_name.push(".json");
        let manifest = database_path.with_file_name(manifest_name);
        if !path_exists(&database)?
            && !path_exists(&temporary_database)?
            && !path_exists(&manifest)?
        {
            return Ok(BackupPaths {
                temporary_database,
                database,
                manifest,
            });
        }
    }
    Err(StorageError::Io {
        operation: "reserve a unique migration backup name",
        path: database_path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "migration backup name space exhausted",
        ),
    })
}

fn path_exists(path: &Path) -> Result<bool, StorageError> {
    path.try_exists().map_err(|source| StorageError::Io {
        operation: "inspect migration backup target",
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn hash_and_sync(path: &Path) -> Result<(String, u64), StorageError> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|source| StorageError::Io {
            operation: "open migration backup for verification",
            path: path.to_path_buf(),
            source,
        })?;
    file.sync_all().map_err(|source| StorageError::Io {
        operation: "sync migration backup",
        path: path.to_path_buf(),
        source,
    })?;
    let bytes = file
        .metadata()
        .map_err(|source| StorageError::Io {
            operation: "read migration backup metadata",
            path: path.to_path_buf(),
            source,
        })?
        .len();
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|source| StorageError::Io {
            operation: "hash migration backup",
            path: path.to_path_buf(),
            source,
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok((format!("{:x}", digest.finalize()), bytes))
}

fn write_new_synced(path: &Path, content: &[u8]) -> Result<(), StorageError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|source| StorageError::Io {
            operation: "create migration backup manifest",
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(content).map_err(|source| StorageError::Io {
        operation: "write migration backup manifest",
        path: path.to_path_buf(),
        source,
    })?;
    file.write_all(b"\n").map_err(|source| StorageError::Io {
        operation: "finish migration backup manifest",
        path: path.to_path_buf(),
        source,
    })?;
    file.sync_all().map_err(|source| StorageError::Io {
        operation: "sync migration backup manifest",
        path: path.to_path_buf(),
        source,
    })
}

fn file_name_text(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        BASELINE_MIGRATION_SQL, BASELINE_MIGRATION_VERSION, CLEANUP_SOURCE_POLICIES_MIGRATION_SQL,
        CLEANUP_SOURCE_POLICIES_MIGRATION_VERSION, DURABLE_JOBS_MIGRATION_SQL,
        DURABLE_JOBS_MIGRATION_VERSION, LEGACY_ALEMBIC_CLEANUP_MIGRATION_SQL,
        LEGACY_ALEMBIC_CLEANUP_MIGRATION_VERSION, LEGACY_KNOWN_DEVICES_CLEANUP_MIGRATION_SQL,
        LEGACY_KNOWN_DEVICES_CLEANUP_MIGRATION_VERSION, LIBRARY_CATALOG_COUNT_MIGRATION_SQL,
        LIBRARY_CATALOG_COUNT_MIGRATION_VERSION, LIBRARY_STATE_MIGRATION_SQL,
        LIBRARY_STATE_MIGRATION_VERSION, migrator,
    };

    #[test]
    fn embedded_migration_is_cross_platform_stable() {
        assert_eq!(BASELINE_MIGRATION_VERSION, 1);
        assert!(!BASELINE_MIGRATION_SQL.contains('\r'));
        assert!(migrator().version_exists(BASELINE_MIGRATION_VERSION));
        assert_eq!(LIBRARY_STATE_MIGRATION_VERSION, 2);
        assert!(!LIBRARY_STATE_MIGRATION_SQL.contains('\r'));
        assert!(migrator().version_exists(LIBRARY_STATE_MIGRATION_VERSION));
        assert_eq!(LIBRARY_CATALOG_COUNT_MIGRATION_VERSION, 3);
        assert!(!LIBRARY_CATALOG_COUNT_MIGRATION_SQL.contains('\r'));
        assert!(migrator().version_exists(LIBRARY_CATALOG_COUNT_MIGRATION_VERSION));
        assert_eq!(DURABLE_JOBS_MIGRATION_VERSION, 4);
        assert!(!DURABLE_JOBS_MIGRATION_SQL.contains('\r'));
        assert!(migrator().version_exists(DURABLE_JOBS_MIGRATION_VERSION));
        assert_eq!(LEGACY_ALEMBIC_CLEANUP_MIGRATION_VERSION, 5);
        assert!(!LEGACY_ALEMBIC_CLEANUP_MIGRATION_SQL.contains('\r'));
        assert!(migrator().version_exists(LEGACY_ALEMBIC_CLEANUP_MIGRATION_VERSION));
        assert_eq!(LEGACY_KNOWN_DEVICES_CLEANUP_MIGRATION_VERSION, 6);
        assert!(!LEGACY_KNOWN_DEVICES_CLEANUP_MIGRATION_SQL.contains('\r'));
        assert!(migrator().version_exists(LEGACY_KNOWN_DEVICES_CLEANUP_MIGRATION_VERSION));
        assert_eq!(CLEANUP_SOURCE_POLICIES_MIGRATION_VERSION, 7);
        assert!(!CLEANUP_SOURCE_POLICIES_MIGRATION_SQL.contains('\r'));
        assert!(migrator().version_exists(CLEANUP_SOURCE_POLICIES_MIGRATION_VERSION));
    }
}
