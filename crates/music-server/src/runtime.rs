use std::fs;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use music_application::cleanup::{CleanupRepository, CleanupService};
use music_application::library::{
    LibraryCatalogSink, LibraryCoordinatorHandle, LibraryMutationRepository, LibraryRepository,
    LibraryService, ReconciliationStatus, SpawnedLibraryCoordinator, start_library_coordinator,
};
use music_application::playback::{
    CatalogSnapshot, PlaybackActorConfig, PlaybackActorHandle, SpawnedPlaybackActor,
    SystemPlaybackClock, SystemQueueRandom, start_playback_actor,
};
use music_media::{
    FfmpegTools, FilesystemLibraryDiscovery, FilesystemLibraryMutations, LibraryRoot,
    MetadataAdapter,
};
use music_storage::{
    LegacyDeviceImportOutcome, LegacyDeviceImportStatus, SqliteStorage, SqliteStorageOptions,
    StorageError,
};
use tokio::net::TcpListener;
use tokio::time::{Instant, MissedTickBehavior};
use tracing_subscriber::EnvFilter;

use crate::auth::RuntimeAuth;
use crate::config::AppConfig;
use crate::devices::RuntimeDevices;
use crate::error::RuntimeError;
use crate::health::{ComponentStatus, HealthRegistry, ReadinessSnapshot};
use crate::http::build_router;
use crate::library::RuntimeLibrary;
use crate::supervisor::{CriticalTaskError, TaskSupervisor};

const DATABASE_HEALTH_INTERVAL: Duration = Duration::from_secs(30);
const DATABASE_HEALTH_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_SEED_DEPTH: usize = 32;
const MAX_SEED_ENTRIES: usize = 10_000;

#[derive(Debug)]
pub struct AppRuntime {
    config: Arc<AppConfig>,
    health: HealthRegistry,
    supervisor: TaskSupervisor,
    storage: Arc<SqliteStorage>,
    playback: PlaybackActorHandle,
    auth: Arc<RuntimeAuth>,
    devices: Arc<RuntimeDevices>,
    library: LibraryCoordinatorHandle,
    library_service: Arc<LibraryService>,
    cleanup_service: Arc<CleanupService>,
    library_root: LibraryRoot,
    library_metadata: MetadataAdapter,
}

impl AppRuntime {
    pub async fn start(config: AppConfig) -> Result<Self, RuntimeError> {
        let config = Arc::new(config);
        let health = HealthRegistry::new();
        health.set_component("configuration", true, ComponentStatus::Ready);
        health.set_component("filesystem", true, ComponentStatus::Starting);
        health.set_component("instance_lock", true, ComponentStatus::Starting);
        health.set_component("database", true, ComponentStatus::Starting);
        health.set_component("database_schema", true, ComponentStatus::Starting);
        health.set_component("playback", true, ComponentStatus::Starting);
        health.set_component("authentication", true, ComponentStatus::Starting);
        health.set_component("remembered_devices", false, ComponentStatus::Starting);
        health.set_component("library_coordinator", true, ComponentStatus::Starting);
        health.set_component("library", false, ComponentStatus::Starting);
        health.set_component("runtime", true, ComponentStatus::Starting);

        initialize_directories(&config, &health)?;
        let storage =
            match SqliteStorage::open(SqliteStorageOptions::new(&config.database_path)).await {
                Ok(storage) => Arc::new(storage),
                Err(error) => {
                    health.set_component("instance_lock", true, ComponentStatus::Failed);
                    health.set_component("database", true, ComponentStatus::Failed);
                    health.set_component("database_schema", true, ComponentStatus::Failed);
                    return Err(error.into());
                }
            };
        if let Err(error) = storage.healthcheck().await {
            health.set_component("database", true, ComponentStatus::Failed);
            return Err(error.into());
        }
        health.set_component("instance_lock", true, ComponentStatus::Ready);
        health.set_component("database", true, ComponentStatus::Ready);
        health.set_component("database_schema", true, ComponentStatus::Ready);

        let migration = storage.migration_outcome();
        if let Some(backup) = &migration.backup {
            tracing::info!(
                backup = %backup.database_path.display(),
                manifest = %backup.manifest_path.display(),
                bytes = backup.bytes,
                sha256 = %backup.sha256,
                "created and verified the pre-migration database backup"
            );
        }
        if migration.migration_applied {
            tracing::info!(
                schema_before = %migration.schema_before.compatibility,
                schema_version = migration.schema_after.current_schema_version,
                "applied the Rust database baseline"
            );
        }

        initialize_legacy_devices(&storage, &config.devices_file, &health).await?;
        let auth = Arc::new(RuntimeAuth::new(Arc::clone(&storage), &config)?);
        let devices = Arc::new(RuntimeDevices::new(Arc::clone(&storage)));
        health.set_component("authentication", true, ComponentStatus::Ready);

        let supervisor = TaskSupervisor::new(health.clone());
        let playback = match start_playback_actor(
            Arc::clone(&storage),
            SystemPlaybackClock::try_new()?,
            SystemQueueRandom,
            PlaybackActorConfig::default(),
            CatalogSnapshot::default(),
        )
        .await
        {
            Ok(spawned) => supervise_playback(&supervisor, spawned)?,
            Err(error) => {
                health.set_component("playback", true, ComponentStatus::Failed);
                storage.close().await;
                return Err(error.into());
            }
        };
        health.set_component("playback", true, ComponentStatus::Ready);
        let library_root = LibraryRoot::open(&config.music_dir)?;
        let library_metadata = MetadataAdapter::with_ffmpeg(FfmpegTools::new("ffmpeg", "ffprobe"));
        let discovery = Arc::new(FilesystemLibraryDiscovery::new(
            library_root.clone(),
            library_metadata.clone(),
        ));
        let mutation_repository: Arc<dyn LibraryMutationRepository> = storage.clone();
        let read_repository: Arc<dyn LibraryRepository> = storage.clone();
        let cleanup_repository: Arc<dyn CleanupRepository> = storage.clone();
        let catalog_sink: Arc<dyn LibraryCatalogSink> = Arc::new(playback.clone());
        let effects = Arc::new(FilesystemLibraryMutations::new(
            library_root.clone(),
            library_metadata.clone(),
        ));
        let spawned_library =
            start_library_coordinator(mutation_repository, discovery, catalog_sink, effects)
                .await?;
        let library = supervise_library(&supervisor, spawned_library, health.clone())?;
        let library_service = Arc::new(LibraryService::new(read_repository));
        let cleanup_service = Arc::new(CleanupService::new(cleanup_repository));
        health.set_component("library_coordinator", true, ComponentStatus::Ready);
        apply_library_health(&health, library.status().status);
        library.request_reconciliation()?;
        start_database_monitor(&supervisor, Arc::clone(&storage))?;
        health.set_component("runtime", true, ComponentStatus::Ready);

        tracing::info!(
            music_dir = %config.music_dir.display(),
            sfx_library_dir = %config.sfx_library_dir.display(),
            modes_dir = %config.modes_dir.display(),
            "Rust runtime initialized"
        );
        Ok(Self {
            config,
            health,
            supervisor,
            storage,
            playback,
            auth,
            devices,
            library,
            library_service,
            cleanup_service,
            library_root,
            library_metadata,
        })
    }

    #[must_use]
    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    #[must_use]
    pub fn readiness(&self) -> ReadinessSnapshot {
        self.health.snapshot()
    }

    #[must_use]
    pub fn library_status(&self) -> music_application::library::LibraryStatus {
        self.library.status()
    }

    pub fn router(&self) -> Result<Router, RuntimeError> {
        build_router(
            &self.config,
            self.health.clone(),
            self.playback.clone(),
            Arc::clone(&self.auth),
            Arc::clone(&self.devices),
            Arc::new(RuntimeLibrary {
                service: Arc::clone(&self.library_service),
                cleanup: Arc::clone(&self.cleanup_service),
                coordinator: self.library.clone(),
                root: self.library_root.clone(),
                metadata: self.library_metadata.clone(),
                max_upload_files: self.config.max_upload_files,
                max_upload_file_bytes: self.config.max_upload_file_bytes,
            }),
        )
    }

    pub async fn run<F>(self, listener: TcpListener, shutdown_signal: F) -> Result<(), RuntimeError>
    where
        F: Future<Output = Result<(), RuntimeError>> + Send,
    {
        let router = match self.router() {
            Ok(router) => router,
            Err(error) => {
                self.health
                    .set_component("http", true, ComponentStatus::Failed);
                let _ = self.shutdown().await;
                return Err(error);
            }
        };
        self.health
            .set_component("http", true, ComponentStatus::Ready);
        let cancellation = self.supervisor.cancellation_token();
        let graceful_cancellation = cancellation.clone();
        let server = async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(graceful_cancellation.cancelled_owned())
            .await
        };
        tokio::pin!(server);
        tokio::pin!(shutdown_signal);

        let primary_result = tokio::select! {
            server_result = &mut server => {
                cancellation.cancel();
                server_result.map_err(|source| RuntimeError::io("serve HTTP requests", source))
            }
            signal_result = &mut shutdown_signal => {
                cancellation.cancel();
                let server_result = server.await
                    .map_err(|source| RuntimeError::io("complete graceful HTTP shutdown", source));
                signal_result.and(server_result)
            }
        };
        let shutdown_result = self.shutdown().await;

        primary_result?;
        if let Some(failure) = self.supervisor.failure() {
            return Err(RuntimeError::CriticalTaskFailed { task: failure.task });
        }
        shutdown_result
    }

    pub async fn shutdown(&self) -> Result<(), RuntimeError> {
        self.health
            .set_component("http", true, ComponentStatus::Starting);
        let supervisor_result = self.supervisor.shutdown(SHUTDOWN_TIMEOUT).await;
        self.storage.close().await;
        supervisor_result
    }
}

async fn initialize_legacy_devices(
    storage: &SqliteStorage,
    path: &Path,
    health: &HealthRegistry,
) -> Result<(), RuntimeError> {
    let outcome = match storage.import_legacy_devices_once(path).await {
        Ok(outcome) => outcome,
        Err(error @ StorageError::Io { .. }) => {
            health.set_component("remembered_devices", false, ComponentStatus::Degraded);
            tracing::warn!(error = %error, "legacy device registry could not be inspected");
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    let status = match &outcome {
        LegacyDeviceImportOutcome::Applied(record)
        | LegacyDeviceImportOutcome::AlreadyRecorded(record) => Some(record.status),
        LegacyDeviceImportOutcome::TargetNotEmpty => None,
    };
    if matches!(
        status,
        Some(LegacyDeviceImportStatus::Corrupt | LegacyDeviceImportStatus::Unsupported)
    ) {
        health.set_component("remembered_devices", false, ComponentStatus::Degraded);
        tracing::warn!(?outcome, "legacy device registry was not importable");
    } else {
        health.set_component("remembered_devices", false, ComponentStatus::Ready);
        tracing::info!(?outcome, "remembered-device storage initialized");
    }
    Ok(())
}

fn supervise_playback(
    supervisor: &TaskSupervisor,
    spawned: SpawnedPlaybackActor,
) -> Result<PlaybackActorHandle, RuntimeError> {
    let handle = spawned.handle;
    let shutdown_handle = handle.clone();
    let cancellation = supervisor.cancellation_token();
    let mut task = spawned.task;
    supervisor.spawn_critical("playback-owner", "playback", async move {
        tokio::select! {
            result = &mut task => map_playback_exit(result),
            () = cancellation.cancelled() => {
                shutdown_handle.shutdown();
                map_playback_exit(task.await)
            }
        }
    })?;
    Ok(handle)
}

fn map_playback_exit(
    result: Result<
        Result<(), music_application::playback::PlaybackActorError>,
        tokio::task::JoinError,
    >,
) -> Result<(), CriticalTaskError> {
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(CriticalTaskError::new("playback_owner_failed")),
        Err(_) => Err(CriticalTaskError::new("playback_owner_panicked")),
    }
}

fn supervise_library(
    supervisor: &TaskSupervisor,
    spawned: SpawnedLibraryCoordinator,
    health: HealthRegistry,
) -> Result<LibraryCoordinatorHandle, RuntimeError> {
    let handle = spawned.handle;
    let shutdown_handle = handle.clone();
    let mut status = handle.subscribe_status();
    let cancellation = supervisor.cancellation_token();
    let mut task = spawned.task;
    supervisor.spawn_critical("library-owner", "library_coordinator", async move {
        loop {
            tokio::select! {
                result = &mut task => return map_library_exit(result),
                () = cancellation.cancelled() => {
                    shutdown_handle.shutdown();
                    return map_library_exit(task.await);
                }
                changed = status.changed() => {
                    if changed.is_err() {
                        return Err(CriticalTaskError::new("library_status_channel_closed"));
                    }
                    apply_library_health(&health, status.borrow().status);
                }
            }
        }
    })?;
    Ok(handle)
}

fn map_library_exit(
    result: Result<
        Result<(), music_application::library::LibraryCoordinatorError>,
        tokio::task::JoinError,
    >,
) -> Result<(), CriticalTaskError> {
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(CriticalTaskError::new("library_owner_failed")),
        Err(_) => Err(CriticalTaskError::new("library_owner_panicked")),
    }
}

fn apply_library_health(health: &HealthRegistry, status: ReconciliationStatus) {
    health.set_component(
        "library",
        false,
        if status == ReconciliationStatus::Failed {
            ComponentStatus::Degraded
        } else {
            ComponentStatus::Ready
        },
    );
}

pub fn initialize_tracing(config: &AppConfig) -> Result<(), RuntimeError> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::new(config.log_level.to_string()))
        .try_init()
        .map_err(|_| RuntimeError::TracingInitialization)
}

fn start_database_monitor(
    supervisor: &TaskSupervisor,
    storage: Arc<SqliteStorage>,
) -> Result<(), RuntimeError> {
    let cancellation = supervisor.cancellation_token();
    supervisor.spawn_critical("database-monitor", "database", async move {
        let first_tick = Instant::now() + DATABASE_HEALTH_INTERVAL;
        let mut interval = tokio::time::interval_at(first_tick, DATABASE_HEALTH_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                () = cancellation.cancelled() => return Ok(()),
                _ = interval.tick() => {
                    match tokio::time::timeout(DATABASE_HEALTH_TIMEOUT, storage.healthcheck()).await {
                        Ok(Ok(())) => {}
                        Ok(Err(_)) => {
                            return Err(CriticalTaskError::new("database_healthcheck_failed"));
                        }
                        Err(_) => {
                            return Err(CriticalTaskError::new("database_healthcheck_timed_out"));
                        }
                    }
                }
            }
        }
    })
}

fn initialize_directories(config: &AppConfig, health: &HealthRegistry) -> Result<(), RuntimeError> {
    create_directory(&config.music_dir, "create the music library directory")?;
    create_directory(
        &config.sfx_library_dir,
        "create the sound-effects library directory",
    )?;
    create_directory(&config.modes_dir, "create the mode directory")?;
    health.set_component("filesystem", true, ComponentStatus::Ready);

    match seed_modes_if_empty(config) {
        Ok(SeedOutcome::Ready) => {
            health.set_component("mode_seed", false, ComponentStatus::Ready);
        }
        Ok(SeedOutcome::Unavailable) => {
            health.set_component("mode_seed", false, ComponentStatus::Degraded);
            tracing::warn!("configured mode seed is unavailable; continuing without seeding");
        }
        Err(error) => {
            health.set_component("mode_seed", false, ComponentStatus::Degraded);
            tracing::warn!(error_kind = ?error.kind(), "mode seeding was incomplete");
        }
    }
    Ok(())
}

fn create_directory(path: &Path, operation: &'static str) -> Result<(), RuntimeError> {
    fs::create_dir_all(path).map_err(|source| RuntimeError::io(operation, source))
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum SeedOutcome {
    Ready,
    Unavailable,
}

fn seed_modes_if_empty(config: &AppConfig) -> io::Result<SeedOutcome> {
    let Some(seed) = config.modes_seed_dir.as_deref() else {
        return Ok(SeedOutcome::Ready);
    };
    if !seed.is_dir() {
        return Ok(SeedOutcome::Unavailable);
    }
    if fs::read_dir(&config.modes_dir)?
        .next()
        .transpose()?
        .is_some()
    {
        return Ok(SeedOutcome::Ready);
    }
    if fs::canonicalize(seed)? == fs::canonicalize(&config.modes_dir)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "mode seed and target are the same directory",
        ));
    }

    let mut remaining = MAX_SEED_ENTRIES;
    copy_seed_directory(seed, &config.modes_dir, 0, &mut remaining)?;
    Ok(SeedOutcome::Ready)
}

fn copy_seed_directory(
    source: &Path,
    target: &Path,
    depth: usize,
    remaining: &mut usize,
) -> io::Result<()> {
    if depth > MAX_SEED_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "mode seed exceeds the directory depth limit",
        ));
    }
    for entry in fs::read_dir(source)? {
        if *remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "mode seed exceeds the entry limit",
            ));
        }
        *remaining -= 1;
        let entry = entry?;
        let metadata = entry.file_type()?;
        let destination = target.join(entry.file_name());
        if metadata.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "mode seed contains a symbolic link",
            ));
        }
        if metadata.is_dir() {
            fs::create_dir(&destination)?;
            copy_seed_directory(&entry.path(), &destination, depth + 1, remaining)?;
        } else if metadata.is_file() {
            fs::copy(entry.path(), destination)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "mode seed contains an unsupported file type",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::error::Error;
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    use axum::body::{Body, to_bytes};
    use axum::http::header::{
        ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_RANGE, CONTENT_SECURITY_POLICY, CONTENT_TYPE,
        ETAG, IF_NONE_MATCH, RANGE, SET_COOKIE, X_CONTENT_TYPE_OPTIONS,
    };
    use axum::http::{Request, StatusCode};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use music_application::auth::UnixSeconds;
    use music_application::library::{LibraryFileMutation, ReconciliationStatus};
    use music_application::recovery::{
        RecoveryDomain, RecoveryJournalDraft, RecoveryJournalRepository, RecoveryState,
        RecoveryTransition,
    };
    use music_storage::StorageError;
    use serde_json::Value;
    use tempfile::tempdir;
    use tower::ServiceExt;

    use super::AppRuntime;
    use crate::config::AppConfig;
    use crate::error::RuntimeError;
    use crate::health::{ComponentStatus, ReadinessStatus};

    fn runtime_config(root: &Path) -> Result<AppConfig, RuntimeError> {
        AppConfig::from_values(&BTreeMap::from([
            (
                "DATABASE_URL".to_owned(),
                format!("sqlite:///{}", root.join("app.db").display()),
            ),
            (
                "MUSIC_DIR".to_owned(),
                root.join("music").display().to_string(),
            ),
            (
                "SFX_LIBRARY_DIR".to_owned(),
                root.join("sfx").display().to_string(),
            ),
            (
                "MODES_DIR".to_owned(),
                root.join("modes").display().to_string(),
            ),
            (
                "STATIC_DIR".to_owned(),
                root.join("missing-static").display().to_string(),
            ),
        ]))
        .map_err(Into::into)
    }

    fn reference_wav() -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../contracts/reference/v1/metadata.examples.json"
        ))?;
        let encoded = fixture["cases"]
            .as_array()
            .and_then(|cases| {
                cases
                    .iter()
                    .find(|case| case["extension"].as_str() == Some(".wav"))
            })
            .and_then(|case| case["source_base64"].as_str())
            .ok_or("WAV metadata fixture is missing")?;
        Ok(STANDARD.decode(encoded)?)
    }

    fn multipart_upload(files: &[(&str, &[u8])]) -> (String, Vec<u8>) {
        let boundary = "music-rust-upload-boundary";
        let mut body = Vec::new();
        for (name, bytes) in files {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"files\"; filename=\"{name}\"\r\n\
                     Content-Type: application/octet-stream\r\n\r\n"
                )
                .as_bytes(),
            );
            body.extend_from_slice(bytes);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        (format!("multipart/form-data; boundary={boundary}"), body)
    }

    #[tokio::test]
    async fn startup_owns_storage_and_starts_the_playback_owner() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;

        assert!(directory.path().join("music").is_dir());
        assert!(directory.path().join("sfx").is_dir());
        assert!(directory.path().join("modes").is_dir());
        assert_eq!(runtime.readiness().status, ReadinessStatus::Ready);
        assert_eq!(
            runtime.readiness().components.get("playback"),
            Some(&ComponentStatus::Ready)
        );
        let state_response = runtime
            .router()?
            .oneshot(Request::get("/api/sync/state?client_id=test-output").body(Body::empty())?)
            .await?;
        assert_eq!(state_response.status(), StatusCode::OK);
        let state_body = to_bytes(state_response.into_body(), 1024 * 1024).await?;
        let state_json: Value = serde_json::from_slice(&state_body)?;
        assert_eq!(state_json["revision"], 0);
        assert_eq!(
            state_json["active_output_device_ids"],
            serde_json::json!([])
        );
        assert_eq!(state_json["connected_devices"], serde_json::json!([]));

        let second = AppRuntime::start(runtime_config(directory.path())?).await;
        assert!(matches!(
            second,
            Err(RuntimeError::Storage(StorageError::LockUnavailable { .. }))
        ));

        runtime.shutdown().await?;
        drop(runtime);
        let reopened = AppRuntime::start(runtime_config(directory.path())?).await?;
        reopened.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn seeds_only_an_empty_mode_directory() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let seed = directory.path().join("seed");
        fs::create_dir_all(seed.join("starter"))?;
        fs::write(seed.join("starter/mode.yaml"), "name: Starter\n")?;
        let mut values = BTreeMap::from([
            (
                "DATABASE_URL".to_owned(),
                format!("sqlite:///{}", directory.path().join("app.db").display()),
            ),
            (
                "MUSIC_DIR".to_owned(),
                directory.path().join("music").display().to_string(),
            ),
            (
                "SFX_LIBRARY_DIR".to_owned(),
                directory.path().join("sfx").display().to_string(),
            ),
            (
                "MODES_DIR".to_owned(),
                directory.path().join("modes").display().to_string(),
            ),
            ("MODES_SEED_DIR".to_owned(), seed.display().to_string()),
        ]);
        let runtime = AppRuntime::start(AppConfig::from_values(&values)?).await?;
        assert!(directory.path().join("modes/starter/mode.yaml").is_file());
        runtime.shutdown().await?;
        drop(runtime);

        fs::write(directory.path().join("modes/operator.txt"), "keep")?;
        values.insert(
            "DATABASE_URL".to_owned(),
            format!("sqlite:///{}", directory.path().join("second.db").display()),
        );
        let runtime = AppRuntime::start(AppConfig::from_values(&values)?).await?;
        assert_eq!(
            fs::read_to_string(directory.path().join("modes/operator.txt"))?,
            "keep"
        );
        runtime.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn server_shutdown_releases_the_storage_owner() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;

        runtime
            .run(listener, async {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                Ok(())
            })
            .await?;

        let reopened = AppRuntime::start(runtime_config(directory.path())?).await?;
        reopened.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn startup_replays_an_applied_folder_move_before_publishing_the_catalog()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        fs::create_dir_all(directory.path().join("music/Old"))?;
        fs::write(
            directory.path().join("music/Old/track.mp3"),
            b"metadata fallback fixture",
        )?;
        let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if runtime.library_status().status == ReconciliationStatus::Current {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        let track_id = *runtime
            .library_service
            .catalog_track_ids()
            .await?
            .first()
            .ok_or("startup scan did not index the recovery fixture")?;
        let mutation = LibraryFileMutation::RenameFolder {
            source: music_domain::LibraryPath::parse("Old")?,
            destination: music_domain::LibraryPath::parse("Recovered")?,
        };
        let draft = RecoveryJournalDraft::new(
            RecoveryDomain::Library,
            mutation.operation()?,
            mutation.plan(),
        )?;
        let journal_id = draft.id.clone();
        RecoveryJournalRepository::create_recovery_journal(runtime.storage.as_ref(), draft).await?;
        let applying = RecoveryJournalRepository::transition_recovery_journal(
            runtime.storage.as_ref(),
            &journal_id,
            RecoveryState::Planned,
            RecoveryState::Applying,
            serde_json::json!({}),
        )
        .await?;
        assert!(matches!(applying, RecoveryTransition::Applied(_)));
        fs::rename(
            directory.path().join("music/Old"),
            directory.path().join("music/Recovered"),
        )?;
        runtime.shutdown().await?;
        drop(runtime);

        let recovered = AppRuntime::start(runtime_config(directory.path())?).await?;
        let track = recovered
            .library_service
            .track(track_id)
            .await?
            .ok_or("recovery changed or removed the track identity")?;
        assert_eq!(track.path.as_str(), "Recovered/track.mp3");
        assert!(
            RecoveryJournalRepository::unfinished_recovery_journals(
                recovered.storage.as_ref(),
                RecoveryDomain::Library,
            )
            .await?
            .is_empty()
        );
        recovered.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn metadata_routes_preserve_tag_db_and_bulk_partial_failure_contracts()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        fs::create_dir_all(directory.path().join("music/Metadata"))?;
        let wav = reference_wav()?;
        fs::write(directory.path().join("music/Metadata/first.wav"), &wav)?;
        fs::write(directory.path().join("music/Metadata/second.wav"), &wav)?;

        let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let status = runtime.library_status();
                if status.status == ReconciliationStatus::Current {
                    break;
                }
                assert_ne!(status.status, ReconciliationStatus::Failed);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        let indexed = runtime
            .library_service
            .tracks_by_ids(&runtime.library_service.catalog_track_ids().await?)
            .await?;
        let first_id = indexed
            .iter()
            .find(|track| track.path.as_str() == "Metadata/first.wav")
            .ok_or("first metadata fixture was not indexed")?
            .id;
        let second_id = indexed
            .iter()
            .find(|track| track.path.as_str() == "Metadata/second.wav")
            .ok_or("second metadata fixture was not indexed")?
            .id;

        let hash = music_storage::hash_password("correct horse battery staple")?;
        runtime
            .storage
            .create_user("operator", &hash, UnixSeconds::new(1_800_000_000))
            .await?;
        let router = runtime.router()?;
        let unauthorized = router
            .clone()
            .oneshot(
                Request::patch(format!("/api/library/tracks/{}/metadata", first_id.get()))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title":"Denied"}"#))?,
            )
            .await?;
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let login = router
            .clone()
            .oneshot(
                Request::post("/api/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"username":"operator","password":"correct horse battery staple"}"#,
                    ))?,
            )
            .await?;
        assert_eq!(login.status(), StatusCode::OK);
        let cookie = login
            .headers()
            .get(SET_COOKIE)
            .ok_or("login did not set a session cookie")?
            .to_str()?
            .split(';')
            .next()
            .ok_or("session cookie was empty")?
            .to_owned();

        let edited = router
            .clone()
            .oneshot(
                Request::patch(format!("/api/library/tracks/{}/metadata", first_id.get()))
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"title":"Rust title","origin":"Game OST","display_title":null}"#,
                    ))?,
            )
            .await?;
        assert_eq!(edited.status(), StatusCode::OK);
        let edited_json: Value =
            serde_json::from_slice(&to_bytes(edited.into_body(), 1024 * 1024).await?)?;
        assert_eq!(edited_json["id"], first_id.get());
        assert_eq!(edited_json["title"], "Rust title");
        assert_eq!(edited_json["origin"], "Game OST");
        assert_eq!(edited_json["display_title"], "");
        let file_metadata = music_media::MetadataAdapter::native_only()
            .read(&directory.path().join("music/Metadata/first.wav"))?;
        assert_eq!(file_metadata.title, "Rust title");

        let unchanged = router
            .clone()
            .oneshot(
                Request::patch(format!("/api/library/tracks/{}/metadata", first_id.get()))
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))?,
            )
            .await?;
        assert_eq!(unchanged.status(), StatusCode::OK);
        let invalid = router
            .clone()
            .oneshot(
                Request::patch(format!("/api/library/tracks/{}/metadata", first_id.get()))
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"bpm":10000}"#))?,
            )
            .await?;
        assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let empty_bulk = router
            .clone()
            .oneshot(
                Request::patch("/api/library/tracks/bulk-metadata")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        "{{\"track_ids\":[{}],\"updates\":{{}}}}",
                        first_id.get()
                    )))?,
            )
            .await?;
        assert_eq!(empty_bulk.status(), StatusCode::BAD_REQUEST);
        let empty_ids = router
            .clone()
            .oneshot(
                Request::patch("/api/library/tracks/bulk-metadata")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"track_ids":[],"updates":{"artist":"Nobody"}}"#,
                    ))?,
            )
            .await?;
        assert_eq!(empty_ids.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let no_matches = router
            .clone()
            .oneshot(
                Request::patch("/api/library/tracks/bulk-metadata")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"track_ids":[9999991,9999992],"updates":{"artist":"Nobody"}}"#,
                    ))?,
            )
            .await?;
        assert_eq!(no_matches.status(), StatusCode::NOT_FOUND);

        fs::remove_file(directory.path().join("music/Metadata/second.wav"))?;
        let missing_single = router
            .clone()
            .oneshot(
                Request::patch(format!("/api/library/tracks/{}/metadata", second_id.get()))
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"artist":"Missing"}"#))?,
            )
            .await?;
        assert_eq!(missing_single.status(), StatusCode::GONE);
        let bulk = router
            .clone()
            .oneshot(
                Request::patch("/api/library/tracks/bulk-metadata")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        "{{\"track_ids\":[{},{},999999],\"updates\":{{\"artist\":\"Rust artist\",\"origin\":\"Bulk origin\"}}}}",
                        first_id.get(),
                        second_id.get()
                    )))?,
            )
            .await?;
        assert_eq!(bulk.status(), StatusCode::OK);
        let bulk_json: Value =
            serde_json::from_slice(&to_bytes(bulk.into_body(), 1024 * 1024).await?)?;
        assert_eq!(bulk_json["updated"].as_array().map(Vec::len), Some(2));
        assert_eq!(
            bulk_json["skipped"].as_array().map(Vec::len),
            Some(1),
            "{bulk_json}"
        );
        assert_eq!(bulk_json["skipped"][0]["track_id"], second_id.get());
        assert!(
            bulk_json["skipped"][0]["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("missing"))
        );

        let first = runtime
            .library_service
            .track(first_id)
            .await?
            .ok_or("updated first track disappeared")?;
        let second = runtime
            .library_service
            .track(second_id)
            .await?
            .ok_or("updated second track disappeared")?;
        assert_eq!(first.metadata.artist, "Rust artist");
        assert_eq!(first.origin, "Bulk origin");
        assert_ne!(second.metadata.artist, "Rust artist");
        assert_eq!(second.origin, "Bulk origin");
        assert_eq!(
            music_media::MetadataAdapter::native_only()
                .read(&directory.path().join("music/Metadata/first.wav"))?
                .artist,
            "Rust artist"
        );
        assert!(
            RecoveryJournalRepository::unfinished_recovery_journals(
                runtime.storage.as_ref(),
                RecoveryDomain::Library,
            )
            .await?
            .is_empty()
        );

        runtime.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn upload_routes_stream_resolve_conflicts_and_commit_the_catalog()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let wav = reference_wav()?;
        let mut config = runtime_config(directory.path())?;
        config.max_upload_files = 2;
        config.max_upload_file_bytes = 2_200_000;
        let runtime = AppRuntime::start(config).await?;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let status = runtime.library_status();
                if status.status == ReconciliationStatus::Current {
                    break;
                }
                assert_ne!(status.status, ReconciliationStatus::Failed);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        let hash = music_storage::hash_password("correct horse battery staple")?;
        runtime
            .storage
            .create_user("operator", &hash, UnixSeconds::new(1_800_000_000))
            .await?;
        let router = runtime.router()?;
        let login = router
            .clone()
            .oneshot(
                Request::post("/api/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"username":"operator","password":"correct horse battery staple"}"#,
                    ))?,
            )
            .await?;
        let cookie = login
            .headers()
            .get(SET_COOKIE)
            .ok_or("login did not set a session cookie")?
            .to_str()?
            .split(';')
            .next()
            .ok_or("session cookie was empty")?
            .to_owned();

        let (content_type, body) = multipart_upload(&[("song.wav", &wav)]);
        let first = router
            .clone()
            .oneshot(
                Request::post("/api/library/upload?dest=Uploads")
                    .header("cookie", &cookie)
                    .header("content-type", &content_type)
                    .body(Body::from(body))?,
            )
            .await?;
        assert_eq!(first.status(), StatusCode::CREATED);
        let first_json: Value =
            serde_json::from_slice(&to_bytes(first.into_body(), 1024 * 1024).await?)?;
        assert_eq!(first_json["destination"], "Uploads");
        assert_eq!(first_json["saved"][0]["path"], "Uploads/song.wav");
        let first_id = first_json["saved"][0]["id"]
            .as_i64()
            .ok_or("uploaded track id is missing")?;

        let (content_type, body) = multipart_upload(&[("song.wav", &wav)]);
        let renamed = router
            .clone()
            .oneshot(
                Request::post("/api/library/upload?dest=Uploads")
                    .header("cookie", &cookie)
                    .header("content-type", &content_type)
                    .body(Body::from(body))?,
            )
            .await?;
        assert_eq!(renamed.status(), StatusCode::CREATED);
        let renamed_json: Value =
            serde_json::from_slice(&to_bytes(renamed.into_body(), 1024 * 1024).await?)?;
        assert_eq!(renamed_json["saved"][0]["path"], "Uploads/song-1.wav");

        let (content_type, body) = multipart_upload(&[("song.wav", &wav)]);
        let overwritten = router
            .clone()
            .oneshot(
                Request::post("/api/library/upload?dest=Uploads&conflict=overwrite")
                    .header("cookie", &cookie)
                    .header("content-type", &content_type)
                    .body(Body::from(body))?,
            )
            .await?;
        assert_eq!(overwritten.status(), StatusCode::CREATED);
        let overwritten_json: Value =
            serde_json::from_slice(&to_bytes(overwritten.into_body(), 1024 * 1024).await?)?;
        assert_eq!(overwritten_json["saved"][0]["id"], first_id);
        assert_eq!(overwritten_json["saved"][0]["path"], "Uploads/song.wav");

        let (content_type, body) = multipart_upload(&[("song.wav", &wav)]);
        let skipped = router
            .clone()
            .oneshot(
                Request::post("/api/library/upload?dest=Uploads&conflict=skip")
                    .header("cookie", &cookie)
                    .header("content-type", &content_type)
                    .body(Body::from(body))?,
            )
            .await?;
        assert_eq!(skipped.status(), StatusCode::CREATED);
        let skipped_json: Value =
            serde_json::from_slice(&to_bytes(skipped.into_body(), 1024 * 1024).await?)?;
        assert_eq!(skipped_json["saved"].as_array().map(Vec::len), Some(0));
        assert_eq!(skipped_json["skipped"][0], "song.wav");

        let large_unindexed = vec![0_u8; 2_100_000];
        let (content_type, body) = multipart_upload(&[("large-unindexed.bin", &large_unindexed)]);
        let large = router
            .clone()
            .oneshot(
                Request::post("/api/library/upload?dest=Uploads")
                    .header("cookie", &cookie)
                    .header("content-type", &content_type)
                    .body(Body::from(body))?,
            )
            .await?;
        assert_eq!(large.status(), StatusCode::CREATED);
        assert!(
            directory
                .path()
                .join("music/Uploads/large-unindexed.bin")
                .is_file()
        );

        let checked = router
            .clone()
            .oneshot(
                Request::post("/api/library/upload/check")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"items":[{"dest":"Uploads","name":"song.wav"},{"dest":"Uploads","name":"new.wav"}]}"#,
                    ))?,
            )
            .await?;
        assert_eq!(checked.status(), StatusCode::OK);
        let checked_json: Value =
            serde_json::from_slice(&to_bytes(checked.into_body(), 1024 * 1024).await?)?;
        assert_eq!(checked_json["collisions"].as_array().map(Vec::len), Some(1));
        assert_eq!(checked_json["collisions"][0]["name"], "song.wav");

        let (content_type, body) = multipart_upload(&[
            ("flood-1.wav", &wav),
            ("flood-2.wav", &wav),
            ("flood-3.wav", &wav),
        ]);
        let too_many = router
            .clone()
            .oneshot(
                Request::post("/api/library/upload?dest=Flood")
                    .header("cookie", &cookie)
                    .header("content-type", &content_type)
                    .body(Body::from(body))?,
            )
            .await?;
        assert_eq!(too_many.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(!directory.path().join("music/Flood/flood-1.wav").exists());

        let oversized = vec![0_u8; 2_200_001];
        let (content_type, body) = multipart_upload(&[("oversized.wav", &oversized)]);
        let too_large = router
            .clone()
            .oneshot(
                Request::post("/api/library/upload?dest=Oversized")
                    .header("cookie", &cookie)
                    .header("content-type", &content_type)
                    .body(Body::from(body))?,
            )
            .await?;
        assert_eq!(too_large.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(
            std::fs::read_dir(directory.path().join("music/Oversized"))?
                .all(|entry| entry.is_ok_and(|entry| !entry.path().is_file()))
        );

        assert_eq!(runtime.library_status().discovered_tracks, 2);
        assert!(
            RecoveryJournalRepository::unfinished_recovery_journals(
                runtime.storage.as_ref(),
                RecoveryDomain::Library,
            )
            .await?
            .is_empty()
        );
        runtime.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn startup_reconciles_the_durable_catalog_and_serves_compatible_read_routes()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        fs::create_dir_all(directory.path().join("music/Album"))?;
        fs::write(
            directory.path().join("music/Album/01 - First.mp3"),
            b"metadata fallback fixture",
        )?;
        fs::write(
            directory.path().join("music/Album/02 - Second.flac"),
            b"metadata fallback fixture",
        )?;
        fs::create_dir_all(directory.path().join("music/Campaign/Scenes/Empty"))?;
        let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let status = runtime.library_status();
                if status.status == ReconciliationStatus::Current {
                    break;
                }
                assert_ne!(status.status, ReconciliationStatus::Failed);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        let ids = runtime.library_service.catalog_track_ids().await?;
        assert_eq!(ids.len(), 2);
        let indexed_tracks = runtime.library_service.tracks_by_ids(&ids).await?;
        let first_id = indexed_tracks
            .iter()
            .find(|track| track.path.as_str() == "Album/01 - First.mp3")
            .ok_or("first indexed track was missing")?
            .id;
        let second_id = indexed_tracks
            .iter()
            .find(|track| track.path.as_str() == "Album/02 - Second.flac")
            .ok_or("second indexed track was missing")?
            .id;

        let hash = music_storage::hash_password("correct horse battery staple")?;
        runtime
            .storage
            .create_user("operator", &hash, UnixSeconds::new(1_800_000_000))
            .await?;
        let router = runtime.router()?;
        let unauthorized = router
            .clone()
            .oneshot(Request::get("/api/library/search").body(Body::empty())?)
            .await?;
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        let unauthorized_cleanup = router
            .clone()
            .oneshot(
                Request::post("/api/library/cleanup/analyze")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"scope":{"type":"all"}}"#))?,
            )
            .await?;
        assert_eq!(unauthorized_cleanup.status(), StatusCode::UNAUTHORIZED);

        let login = router
            .clone()
            .oneshot(
                Request::post("/api/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"username":"operator","password":"correct horse battery staple"}"#,
                    ))?,
            )
            .await?;
        assert_eq!(login.status(), StatusCode::OK);
        let cookie = login
            .headers()
            .get(SET_COOKIE)
            .ok_or("login did not set a session cookie")?
            .to_str()?
            .split(';')
            .next()
            .ok_or("session cookie was empty")?
            .to_owned();

        let cleanup = router
            .clone()
            .oneshot(
                Request::post("/api/library/cleanup/analyze")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(
                        r#"{"scope":{"type":"folder","path":"Album","recursive":false}}"#,
                    ))?,
            )
            .await?;
        assert_eq!(cleanup.status(), StatusCode::OK);
        let cleanup_json: Value =
            serde_json::from_slice(&to_bytes(cleanup.into_body(), 1024 * 1024).await?)?;
        assert_eq!(cleanup_json["scanned"], 2);
        assert_eq!(cleanup_json["plans"].as_array().map(Vec::len), Some(2));
        assert!(cleanup_json["plans"].as_array().is_some_and(|plans| {
            plans.iter().all(|plan| {
                plan["ops"].as_array().is_some_and(|operations| {
                    operations.iter().any(|operation| {
                        operation["kind"] == "rename" && operation["confidence"] == "high"
                    })
                })
            })
        }));

        let empty_track_scope = router
            .clone()
            .oneshot(
                Request::post("/api/library/cleanup/analyze")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(r#"{"scope":{"type":"tracks","track_ids":[]}}"#))?,
            )
            .await?;
        assert_eq!(empty_track_scope.status(), StatusCode::BAD_REQUEST);

        let search = router
            .clone()
            .oneshot(
                Request::get("/api/library/search?q=First")
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(search.status(), StatusCode::OK);
        let search_json: Value =
            serde_json::from_slice(&to_bytes(search.into_body(), 1024 * 1024).await?)?;
        assert_eq!(search_json["total"], 1);
        assert_eq!(search_json["tracks"][0]["title"], "01 - First");
        assert_eq!(search_json["tracks"][0]["path"], "Album/01 - First.mp3");

        let batch = router
            .clone()
            .oneshot(
                Request::get(format!(
                    "/api/library/tracks?ids={},{},999999",
                    ids[1].get(),
                    ids[1].get()
                ))
                .body(Body::empty())?,
            )
            .await?;
        assert_eq!(batch.status(), StatusCode::OK);
        let batch_json: Value =
            serde_json::from_slice(&to_bytes(batch.into_body(), 1024 * 1024).await?)?;
        assert_eq!(batch_json.as_array().map(Vec::len), Some(1));
        assert_eq!(batch_json[0]["id"], ids[1].get());

        let tree = router
            .clone()
            .oneshot(
                Request::get("/api/library/tree?path=Album")
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(tree.status(), StatusCode::OK);
        let tree_json: Value =
            serde_json::from_slice(&to_bytes(tree.into_body(), 1024 * 1024).await?)?;
        assert_eq!(tree_json["tracks"].as_array().map(Vec::len), Some(2));

        let folders = router
            .clone()
            .oneshot(
                Request::get("/api/library/folders")
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(folders.status(), StatusCode::OK);
        let folders_json: Value =
            serde_json::from_slice(&to_bytes(folders.into_body(), 1024 * 1024).await?)?;
        let folder_entries = folders_json["folders"]
            .as_array()
            .ok_or("folders response was not an array")?;
        let album = folder_entries
            .iter()
            .find(|folder| folder["path"] == "Album")
            .ok_or("album folder was missing")?;
        assert_eq!(album["track_count"], 2);
        assert_eq!(album["has_children"], false);
        let campaign = folder_entries
            .iter()
            .find(|folder| folder["path"] == "Campaign")
            .ok_or("campaign folder was missing")?;
        assert_eq!(campaign["track_count"], 0);
        assert_eq!(campaign["has_children"], true);
        assert!(
            folder_entries
                .iter()
                .any(|folder| folder["path"] == "Campaign/Scenes/Empty")
        );

        let stream = router
            .clone()
            .oneshot(
                Request::get(format!("/api/library/tracks/{}/stream", first_id.get()))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(stream.status(), StatusCode::OK);
        assert_eq!(
            stream.headers().get(CONTENT_TYPE),
            Some(&"audio/mpeg".parse()?)
        );
        assert_eq!(stream.headers().get(ACCEPT_RANGES), Some(&"bytes".parse()?));
        assert_eq!(
            stream.headers().get(CONTENT_DISPOSITION),
            Some(&"inline".parse()?)
        );
        let etag = stream
            .headers()
            .get(ETAG)
            .ok_or("stream response omitted its entity tag")?
            .clone();
        assert_eq!(
            to_bytes(stream.into_body(), 1024 * 1024).await?,
            &b"metadata fallback fixture"[..]
        );

        let conditional = router
            .clone()
            .oneshot(
                Request::get(format!("/api/library/tracks/{}/stream", first_id.get()))
                    .header(IF_NONE_MATCH, etag)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(conditional.status(), StatusCode::NOT_MODIFIED);
        assert!(to_bytes(conditional.into_body(), 1024).await?.is_empty());

        let range = router
            .clone()
            .oneshot(
                Request::get(format!("/api/library/tracks/{}/stream", first_id.get()))
                    .header(RANGE, "bytes=0-7")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(range.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            range.headers().get(CONTENT_RANGE),
            Some(&"bytes 0-7/25".parse()?)
        );
        assert_eq!(to_bytes(range.into_body(), 1024).await?, &b"metadata"[..]);

        let unsatisfiable = router
            .clone()
            .oneshot(
                Request::get(format!("/api/library/tracks/{}/stream", first_id.get()))
                    .header(RANGE, "bytes=9999-")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(unsatisfiable.status(), StatusCode::RANGE_NOT_SATISFIABLE);

        let multiple_ranges = router
            .clone()
            .oneshot(
                Request::get(format!("/api/library/tracks/{}/stream", first_id.get()))
                    .header(RANGE, "bytes=0-1,3-4")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(multiple_ranges.status(), StatusCode::RANGE_NOT_SATISFIABLE);

        let no_cover = router
            .clone()
            .oneshot(
                Request::get(format!("/api/library/tracks/{}/cover", first_id.get()))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(no_cover.status(), StatusCode::NOT_FOUND);

        fs::write(
            directory.path().join("music/Album/cover.png"),
            b"safe cover",
        )?;
        let cover = router
            .clone()
            .oneshot(
                Request::get(format!("/api/library/tracks/{}/cover", first_id.get()))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(cover.status(), StatusCode::OK);
        assert_eq!(
            cover.headers().get(CONTENT_TYPE),
            Some(&"image/png".parse()?)
        );
        assert_eq!(
            cover.headers().get(X_CONTENT_TYPE_OPTIONS),
            Some(&"nosniff".parse()?)
        );
        assert_eq!(
            cover.headers().get(CONTENT_SECURITY_POLICY),
            Some(&"default-src 'none'; sandbox".parse()?)
        );
        assert_eq!(to_bytes(cover.into_body(), 1024).await?, &b"safe cover"[..]);

        let unauthorized_create = router
            .clone()
            .oneshot(
                Request::post("/api/library/folders")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"path":"Scratch/Nested"}"#))?,
            )
            .await?;
        assert_eq!(unauthorized_create.status(), StatusCode::UNAUTHORIZED);

        let created = router
            .clone()
            .oneshot(
                Request::post("/api/library/folders")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"path":"Scratch/Nested"}"#))?,
            )
            .await?;
        assert_eq!(created.status(), StatusCode::CREATED);
        let created_json: Value =
            serde_json::from_slice(&to_bytes(created.into_body(), 1024 * 1024).await?)?;
        assert_eq!(created_json["path"], "Scratch/Nested");
        assert_eq!(created_json["track_count"], 0);
        assert!(directory.path().join("music/Scratch/Nested").is_dir());

        let renamed_empty = router
            .clone()
            .oneshot(
                Request::post("/api/library/folders/rename")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"src":"Scratch","dst":"Archive/Scratch"}"#))?,
            )
            .await?;
        assert_eq!(renamed_empty.status(), StatusCode::OK);
        let renamed_empty_json: Value =
            serde_json::from_slice(&to_bytes(renamed_empty.into_body(), 1024 * 1024).await?)?;
        assert_eq!(renamed_empty_json["path"], "Archive/Scratch");
        assert_eq!(renamed_empty_json["has_children"], true);
        assert!(
            directory
                .path()
                .join("music/Archive/Scratch/Nested")
                .is_dir()
        );

        let non_recursive = router
            .clone()
            .oneshot(
                Request::delete("/api/library/folders?path=Archive/Scratch")
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(non_recursive.status(), StatusCode::BAD_REQUEST);
        let deleted_empty = router
            .clone()
            .oneshot(
                Request::delete("/api/library/folders?path=Archive/Scratch&recursive=true")
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(deleted_empty.status(), StatusCode::OK);
        let deleted_empty_json: Value =
            serde_json::from_slice(&to_bytes(deleted_empty.into_body(), 1024 * 1024).await?)?;
        assert_eq!(deleted_empty_json["removed_tracks"], 0);

        let generation_before_rename = runtime.library_status().generation.get();
        let renamed_album = router
            .clone()
            .oneshot(
                Request::post("/api/library/folders/rename")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"src":"Album","dst":"Renamed/Album"}"#))?,
            )
            .await?;
        assert_eq!(renamed_album.status(), StatusCode::OK);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let status = runtime.library_status();
                if status.status == ReconciliationStatus::Current
                    && status.generation.get() >= generation_before_rename + 2
                {
                    break;
                }
                assert_ne!(status.status, ReconciliationStatus::Failed);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        let renamed_track = runtime
            .library_service
            .track(first_id)
            .await?
            .ok_or("renamed track lost its catalog identity")?;
        assert_eq!(renamed_track.id, first_id);
        assert_eq!(renamed_track.path.as_str(), "Renamed/Album/01 - First.mp3");
        assert!(
            directory
                .path()
                .join("music/Renamed/Album/01 - First.mp3")
                .is_file()
        );

        let bulk_moved = router
            .clone()
            .oneshot(
                Request::post("/api/library/tracks/bulk-move")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        "{{\"track_ids\":[{},{},999999],\"destination\":\"Bulk\"}}",
                        first_id.get(),
                        second_id.get()
                    )))?,
            )
            .await?;
        assert_eq!(bulk_moved.status(), StatusCode::OK);
        let bulk_moved_json: Value =
            serde_json::from_slice(&to_bytes(bulk_moved.into_body(), 1024 * 1024).await?)?;
        assert_eq!(bulk_moved_json["moved"].as_array().map(Vec::len), Some(2));
        assert_eq!(bulk_moved_json["skipped"].as_array().map(Vec::len), Some(1));
        assert!(
            directory
                .path()
                .join("music/Bulk/02 - Second.flac")
                .is_file()
        );

        let moved_track = router
            .clone()
            .oneshot(
                Request::post(format!("/api/library/tracks/{}/move", first_id.get()))
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"destination":"Moved","new_filename":"Final.mp3"}"#,
                    ))?,
            )
            .await?;
        assert_eq!(moved_track.status(), StatusCode::OK);
        let moved_track_json: Value =
            serde_json::from_slice(&to_bytes(moved_track.into_body(), 1024 * 1024).await?)?;
        assert_eq!(moved_track_json["id"], first_id.get());
        assert_eq!(moved_track_json["path"], "Moved/Final.mp3");
        assert!(directory.path().join("music/Moved/Final.mp3").is_file());

        let generation_before_rescan = runtime.library_status().generation.get();
        let rescan = router
            .clone()
            .oneshot(
                Request::post("/api/library/rescan")
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(rescan.status(), StatusCode::OK);
        let rescan_json: Value =
            serde_json::from_slice(&to_bytes(rescan.into_body(), 1024 * 1024).await?)?;
        assert_eq!(rescan_json["updated"], 0);
        assert_eq!(rescan_json["unchanged"], 2);
        assert_eq!(
            runtime.library_status().generation.get(),
            generation_before_rescan + 1
        );

        fs::remove_file(directory.path().join("music/Bulk/02 - Second.flac"))?;
        let missing_stream = router
            .clone()
            .oneshot(
                Request::get(format!("/api/library/tracks/{}/stream", second_id.get()))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(missing_stream.status(), StatusCode::GONE);

        let bulk_deleted = router
            .clone()
            .oneshot(
                Request::post("/api/library/tracks/bulk-delete")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        "{{\"track_ids\":[{},999999]}}",
                        second_id.get()
                    )))?,
            )
            .await?;
        assert_eq!(bulk_deleted.status(), StatusCode::OK);
        let bulk_deleted_json: Value =
            serde_json::from_slice(&to_bytes(bulk_deleted.into_body(), 1024 * 1024).await?)?;
        assert_eq!(
            bulk_deleted_json["deleted_ids"],
            serde_json::json!([second_id.get()])
        );
        assert_eq!(
            bulk_deleted_json["skipped"].as_array().map(Vec::len),
            Some(1)
        );
        assert!(runtime.library_service.track(second_id).await?.is_none());
        let deleted_track = router
            .clone()
            .oneshot(
                Request::delete(format!("/api/library/tracks/{}", first_id.get()))
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(deleted_track.status(), StatusCode::NO_CONTENT);
        let deleted_again = router
            .oneshot(
                Request::delete(format!("/api/library/tracks/{}", first_id.get()))
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(deleted_again.status(), StatusCode::NOT_FOUND);

        runtime.shutdown().await?;
        Ok(())
    }
}
