use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::migrate::{Migration, MigrationType, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{SqlSafeStr, SqlitePool};

use crate::schema::{CURRENT_SCHEMA_VERSION, SchemaCompatibility, SchemaReport, inspect_database};
use crate::{SqliteStorageOptions, StorageError};

/// Every embedded migration, in order. Applied migrations are immutable: SQLx
/// stores each checksum and refuses a database whose history differs.
pub(crate) const MIGRATIONS: &[(i64, &str, &str)] = &[
    (
        1,
        "rust baseline",
        include_str!("../migrations/0001_rust_baseline.sql"),
    ),
    (
        2,
        "library reconciliation state",
        include_str!("../migrations/0002_library_state.sql"),
    ),
    (
        3,
        "library catalog count backfill",
        include_str!("../migrations/0003_library_catalog_count.sql"),
    ),
    (
        4,
        "durable job execution leases",
        include_str!("../migrations/0004_durable_jobs.sql"),
    ),
    (
        5,
        "remove legacy alembic ledger",
        include_str!("../migrations/0005_remove_legacy_alembic_ledger.sql"),
    ),
    (
        6,
        "remove legacy known devices table",
        include_str!("../migrations/0006_remove_legacy_known_devices.sql"),
    ),
    (
        7,
        "cleanup source policies",
        include_str!("../migrations/0007_cleanup_source_policies.sql"),
    ),
    (
        8,
        "cleanup track enrichment cache",
        include_str!("../migrations/0008_cleanup_enrichments.sql"),
    ),
    (
        9,
        "encrypted cleanup source credentials",
        include_str!("../migrations/0009_cleanup_source_credentials.sql"),
    ),
    (
        10,
        "catalog evidence provenance",
        include_str!("../migrations/0010_catalog_evidence_revision.sql"),
    ),
    (
        11,
        "hashed sessions and independent management IDs",
        include_str!("../migrations/0011_hashed_sessions.sql"),
    ),
    (
        12,
        "durable model batches",
        include_str!("../migrations/0012_model_batches.sql"),
    ),
    (
        13,
        "recoverable cleanup rejections",
        include_str!("../migrations/0013_cleanup_rejections.sql"),
    ),
    (
        14,
        "rich track metadata",
        include_str!("../migrations/0014_rich_track_metadata.sql"),
    ),
    (
        15,
        "song evidence rebuild",
        include_str!("../migrations/0015_song_evidence_rebuild.sql"),
    ),
    (
        16,
        "per-tag decisions",
        include_str!("../migrations/0016_tag_decisions.sql"),
    ),
    (
        17,
        "remove legacy device import ledger",
        include_str!("../migrations/0017_remove_legacy_device_imports.sql"),
    ),
];

const BACKUP_KIND: &str = "pre-migration";
const BACKUP_FORMAT_VERSION: u8 = 1;
const BACKUP_NAME_ATTEMPTS: u16 = 100;

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

pub(crate) fn migrator() -> Migrator {
    Migrator::with_migrations(
        MIGRATIONS
            .iter()
            .map(|(version, description, sql)| {
                Migration::new(
                    *version,
                    (*description).into(),
                    MigrationType::Simple,
                    sql.into_sql_str(),
                    false,
                )
            })
            .collect(),
    )
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
            ".pre-v{CURRENT_SCHEMA_VERSION}-{created_unix_seconds}-{process_id}-{attempt}.bak"
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
    Ok((hex::encode(digest.finalize()), bytes))
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
    use super::{MIGRATIONS, migrator};

    #[tokio::test]
    async fn rich_metadata_migrates_years_and_backs_up_the_old_schema()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::{SqliteStorage, SqliteStorageOptions};
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("v13.db");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true),
            )
            .await?;
        let old = sqlx::migrate::Migrator::with_migrations(
            migrator()
                .iter()
                .filter(|m| m.version <= 13)
                .cloned()
                .collect(),
        );
        old.run(&pool).await?;
        sqlx::query("INSERT INTO tracks (path, title, artist, album_artist, album, year, genre, length_s, size_bytes, mtime, added_at, display_title, origin) VALUES ('old.mp3', 'Song', 'Artist', '', '', 1998, '', 1, 1, 1, '2026-09-13', '', '')").execute(&pool).await?;
        pool.close().await;
        let storage = SqliteStorage::open(SqliteStorageOptions::new(&path)).await?;
        let row: (String, String, String) =
            sqlx::query_as("SELECT release_date, original_release_date, composer FROM tracks")
                .fetch_one(&storage.pool)
                .await?;
        assert_eq!(row, ("1998".into(), "".into(), "".into()));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT MAX(version) FROM _sqlx_migrations")
                .fetch_one(&storage.pool)
                .await?,
            crate::CURRENT_SCHEMA_VERSION
        );
        let backups = std::fs::read_dir(directory.path())?
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "bak"))
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        let backup = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(backups[0].path())
                    .read_only(true),
            )
            .await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT MAX(version) FROM _sqlx_migrations")
                .fetch_one(&backup)
                .await?,
            13
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT year FROM tracks")
                .fetch_one(&backup)
                .await?,
            1998
        );
        backup.close().await;
        Ok(())
    }

    #[test]
    fn embedded_migrations_are_ordered_and_cross_platform_stable() {
        for (index, (version, _, sql)) in MIGRATIONS.iter().enumerate() {
            assert_eq!(usize::try_from(*version).ok(), Some(index + 1));
            assert!(
                !sql.contains('\r'),
                "migration {version} has CRLF line endings"
            );
        }
        assert_eq!(
            usize::try_from(crate::CURRENT_SCHEMA_VERSION).ok(),
            Some(MIGRATIONS.len())
        );
    }

    async fn version_ten_fixture(
        path: &std::path::Path,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(path)
                    .create_if_missing(true),
            )
            .await?;
        let old = sqlx::migrate::Migrator::with_migrations(
            migrator()
                .iter()
                .filter(|migration| migration.version <= 10)
                .cloned()
                .collect(),
        );
        old.run(&pool).await?;
        sqlx::query("INSERT INTO users (id, username, password_hash, created_at) VALUES (1, 'operator', 'preserved-password-hash', '2026-09-05 12:00:00')").execute(&pool).await?;
        sqlx::query("INSERT INTO auth_sessions VALUES ('unexpired-legacy-bearer-secret', 1, '2026-09-05 12:00:00', '2100-01-01 00:00:00', '2026-09-05 12:00:00')").execute(&pool).await?;
        pool.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn session_migration_revokes_unexpired_legacy_tokens_and_preserves_backup_and_accounts()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::{SqliteStorage, SqliteStorageOptions, inspect_database};
        use music_application::auth::{AuthRepository, SessionLookup, SessionTouch, UnixSeconds};
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("app.db");
        version_ten_fixture(&path).await?;
        let before = inspect_database(&path).await?;
        assert!(before.is_compatible());
        assert!(before.pending_migrations.contains(&11));
        let storage = SqliteStorage::open(SqliteStorageOptions::new(&path)).await?;
        assert_eq!(
            storage.migration_outcome().schema_after.migration_version,
            Some(crate::schema::CURRENT_SCHEMA_VERSION)
        );
        let backup = storage
            .migration_outcome()
            .backup
            .as_ref()
            .ok_or("missing backup")?;
        let backup_bytes = std::fs::read(&backup.database_path)?;
        let legacy_token = "unexpired-legacy-bearer-secret";
        assert!(
            backup_bytes
                .windows(legacy_token.len())
                .any(|window| window == legacy_token.as_bytes())
        );
        assert_eq!(
            storage
                .lookup_session(
                    legacy_token,
                    UnixSeconds::new(1_800_000_000),
                    SessionTouch::PreserveLastSeen,
                    std::time::Duration::from_secs(60)
                )
                .await?,
            SessionLookup::Missing
        );
        let user = storage
            .find_user_by_username("operator")
            .await?
            .ok_or("user missing")?;
        assert_eq!(user.password_hash.as_str(), "preserved-password-hash");
        storage
            .create_session(
                1,
                "fresh-bearer-cookie",
                UnixSeconds::new(1_800_000_000),
                UnixSeconds::new(1_900_000_000),
            )
            .await?;
        assert!(matches!(
            storage
                .lookup_session(
                    "fresh-bearer-cookie",
                    UnixSeconds::new(1_800_000_001),
                    SessionTouch::PreserveLastSeen,
                    std::time::Duration::from_secs(60)
                )
                .await?,
            SessionLookup::Authenticated { .. }
        ));
        storage.close().await;
        drop(storage);
        let reopened = SqliteStorage::open(SqliteStorageOptions::new(&path)).await?;
        assert!(!reopened.migration_outcome().migration_applied);
        assert_eq!(
            reopened
                .list_sessions(1, "fresh-bearer-cookie")
                .await?
                .len(),
            1
        );
        reopened.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn song_evidence_cutover_clears_only_generated_state_once()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("app.db");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true)
                    .foreign_keys(true),
            )
            .await?;
        let old = sqlx::migrate::Migrator::with_migrations(
            migrator()
                .iter()
                .filter(|migration| migration.version <= 14)
                .cloned()
                .collect(),
        );
        old.run(&pool).await?;
        sqlx::raw_sql("INSERT INTO tracks (id,path,title,artist,album_artist,album,genre,length_s,display_title,origin,size_bytes,mtime,added_at) VALUES (1,'kept.flac','Kept','Artist','','Album','',120,'Kept','',10,20,CURRENT_TIMESTAMP);
            INSERT INTO track_user_tags VALUES (1,'accepted-calm',CURRENT_TIMESTAMP);
            INSERT INTO playlists (id,name,automatic_rule_json,created_at,updated_at) VALUES (1,'Kept playlist','',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP);
            INSERT INTO playlist_items (playlist_id,track_id,position,added_at) VALUES (1,1,0,CURRENT_TIMESTAMP);
            INSERT INTO track_contexts VALUES (1,'local-context/v2','old','job','full','high','{}','[]','[]','{}','{}',CURRENT_TIMESTAMP);
            INSERT INTO track_analyses VALUES (1,'model-context-tagger/v7','old','job',0.2,0.3,0.4,'[\"calm\"]','[]','{}','high',CURRENT_TIMESTAMP);
            INSERT INTO track_analysis_tag_reviews VALUES (1,'model-context-tagger/v7','calm','old','accepted',CURRENT_TIMESTAMP);
            INSERT INTO track_analysis_failures VALUES (1,'local-context/v2','old','job','old error',CURRENT_TIMESTAMP);
            INSERT INTO background_jobs (id,kind,status,parameters_json,result_json,progress_current,progress_phase,progress_message,attempts,created_at,updated_at)
            VALUES ('job','assistant.library-context-analysis','running','{}','{\"old_checkpoint\":true}',1,'Old','Old',1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP);")
            .execute(&pool).await?;
        sqlx::query("INSERT INTO background_jobs (id,kind,status,lane,parameters_json,result_json,progress_current,progress_phase,progress_message,attempts,created_at,updated_at) VALUES ('paid','assistant.model-music-tagging','running','provider','{}','{\"provider_attempt\":\"uncertain\"}',1,'Submitted','Submitted',1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)").execute(&pool).await?;
        let rule = serde_json::json!({"schema":"automatic-playlist/v1","tag_sources":"manual_and_local","include_tags":["accepted-calm"],"maximum_tracks":25});
        sqlx::query(
            "UPDATE playlists SET automatic_rule_json = ?, automatic_source_signature = 'old'",
        )
        .bind(rule.to_string())
        .execute(&pool)
        .await?;
        pool.close().await;
        let before = crate::inspect_database(&path).await?;
        assert!(before.is_compatible(), "{before:?}");
        let storage = crate::SqliteStorage::open(crate::SqliteStorageOptions::new(&path)).await?;
        assert!(storage.migration_outcome().backup.is_some());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM track_contexts")
                .fetch_one(&storage.pool)
                .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM track_analyses")
                .fetch_one(&storage.pool)
                .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM track_analysis_tag_reviews")
                .fetch_one(&storage.pool)
                .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM track_analysis_failures")
                .fetch_one(&storage.pool)
                .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT tag FROM track_user_tags")
                .fetch_one(&storage.pool)
                .await?,
            "accepted-calm"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM playlist_items")
                .fetch_one(&storage.pool)
                .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM background_jobs WHERE id='job'")
                .fetch_one(&storage.pool)
                .await?,
            "cancelled"
        );
        assert_eq!(
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT result_json FROM background_jobs WHERE id='job'"
            )
            .fetch_one(&storage.pool)
            .await?,
            None
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT result_json FROM background_jobs WHERE id='paid'"
            )
            .fetch_one(&storage.pool)
            .await?,
            "{\"provider_attempt\":\"uncertain\"}"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM background_jobs WHERE id='paid'")
                .fetch_one(&storage.pool)
                .await?,
            "cancelled"
        );
        let actual_rule: String =
            sqlx::query_scalar("SELECT automatic_rule_json FROM playlists WHERE id=1")
                .fetch_one(&storage.pool)
                .await?;
        let parsed_rule: music_application::playlists::AutomaticPlaylistRule =
            serde_json::from_str(&actual_rule)?;
        assert_eq!(parsed_rule.normalized()?.maximum_tracks, 25);
        let mut expected_rule = rule;
        expected_rule["tag_sources"] = serde_json::json!("manual");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&actual_rule)?,
            expected_rule
        );
        assert!(
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT automatic_source_signature FROM playlists WHERE id=1"
            )
            .fetch_one(&storage.pool)
            .await?
            .is_none()
        );
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('track_analyses')")
                .fetch_all(&storage.pool)
                .await?;
        for retired in ["energy", "brightness", "tension", "confidence"] {
            assert!(!columns.iter().any(|name| name == retired));
        }
        assert!(columns.iter().any(|name| name == "decisions_json"));
        sqlx::query("INSERT INTO track_analyses (track_id,analyzer_id,source_signature,job_id,moods_json,evidence_json,metrics_json,decisions_json,updated_at) VALUES (1,'model-context-tagger/v8','new','new-job','[]','[\"No useful support\"]','{}','[]',CURRENT_TIMESTAMP)")
            .execute(&storage.pool).await?;
        sqlx::query("INSERT INTO track_contexts VALUES (1,'local-context/v3','new','new-job','full','{}','[]','[]','{}','{}',CURRENT_TIMESTAMP)").execute(&storage.pool).await?;
        storage.close().await;
        drop(storage);
        let reopened = crate::SqliteStorage::open(crate::SqliteStorageOptions::new(&path)).await?;
        assert!(!reopened.migration_outcome().migration_applied);
        assert!(reopened.migration_outcome().backup.is_none());
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT source_signature FROM track_contexts")
                .fetch_one(&reopened.pool)
                .await?,
            "new"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT source_signature FROM track_analyses")
                .fetch_one(&reopened.pool)
                .await?,
            "new"
        );
        reopened.close().await;
        Ok(())
    }
}
