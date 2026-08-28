use std::fs;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use music_application::cleanup::{
    CleanupMutationRepository, CleanupNameLookup, CleanupRepository, CleanupService,
    CleanupVerificationRepository, CleanupVerificationService,
};
use music_application::library::{
    LibraryCatalogSink, LibraryCoordinatorHandle, LibraryRepository, LibraryService,
    ReconciliationStatus, SpawnedLibraryCoordinator, start_library_coordinator,
};
use music_application::modes::{
    ModeCatalogSink, ModeCoordinatorHandle, ModeLoadState, SpawnedModeCoordinator,
    start_mutable_mode_coordinator,
};
use music_application::playback::{
    CatalogSnapshot, PlaybackActorConfig, PlaybackActorHandle, SpawnedPlaybackActor,
    SystemPlaybackClock, SystemQueueRandom, start_playback_actor,
};
use music_application::playlists::{PlaylistRepository, PlaylistService};
use music_application::recovery::RecoveryJournalRepository;
use music_media::{
    FfmpegTools, FilesystemLibraryDiscovery, FilesystemLibraryMutations,
    FilesystemModeCatalogSource, FilesystemModeMutations, LibraryRoot, MetadataAdapter,
};
use music_storage::{
    LegacyDeviceImportOutcome, LegacyDeviceImportStatus, SqliteStorage, SqliteStorageOptions,
    StorageError,
};
use tokio::net::TcpListener;
use tokio::time::{Instant, MissedTickBehavior};
use tracing_subscriber::EnvFilter;

use crate::admin::{BackupService, MaintenanceGate, pending_restore_journal};
use crate::auth::RuntimeAuth;
use crate::cleanup::MusicBrainzNameLookup;
use crate::config::AppConfig;
use crate::devices::RuntimeDevices;
use crate::error::RuntimeError;
use crate::health::{ComponentStatus, HealthRegistry, ReadinessSnapshot};
use crate::http::{RuntimeServices, build_router};
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
    backup: Arc<BackupService>,
    devices: Arc<RuntimeDevices>,
    library: LibraryCoordinatorHandle,
    modes: ModeCoordinatorHandle,
    library_service: Arc<LibraryService>,
    cleanup_service: Arc<CleanupService>,
    cleanup_verification_service: Arc<CleanupVerificationService>,
    playlist_service: Arc<PlaylistService>,
    library_root: LibraryRoot,
    library_metadata: MetadataAdapter,
}

impl AppRuntime {
    pub async fn start(config: AppConfig) -> Result<Self, RuntimeError> {
        if let Some(journal) = pending_restore_journal(&config.database_path) {
            return Err(RuntimeError::PendingRestore { journal });
        }
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
        health.set_component("mode_coordinator", true, ComponentStatus::Starting);
        health.set_component("modes", false, ComponentStatus::Starting);
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
        let backup = Arc::new(BackupService::new(
            Arc::clone(&storage),
            Arc::clone(&config),
            MaintenanceGate::default(),
        ));
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
        let mode_source = Arc::new(FilesystemModeCatalogSource::open(&config.modes_dir)?);
        let mode_sink: Arc<dyn ModeCatalogSink> = Arc::new(playback.clone());
        let mode_journal: Arc<dyn RecoveryJournalRepository> = storage.clone();
        let mode_effects = Arc::new(FilesystemModeMutations::open(&config.modes_dir).map_err(
            |_| {
                music_application::modes::ModeCoordinatorError::MutationRecovery(
                    "mode mutation filesystem could not be initialized",
                )
            },
        )?);
        let spawned_modes =
            start_mutable_mode_coordinator(mode_source, mode_sink, mode_journal, mode_effects)
                .await?;
        let modes = supervise_modes(&supervisor, spawned_modes, health.clone())?;
        let initial_mode_status = modes.wait_until_initialized().await?;
        health.set_component("mode_coordinator", true, ComponentStatus::Ready);
        apply_mode_health(&health, initial_mode_status.state);
        let library_root = LibraryRoot::open(&config.music_dir)?;
        let library_metadata = MetadataAdapter::with_ffmpeg(FfmpegTools::new("ffmpeg", "ffprobe"));
        let discovery = Arc::new(FilesystemLibraryDiscovery::new(
            library_root.clone(),
            library_metadata.clone(),
        ));
        let mutation_repository: Arc<dyn CleanupMutationRepository> = storage.clone();
        let read_repository: Arc<dyn LibraryRepository> = storage.clone();
        let cleanup_repository: Arc<dyn CleanupRepository> = storage.clone();
        let cleanup_verification_repository: Arc<dyn CleanupVerificationRepository> =
            storage.clone();
        let cleanup_lookup: Arc<dyn CleanupNameLookup> =
            Arc::new(MusicBrainzNameLookup::new().map_err(|source| {
                RuntimeError::io(
                    "initialize the MusicBrainz HTTP client",
                    io::Error::other(source),
                )
            })?);
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
        let cleanup_verification_service = Arc::new(CleanupVerificationService::new(
            cleanup_verification_repository,
            cleanup_lookup,
        ));
        let playlist_repository: Arc<dyn PlaylistRepository> = storage.clone();
        let playlist_service = Arc::new(PlaylistService::new(playlist_repository));
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
            backup,
            devices,
            library,
            modes,
            library_service,
            cleanup_service,
            cleanup_verification_service,
            playlist_service,
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
            RuntimeServices {
                health: self.health.clone(),
                playback: self.playback.clone(),
                auth: Arc::clone(&self.auth),
                backup: Arc::clone(&self.backup),
                devices: Arc::clone(&self.devices),
                library: Arc::new(RuntimeLibrary {
                    service: Arc::clone(&self.library_service),
                    cleanup: Arc::clone(&self.cleanup_service),
                    cleanup_verification: Arc::clone(&self.cleanup_verification_service),
                    coordinator: self.library.clone(),
                    root: self.library_root.clone(),
                    metadata: self.library_metadata.clone(),
                    max_upload_files: self.config.max_upload_files,
                    max_upload_file_bytes: self.config.max_upload_file_bytes,
                }),
                modes: self.modes.clone(),
                playlists: Arc::clone(&self.playlist_service),
            },
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

fn supervise_modes(
    supervisor: &TaskSupervisor,
    spawned: SpawnedModeCoordinator,
    health: HealthRegistry,
) -> Result<ModeCoordinatorHandle, RuntimeError> {
    let handle = spawned.handle;
    let shutdown_handle = handle.clone();
    let mut status = handle.subscribe_status();
    let cancellation = supervisor.cancellation_token();
    let mut task = spawned.task;
    supervisor.spawn_critical("mode-owner", "mode_coordinator", async move {
        loop {
            tokio::select! {
                result = &mut task => return map_mode_exit(result),
                () = cancellation.cancelled() => {
                    shutdown_handle.shutdown();
                    return map_mode_exit(task.await);
                }
                changed = status.changed() => {
                    if changed.is_err() {
                        return Err(CriticalTaskError::new("mode_status_channel_closed"));
                    }
                    apply_mode_health(&health, status.borrow().state);
                }
            }
        }
    })?;
    Ok(handle)
}

fn map_mode_exit(
    result: Result<
        Result<(), music_application::modes::ModeCoordinatorError>,
        tokio::task::JoinError,
    >,
) -> Result<(), CriticalTaskError> {
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(CriticalTaskError::new("mode_owner_failed")),
        Err(_) => Err(CriticalTaskError::new("mode_owner_panicked")),
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

fn apply_mode_health(health: &HealthRegistry, status: ModeLoadState) {
    let health_status = match status {
        ModeLoadState::Starting => ComponentStatus::Starting,
        ModeLoadState::Current => ComponentStatus::Ready,
        ModeLoadState::Degraded | ModeLoadState::Failed => ComponentStatus::Degraded,
    };
    health.set_component("modes", false, health_status);
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
    use std::io::Cursor;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::header::{
        ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_RANGE, CONTENT_SECURITY_POLICY, CONTENT_TYPE,
        ETAG, IF_NONE_MATCH, RANGE, SET_COOKIE, X_CONTENT_TYPE_OPTIONS,
    };
    use axum::http::{Request, StatusCode};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use flate2::read::GzDecoder;
    use music_application::auth::UnixSeconds;
    use music_application::cleanup::{
        CleanupBatchAppend, CleanupFuture, CleanupNameLookup, CleanupNameScores, CleanupRepository,
        CleanupVerificationService,
    };
    use music_application::library::{LibraryFileMutation, ReconciliationStatus};
    use music_application::modes::{
        ModeDocument, ModeMutation, ModeMutationEffects, ModeMutationFailureKind,
    };
    use music_application::recovery::{
        RecoveryDomain, RecoveryJournalDraft, RecoveryJournalId, RecoveryJournalRepository,
        RecoveryOperation, RecoveryState, RecoveryTransition,
    };
    use music_media::FilesystemModeMutations;
    use music_storage::{SqliteStorage, SqliteStorageOptions, StorageError};
    use serde_json::Value;
    use tar::Archive;
    use tempfile::tempdir;
    use tokio::sync::Mutex;
    use tower::ServiceExt;

    use super::AppRuntime;
    use crate::config::AppConfig;
    use crate::error::RuntimeError;
    use crate::health::{ComponentStatus, ReadinessStatus};

    #[derive(Debug)]
    struct FakeCleanupLookup {
        scores: CleanupNameScores,
        calls: Mutex<Vec<String>>,
    }

    impl FakeCleanupLookup {
        fn new(scores: CleanupNameScores) -> Self {
            Self {
                scores,
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl CleanupNameLookup for FakeCleanupLookup {
        fn fetch_name_scores<'a>(&'a self, name: &'a str) -> CleanupFuture<'a, CleanupNameScores> {
            Box::pin(async move {
                self.calls.lock().await.push(name.to_owned());
                Ok(self.scores)
            })
        }
    }

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

    async fn operator_router(runtime: &AppRuntime) -> Result<(Router, String), Box<dyn Error>> {
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
        Ok((router, cookie))
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

    #[tokio::test]
    async fn authenticated_backup_streams_a_verified_database_and_modes_archive()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        fs::create_dir_all(directory.path().join("modes/table/presets"))?;
        fs::write(
            directory.path().join("modes/table/mode.yaml"),
            "name: Table\n",
        )?;
        fs::write(
            directory.path().join("modes/table/presets/calm.yaml"),
            "gain: -2\n",
        )?;
        let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
        let hash = music_storage::hash_password("correct horse battery staple")?;
        runtime
            .storage
            .create_user("operator", &hash, UnixSeconds::new(1_800_000_000))
            .await?;
        let router = runtime.router()?;

        let unauthorized = router
            .clone()
            .oneshot(Request::get("/api/admin/backup").body(Body::empty())?)
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

        let backup = router
            .oneshot(
                Request::get("/api/admin/backup")
                    .header("cookie", cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(backup.status(), StatusCode::OK);
        assert_eq!(backup.headers()[CONTENT_TYPE], "application/gzip");
        assert_eq!(backup.headers()["cache-control"], "no-store");
        let disposition = backup.headers()[CONTENT_DISPOSITION].to_str()?;
        assert!(disposition.contains("music-backup-"));
        assert!(disposition.ends_with(".tar.gz\""));
        let expected_bytes = backup.headers()["content-length"]
            .to_str()?
            .parse::<usize>()?;
        let body = to_bytes(backup.into_body(), 16 * 1_024 * 1_024).await?;
        assert_eq!(body.len(), expected_bytes);

        let mut archive = Archive::new(GzDecoder::new(Cursor::new(body)));
        let mut names = archive
            .entries()?
            .map(|entry| {
                entry.and_then(|entry| {
                    entry
                        .path()
                        .map(|path| path.to_string_lossy().replace('\\', "/"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        names.sort();
        assert!(names.iter().any(|name| name == "manifest.json"));
        assert!(names.iter().any(|name| name == "app.db"));
        assert!(names.iter().any(|name| name == "modes"));
        assert!(
            names
                .iter()
                .any(|name| name == "modes/table/presets/calm.yaml")
        );
        assert!(!fs::read_dir(directory.path())?.any(|entry| {
            entry
                .ok()
                .and_then(|entry| entry.file_name().into_string().ok())
                .is_some_and(|name| name.starts_with(".music-backup-"))
        }));

        runtime.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn startup_refuses_an_interrupted_restore_journal()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let journal = directory.path().join("app.db.restore-journal.json");
        fs::write(&journal, "{}")?;

        let result = AppRuntime::start(runtime_config(directory.path())?).await;

        assert!(matches!(
            result,
            Err(RuntimeError::PendingRestore { journal: path }) if path == journal
        ));
        Ok(())
    }

    #[tokio::test]
    async fn cleanup_apply_serializes_file_catalog_and_batch_history()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        fs::create_dir_all(directory.path().join("music/Cleanup/Disc_1"))?;
        fs::write(
            directory.path().join("music/Cleanup/Disc_1/03 - Gamma.wav"),
            reference_wav()?,
        )?;
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
        let track_id = runtime
            .library_service
            .catalog_track_ids()
            .await?
            .into_iter()
            .next()
            .ok_or("cleanup fixture was not indexed")?;
        let hash = music_storage::hash_password("correct horse battery staple")?;
        runtime
            .storage
            .create_user("operator", &hash, UnixSeconds::new(1_800_000_000))
            .await?;
        let router = runtime.router()?;

        let unauthorized = router
            .clone()
            .oneshot(
                Request::post("/api/library/cleanup/apply")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        "{{\"ops\":[{{\"track_id\":{},\"kind\":\"rename\",\"old\":\"03 - Gamma\",\"new\":\"Gamma\"}}]}}",
                        track_id.get()
                    )))?,
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

        let apply = router
            .clone()
            .oneshot(
                Request::post("/api/library/cleanup/apply")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(format!(
                        concat!(
                            "{{\"scope_label\":\"roundtrip\",\"ops\":[",
                            "{{\"track_id\":{},\"kind\":\"tag\",\"field\":\"title\",\"old\":\"Round Trip\",\"new\":\"Gamma\"}},",
                            "{{\"track_id\":{},\"kind\":\"tag\",\"field\":\"track_no\",\"old\":7,\"new\":3}},",
                            "{{\"track_id\":{},\"kind\":\"rename\",\"old\":\"03 - Gamma\",\"new\":\"Gamma\"}}]}}"
                        ),
                        track_id.get(),
                        track_id.get(),
                        track_id.get()
                    )))?,
            )
            .await?;
        assert_eq!(apply.status(), StatusCode::OK);
        let apply_json: Value =
            serde_json::from_slice(&to_bytes(apply.into_body(), 1024 * 1024).await?)?;
        assert_eq!(apply_json["applied"], 3);
        assert_eq!(apply_json["skipped"], serde_json::json!([]));
        let batch_id = apply_json["batch_id"]
            .as_i64()
            .ok_or("cleanup apply did not create a batch")?;
        assert!(
            directory
                .path()
                .join("music/Cleanup/Disc_1/Gamma.wav")
                .is_file()
        );
        let track = runtime
            .library_service
            .track(track_id)
            .await?
            .ok_or("cleanup track lost its catalog row")?;
        assert_eq!(track.path.as_str(), "Cleanup/Disc_1/Gamma.wav");
        assert_eq!(track.metadata.title, "Gamma");
        assert_eq!(track.metadata.track_no, Some(3));

        let detail = router
            .clone()
            .oneshot(
                Request::get(format!("/api/library/cleanup/batches/{batch_id}"))
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(detail.status(), StatusCode::OK);
        let detail_json: Value =
            serde_json::from_slice(&to_bytes(detail.into_body(), 1024 * 1024).await?)?;
        assert_eq!(detail_json["item_count"], 3);
        assert_eq!(detail_json["items"][0]["kind"], "tag");
        assert_eq!(detail_json["items"][0]["file_old"], "Round Trip");
        assert_eq!(detail_json["items"][1]["file_old"], 7);
        assert_eq!(detail_json["items"][2]["kind"], "rename");

        let append = router
            .clone()
            .oneshot(
                Request::post("/api/library/cleanup/apply")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(format!(
                        "{{\"batch_id\":{batch_id},\"ops\":[{{\"track_id\":{},\"kind\":\"rename\",\"old\":\"Gamma\",\"new\":\"Final\"}}]}}",
                        track_id.get()
                    )))?,
            )
            .await?;
        assert_eq!(append.status(), StatusCode::OK);
        let append_json: Value =
            serde_json::from_slice(&to_bytes(append.into_body(), 1024 * 1024).await?)?;
        assert_eq!(append_json["batch_id"], batch_id);
        assert_eq!(append_json["applied"], 1);
        assert!(
            directory
                .path()
                .join("music/Cleanup/Disc_1/Final.wav")
                .is_file()
        );

        let folders = router
            .clone()
            .oneshot(
                Request::post("/api/library/cleanup/apply")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(format!(
                        "{{\"batch_id\":{batch_id},\"ops\":[{{\"track_id\":0,\"kind\":\"folder_rename\",\"old\":\"Cleanup\",\"new\":\"Cleaned\",\"path\":\"Cleanup\"}},{{\"track_id\":0,\"kind\":\"folder_rename\",\"old\":\"Disc_1\",\"new\":\"Disc 1\",\"path\":\"Cleanup/Disc_1\"}}]}}"
                    )))?,
            )
            .await?;
        assert_eq!(folders.status(), StatusCode::OK);
        let folders_json: Value =
            serde_json::from_slice(&to_bytes(folders.into_body(), 1024 * 1024).await?)?;
        assert_eq!(folders_json["applied"], 2);
        assert_eq!(folders_json["skipped"], serde_json::json!([]));
        assert!(
            directory
                .path()
                .join("music/Cleaned/Disc 1/Final.wav")
                .is_file()
        );
        assert_eq!(
            runtime
                .library_service
                .track(track_id)
                .await?
                .ok_or("folder cleanup lost its catalog row")?
                .path
                .as_str(),
            "Cleaned/Disc 1/Final.wav"
        );

        let stale = router
            .clone()
            .oneshot(
                Request::post("/api/library/cleanup/apply")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(format!(
                        "{{\"ops\":[{{\"track_id\":{},\"kind\":\"rename\",\"old\":\"Gamma\",\"new\":\"Ignored\"}}]}}",
                        track_id.get()
                    )))?,
            )
            .await?;
        let stale_json: Value =
            serde_json::from_slice(&to_bytes(stale.into_body(), 1024 * 1024).await?)?;
        assert_eq!(stale_json["batch_id"], Value::Null);
        assert_eq!(stale_json["applied"], 0);
        assert_eq!(
            stale_json["skipped"][0]["reason"],
            "filename changed since analysis"
        );

        let missing_batch = router
            .clone()
            .oneshot(
                Request::post("/api/library/cleanup/apply")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(format!(
                        "{{\"batch_id\":999999,\"ops\":[{{\"track_id\":{},\"kind\":\"rename\",\"old\":\"Final\",\"new\":\"Ignored\"}}]}}",
                        track_id.get()
                    )))?,
            )
            .await?;
        assert_eq!(missing_batch.status(), StatusCode::NOT_FOUND);

        let reverted = router
            .clone()
            .oneshot(
                Request::post(format!("/api/library/cleanup/batches/{batch_id}/revert"))
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(reverted.status(), StatusCode::OK);
        let reverted_json: Value =
            serde_json::from_slice(&to_bytes(reverted.into_body(), 1024 * 1024).await?)?;
        assert_eq!(reverted_json["reverted"], 6);
        assert_eq!(reverted_json["skipped"], serde_json::json!([]));
        assert!(
            directory
                .path()
                .join("music/Cleanup/Disc_1/03 - Gamma.wav")
                .is_file()
        );
        assert!(!directory.path().join("music/Cleaned").exists());
        let restored = runtime
            .library_service
            .track(track_id)
            .await?
            .ok_or("reverted cleanup lost its catalog row")?;
        assert_eq!(restored.path.as_str(), "Cleanup/Disc_1/03 - Gamma.wav");
        assert_eq!(restored.metadata.title, "Round Trip");
        assert_eq!(restored.metadata.track_no, Some(7));
        let file_metadata = music_media::MetadataAdapter::native_only()
            .read(&directory.path().join("music/Cleanup/Disc_1/03 - Gamma.wav"))?;
        assert_eq!(file_metadata.title, "Round Trip");
        assert_eq!(file_metadata.track_no, Some(7));

        let reverted_detail = router
            .clone()
            .oneshot(
                Request::get(format!("/api/library/cleanup/batches/{batch_id}"))
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        let reverted_detail_json: Value =
            serde_json::from_slice(&to_bytes(reverted_detail.into_body(), 1024 * 1024).await?)?;
        assert!(!reverted_detail_json["reverted_at"].is_null());
        let second_revert = router
            .clone()
            .oneshot(
                Request::post(format!("/api/library/cleanup/batches/{batch_id}/revert"))
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(second_revert.status(), StatusCode::CONFLICT);

        let journal_apply = router
            .clone()
            .oneshot(
                Request::post("/api/library/cleanup/apply")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(format!(
                        "{{\"ops\":[{{\"track_id\":{},\"kind\":\"rename\",\"old\":\"03 - Gamma\",\"new\":\"Journal Restore\"}}]}}",
                        track_id.get()
                    )))?,
            )
            .await?;
        let journal_apply_json: Value =
            serde_json::from_slice(&to_bytes(journal_apply.into_body(), 1024 * 1024).await?)?;
        let journal_batch_id = journal_apply_json["batch_id"]
            .as_i64()
            .ok_or("uploaded-journal cleanup did not create a batch")?;
        let journal_detail = router
            .clone()
            .oneshot(
                Request::get(format!("/api/library/cleanup/batches/{journal_batch_id}"))
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        let journal_detail_json: Value =
            serde_json::from_slice(&to_bytes(journal_detail.into_body(), 1024 * 1024).await?)?;
        let uploaded_revert = router
            .clone()
            .oneshot(
                Request::post("/api/library/cleanup/revert")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(
                        serde_json::json!({"items": journal_detail_json["items"]}).to_string(),
                    ))?,
            )
            .await?;
        assert_eq!(uploaded_revert.status(), StatusCode::OK);
        let uploaded_revert_json: Value =
            serde_json::from_slice(&to_bytes(uploaded_revert.into_body(), 1024 * 1024).await?)?;
        assert_eq!(uploaded_revert_json["reverted"], 1);
        assert_eq!(uploaded_revert_json["skipped"], serde_json::json!([]));
        assert!(
            directory
                .path()
                .join("music/Cleanup/Disc_1/03 - Gamma.wav")
                .is_file()
        );

        let empty_uploaded_revert = router
            .oneshot(
                Request::post("/api/library/cleanup/revert")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(r#"{"items":[]}"#))?,
            )
            .await?;
        assert_eq!(
            empty_uploaded_revert.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert!(
            RecoveryJournalRepository::unfinished_recovery_journals(
                runtime.storage.as_ref(),
                RecoveryDomain::Cleanup,
            )
            .await?
            .is_empty()
        );

        runtime.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn startup_recovers_cleanup_disk_effect_into_catalog_and_history()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        fs::create_dir_all(directory.path().join("music/Recovery"))?;
        fs::write(
            directory.path().join("music/Recovery/01 - Song.wav"),
            reference_wav()?,
        )?;
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
        let track_id = runtime
            .library_service
            .catalog_track_ids()
            .await?
            .into_iter()
            .next()
            .ok_or("recovery fixture was not indexed")?;
        runtime.shutdown().await?;
        drop(runtime);

        let source = music_domain::LibraryPath::parse("Recovery/01 - Song.wav")?;
        let destination = music_domain::LibraryPath::parse("Recovery/Song.wav")?;
        let mutation = LibraryFileMutation::MoveTrack {
            track_id,
            source: source.clone(),
            destination: destination.clone(),
        };
        let append = CleanupBatchAppend::new(
            None,
            "recovered cleanup".to_owned(),
            serde_json::Map::from_iter([
                ("kind".to_owned(), serde_json::json!("rename")),
                ("track_id".to_owned(), serde_json::json!(track_id.get())),
                ("path_before".to_owned(), serde_json::json!(source.as_str())),
                (
                    "path_after".to_owned(),
                    serde_json::json!(destination.as_str()),
                ),
            ]),
        )?;
        let draft = RecoveryJournalDraft::new(
            RecoveryDomain::Cleanup,
            mutation.operation()?,
            append.journal_plan(&mutation)?,
        )?;
        let journal_id = draft.id.clone();
        let storage =
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?;
        RecoveryJournalRepository::create_recovery_journal(&storage, draft).await?;
        let transition = RecoveryJournalRepository::transition_recovery_journal(
            &storage,
            &journal_id,
            RecoveryState::Planned,
            RecoveryState::Applying,
            serde_json::json!({"simulated_crash": true}),
        )
        .await?;
        assert!(matches!(transition, RecoveryTransition::Applied(_)));
        storage.close().await;
        drop(storage);
        fs::rename(
            directory.path().join("music/Recovery/01 - Song.wav"),
            directory.path().join("music/Recovery/Song.wav"),
        )?;

        let recovered = AppRuntime::start(runtime_config(directory.path())?).await?;
        let track = recovered
            .library_service
            .track(track_id)
            .await?
            .ok_or("recovered cleanup lost its catalog row")?;
        assert_eq!(track.path, destination);
        let batches = CleanupRepository::cleanup_batches(recovered.storage.as_ref()).await?;
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].scope_label, "recovered cleanup");
        assert_eq!(batches[0].item_count, 1);
        let batch_id = batches[0].id;
        assert!(
            RecoveryJournalRepository::unfinished_recovery_journals(
                recovered.storage.as_ref(),
                RecoveryDomain::Cleanup,
            )
            .await?
            .is_empty()
        );
        recovered.shutdown().await?;
        drop(recovered);

        let draft = RecoveryJournalDraft::new(
            RecoveryDomain::Cleanup,
            RecoveryOperation::parse("revert_batch")?,
            serde_json::json!({"batch_id": batch_id}),
        )?;
        let journal_id = draft.id.clone();
        let storage =
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?;
        RecoveryJournalRepository::create_recovery_journal(&storage, draft).await?;
        let transition = RecoveryJournalRepository::transition_recovery_journal(
            &storage,
            &journal_id,
            RecoveryState::Planned,
            RecoveryState::Applying,
            serde_json::json!({"simulated_crash": true}),
        )
        .await?;
        assert!(matches!(transition, RecoveryTransition::Applied(_)));
        storage.close().await;
        drop(storage);

        let reverted = AppRuntime::start(runtime_config(directory.path())?).await?;
        let track = reverted
            .library_service
            .track(track_id)
            .await?
            .ok_or("recovered batch revert lost its catalog row")?;
        assert_eq!(track.path, source);
        assert!(
            directory
                .path()
                .join("music/Recovery/01 - Song.wav")
                .is_file()
        );
        let batch = CleanupRepository::cleanup_batch(reverted.storage.as_ref(), batch_id)
            .await?
            .ok_or("recovered batch revert lost its batch history")?;
        assert!(batch.reverted_at_unix_seconds.is_some());
        assert!(
            RecoveryJournalRepository::unfinished_recovery_journals(
                reverted.storage.as_ref(),
                RecoveryDomain::Cleanup,
            )
            .await?
            .is_empty()
        );
        reverted.shutdown().await?;
        Ok(())
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
    async fn diagnostics_requires_auth_and_projects_live_owner_state() -> Result<(), Box<dyn Error>>
    {
        let directory = tempdir()?;
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
        let hash = music_storage::hash_password("correct horse battery staple")?;
        runtime
            .storage
            .create_user("operator", &hash, UnixSeconds::new(1_800_000_000))
            .await?;
        let router = runtime.router()?;

        let unauthorized = router
            .clone()
            .oneshot(Request::get("/api/diagnostics").body(Body::empty())?)
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
        let cookie = login
            .headers()
            .get(SET_COOKIE)
            .ok_or("login did not set a session cookie")?
            .to_str()?
            .split(';')
            .next()
            .ok_or("session cookie was empty")?;
        let response = router
            .oneshot(
                Request::get("/api/diagnostics")
                    .header("cookie", cookie)
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await?)?;
        assert_eq!(body["track_count"], 0);
        assert!(body["last_scan_at"].is_number());
        assert!(body["modes"]["last_load_at"].is_number());
        assert_eq!(body["modes"]["loaded_ids"], serde_json::json!([]));
        assert_eq!(body["modes"]["errors"], serde_json::json!({}));
        assert_eq!(body["connected_device_count"], 0);
        assert_eq!(body["state_revision"], 0);
        runtime.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn mode_reads_active_selection_theme_and_reload_use_the_rust_catalog()
    -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        fs::create_dir_all(directory.path().join("modes/table/soundboards"))?;
        fs::create_dir_all(directory.path().join("modes/table/presets"))?;
        fs::write(
            directory.path().join("modes/table/manifest.yaml"),
            "id: table\nname: Table\ntheme: theme.css\npanels: [now-playing]\nplaylist_categories: [ambient]\ndefault_crossfade_ms: 1250\ndefault_soundboard: main\n",
        )?;
        fs::write(
            directory.path().join("modes/table/theme.css"),
            "[data-mode='table'] { color: teal; }\n",
        )?;
        fs::write(
            directory.path().join("modes/table/soundboards/main.yaml"),
            "id: main\nname: Main\ncategories: []\n",
        )?;
        fs::write(
            directory.path().join("modes/table/presets/calm.yaml"),
            "id: calm\nname: Calm\neffects:\n  - type: eq\n    low: -2\n",
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
        let hash = music_storage::hash_password("correct horse battery staple")?;
        runtime
            .storage
            .create_user("operator", &hash, UnixSeconds::new(1_800_000_000))
            .await?;
        let router = runtime.router()?;

        let unauthorized = router
            .clone()
            .oneshot(Request::get("/api/modes").body(Body::empty())?)
            .await?;
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        let guest_presets = router
            .clone()
            .oneshot(Request::get("/api/modes/table/presets").body(Body::empty())?)
            .await?;
        assert_eq!(guest_presets.status(), StatusCode::OK);
        let guest_presets: Value =
            serde_json::from_slice(&to_bytes(guest_presets.into_body(), 1024 * 1024).await?)?;
        assert_eq!(guest_presets[0]["id"], "calm");
        assert_eq!(guest_presets[0]["effects"][0]["low"], -2);

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

        let listed = router
            .clone()
            .oneshot(
                Request::get("/api/modes")
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        let listed: Value =
            serde_json::from_slice(&to_bytes(listed.into_body(), 1024 * 1024).await?)?;
        assert_eq!(listed[0]["id"], "table");
        assert_eq!(listed[0]["has_theme"], true);
        assert_eq!(listed[0]["default_crossfade_ms"], 1250);

        let detail = router
            .clone()
            .oneshot(
                Request::get("/api/modes/table")
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        let detail: Value =
            serde_json::from_slice(&to_bytes(detail.into_body(), 1024 * 1024).await?)?;
        assert_eq!(detail["soundboards"]["main"]["name"], "Main");
        assert_eq!(detail["presets"]["calm"]["effects"][0]["type"], "eq");

        let theme = router
            .clone()
            .oneshot(
                Request::get("/api/modes/table/theme.css")
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(theme.status(), StatusCode::OK);
        assert!(
            theme.headers()[CONTENT_TYPE]
                .to_str()?
                .starts_with("text/css")
        );
        assert!(
            String::from_utf8(to_bytes(theme.into_body(), 1024).await?.to_vec())?.contains("teal")
        );

        let selected = router
            .clone()
            .oneshot(
                Request::put("/api/modes/active")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"mode_id":"table"}"#))?,
            )
            .await?;
        assert_eq!(selected.status(), StatusCode::OK);
        let selected: Value = serde_json::from_slice(&to_bytes(selected.into_body(), 1024).await?)?;
        assert_eq!(selected, serde_json::json!({"mode_id":"table"}));
        let unknown = router
            .clone()
            .oneshot(
                Request::put("/api/modes/active")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"mode_id":"missing"}"#))?,
            )
            .await?;
        assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);

        fs::create_dir(directory.path().join("modes/cyber"))?;
        fs::write(
            directory.path().join("modes/cyber/manifest.yaml"),
            "id: cyber\nname: Cyber\n",
        )?;
        let reload = router
            .clone()
            .oneshot(
                Request::post("/api/modes/reload")
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(reload.status(), StatusCode::OK);
        let reload: Value =
            serde_json::from_slice(&to_bytes(reload.into_body(), 1024 * 1024).await?)?;
        assert_eq!(reload["errors"], serde_json::json!({}));
        assert!(reload["loaded"].as_array().is_some_and(|ids| {
            ids.iter().any(|id| id == "table") && ids.iter().any(|id| id == "cyber")
        }));

        fs::create_dir(directory.path().join("modes/broken"))?;
        fs::write(
            directory.path().join("modes/broken/manifest.yaml"),
            "id: wrong\nname: Broken\n",
        )?;
        let degraded = router
            .clone()
            .oneshot(
                Request::post("/api/modes/reload")
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        let degraded: Value =
            serde_json::from_slice(&to_bytes(degraded.into_body(), 1024 * 1024).await?)?;
        assert!(degraded["errors"].get("broken").is_some());
        assert!(runtime.modes.snapshot().is_some_and(|catalog| {
            catalog.modes.contains_key("table") && catalog.modes.contains_key("cyber")
        }));

        runtime.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn mode_authoring_crud_is_journaled_validated_and_immediately_visible()
    -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
        let (router, cookie) = operator_router(&runtime).await?;
        let initial_generation = runtime
            .modes
            .snapshot()
            .ok_or("mode catalog was not initialized")?
            .generation;

        let unauthorized = router
            .clone()
            .oneshot(
                Request::post("/api/modes")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"id":"table","name":"Table"}"#))?,
            )
            .await?;
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let created = router
            .clone()
            .oneshot(
                Request::post("/api/modes")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"id":"table","name":"Table"}"#))?,
            )
            .await?;
        assert_eq!(created.status(), StatusCode::CREATED);
        let created: Value =
            serde_json::from_slice(&to_bytes(created.into_body(), 1024 * 1024).await?)?;
        assert_eq!(created["id"], "table");
        assert!(directory.path().join("modes/table/manifest.yaml").is_file());
        let stale = runtime
            .modes
            .mutate(ModeMutation::DeleteMode {
                expected_generation: initial_generation,
                mode_id: "table".to_owned(),
            })
            .await
            .err()
            .ok_or("stale mode mutation unexpectedly succeeded")?;
        assert_eq!(stale.kind, ModeMutationFailureKind::Stale);
        assert!(directory.path().join("modes/table").is_dir());

        let renamed = router
            .clone()
            .oneshot(
                Request::patch("/api/modes/table")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Game Table"}"#))?,
            )
            .await?;
        assert_eq!(renamed.status(), StatusCode::OK);

        let soundboard = router
            .clone()
            .oneshot(
                Request::post("/api/modes/table/soundboards")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"id":"combat","name":"Combat"}"#))?,
            )
            .await?;
        assert_eq!(soundboard.status(), StatusCode::CREATED);

        let category = router
            .clone()
            .oneshot(
                Request::post("/api/modes/table/soundboards/combat/categories")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"id":"weapons","name":"Weapons"}"#))?,
            )
            .await?;
        assert_eq!(category.status(), StatusCode::CREATED);

        let item = router
            .clone()
            .oneshot(
                Request::post("/api/modes/table/soundboards/combat/categories/weapons/items")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"file":"weapons/sword.wav","name":"Sword","icon":"S"}"#,
                    ))?,
            )
            .await?;
        assert_eq!(item.status(), StatusCode::CREATED);

        let patched_item = router
            .clone()
            .oneshot(
                Request::patch("/api/modes/table/soundboards/combat/categories/weapons/items/0")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Longsword","icon":null}"#))?,
            )
            .await?;
        assert_eq!(patched_item.status(), StatusCode::OK);
        let patched_item: Value =
            serde_json::from_slice(&to_bytes(patched_item.into_body(), 1024 * 1024).await?)?;
        assert_eq!(
            patched_item["categories"][0]["items"][0]["name"],
            "Longsword"
        );
        assert!(patched_item["categories"][0]["items"][0]["icon"].is_null());

        let poisoned_item = router
            .clone()
            .oneshot(
                Request::patch("/api/modes/table/soundboards/combat/categories/weapons/items/0")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":null}"#))?,
            )
            .await?;
        assert_eq!(poisoned_item.status(), StatusCode::BAD_REQUEST);

        let interrupt = router
            .clone()
            .oneshot(
                Request::post("/api/modes/table/interrupts")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"Battle","playlist":"battle","duck_to":0.4}"#,
                    ))?,
            )
            .await?;
        assert_eq!(interrupt.status(), StatusCode::CREATED);

        let switched_interrupt = router
            .clone()
            .oneshot(
                Request::patch("/api/modes/table/interrupts/0")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"playlist":null,"soundboard_item":"combat:weapons:0"}"#,
                    ))?,
            )
            .await?;
        assert_eq!(switched_interrupt.status(), StatusCode::OK);

        let cue = router
            .clone()
            .oneshot(
                Request::post("/api/modes/table/cues")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"id":"ambush","name":"Ambush","playlist":"battle","sfx":[{"soundboard":"combat","item":"weapons:0","volume":0.8}]}"#,
                    ))?,
            )
            .await?;
        assert_eq!(cue.status(), StatusCode::CREATED);

        let updated_cue = router
            .clone()
            .oneshot(
                Request::put("/api/modes/table/cues/ambush")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"Ambush Again","start_ms":1250,"loops":[{"soundboard":"combat","item":"weapons:0","interval_s":30.0}]}"#,
                    ))?,
            )
            .await?;
        assert_eq!(updated_cue.status(), StatusCode::OK);

        let rejected_preset = router
            .clone()
            .oneshot(
                Request::post("/api/modes/table/presets")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"id":"unsafe","name":"Unsafe","effects":[{"type":"pitch_shift"}]}"#,
                    ))?,
            )
            .await?;
        assert_eq!(rejected_preset.status(), StatusCode::BAD_REQUEST);
        assert!(
            !directory
                .path()
                .join("modes/table/presets/unsafe.yaml")
                .exists()
        );

        let preset = router
            .clone()
            .oneshot(
                Request::post("/api/modes/table/presets")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"id":"cave","name":"Cave","effects":[{"type":"eq","low":-2}],"crossfade_ms":500}"#,
                    ))?,
            )
            .await?;
        assert_eq!(preset.status(), StatusCode::CREATED);

        let updated_preset = router
            .clone()
            .oneshot(
                Request::put("/api/modes/table/presets/cave")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"Deep Cave","effects":[{"type":"reverb","room_size":0.8}]}"#,
                    ))?,
            )
            .await?;
        assert_eq!(updated_preset.status(), StatusCode::OK);

        let detail = router
            .clone()
            .oneshot(
                Request::get("/api/modes/table")
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        let detail: Value =
            serde_json::from_slice(&to_bytes(detail.into_body(), 1024 * 1024).await?)?;
        assert_eq!(detail["name"], "Game Table");
        assert_eq!(detail["cues"]["ambush"]["start_ms"], 1250);
        assert_eq!(detail["presets"]["cave"]["effects"][0]["type"], "reverb");
        assert_eq!(
            detail["interrupts"][0]["soundboard_item"],
            "combat:weapons:0"
        );

        for (method, path) in [
            ("DELETE", "/api/modes/table/presets/cave"),
            ("DELETE", "/api/modes/table/cues/ambush"),
            ("DELETE", "/api/modes/table/interrupts/0"),
            (
                "DELETE",
                "/api/modes/table/soundboards/combat/categories/weapons/items/0",
            ),
            (
                "DELETE",
                "/api/modes/table/soundboards/combat/categories/weapons",
            ),
            ("DELETE", "/api/modes/table/soundboards/combat"),
        ] {
            let request = Request::builder()
                .method(method)
                .uri(path)
                .header("cookie", &cookie)
                .body(Body::empty())?;
            let response = router.clone().oneshot(request).await?;
            assert!(
                response.status().is_success(),
                "{method} {path} returned {}",
                response.status()
            );
        }

        let deleted = router
            .oneshot(
                Request::delete("/api/modes/table")
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
        assert!(!directory.path().join("modes/table").exists());
        assert!(!directory.path().join("modes/.music-mode-journal").exists());
        assert!(
            RecoveryJournalRepository::unfinished_recovery_journals(
                runtime.storage.as_ref(),
                RecoveryDomain::Modes,
            )
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))?
            .is_empty()
        );
        assert!(
            runtime
                .modes
                .snapshot()
                .is_some_and(|catalog| catalog.modes.is_empty())
        );

        runtime.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn startup_rolls_back_an_interrupted_mode_publication()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let config = runtime_config(directory.path())?;
        fs::create_dir_all(&config.modes_dir)?;
        let storage = SqliteStorage::open(SqliteStorageOptions::new(&config.database_path)).await?;
        let effects = FilesystemModeMutations::open(&config.modes_dir)?;
        let journal_id = RecoveryJournalId::new();
        let prepared = effects
            .prepare(
                &journal_id,
                &ModeMutation::CreateMode {
                    expected_generation: 1,
                    manifest: ModeDocument {
                        id: "interrupted".to_owned(),
                        name: "Interrupted".to_owned(),
                        theme: None,
                        panels: Vec::new(),
                        playlist_categories: Vec::new(),
                        interrupts: Vec::new(),
                        integrations: Default::default(),
                        default_crossfade_ms: 0,
                        default_soundboard: None,
                        extra: BTreeMap::new(),
                    },
                },
            )
            .await?;
        let mut draft =
            RecoveryJournalDraft::new(RecoveryDomain::Modes, prepared.operation, prepared.plan)?;
        draft.id = journal_id;
        let planned = RecoveryJournalRepository::create_recovery_journal(&storage, draft).await?;
        let applying = match RecoveryJournalRepository::transition_recovery_journal(
            &storage,
            &planned.id,
            RecoveryState::Planned,
            RecoveryState::Applying,
            serde_json::json!({"stage": "applying"}),
        )
        .await?
        {
            RecoveryTransition::Applied(entry) => entry,
            RecoveryTransition::Conflict(_) => return Err("journal transition conflicted".into()),
        };
        effects.apply(&applying).await?;
        assert!(config.modes_dir.join("interrupted/manifest.yaml").is_file());
        storage.close().await;
        drop(storage);

        let runtime = AppRuntime::start(config).await?;
        assert!(!runtime.config.modes_dir.join("interrupted").exists());
        assert!(
            !runtime
                .config
                .modes_dir
                .join(".music-mode-journal")
                .exists()
        );
        assert!(
            RecoveryJournalRepository::unfinished_recovery_journals(
                runtime.storage.as_ref(),
                RecoveryDomain::Modes,
            )
            .await?
            .is_empty()
        );
        runtime.shutdown().await?;
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
        let mut runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
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
        let cleanup_lookup = Arc::new(FakeCleanupLookup::new(CleanupNameScores::new(100, 20)?));
        runtime.cleanup_verification_service = Arc::new(CleanupVerificationService::new(
            runtime.storage.clone(),
            cleanup_lookup.clone(),
        ));
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
        let unauthorized_verify = router
            .clone()
            .oneshot(
                Request::post("/api/library/cleanup/verify")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"names":["private name"]}"#))?,
            )
            .await?;
        assert_eq!(unauthorized_verify.status(), StatusCode::UNAUTHORIZED);
        let unauthorized_batches = router
            .clone()
            .oneshot(Request::get("/api/library/cleanup/batches").body(Body::empty())?)
            .await?;
        assert_eq!(unauthorized_batches.status(), StatusCode::UNAUTHORIZED);

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

        let empty_verify = router
            .clone()
            .oneshot(
                Request::post("/api/library/cleanup/verify")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(r#"{"names":[]}"#))?,
            )
            .await?;
        assert_eq!(empty_verify.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let verify = router
            .clone()
            .oneshot(
                Request::post("/api/library/cleanup/verify")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(r#"{"names":["Andrey Vinogradov"]}"#))?,
            )
            .await?;
        assert_eq!(verify.status(), StatusCode::OK);
        let verify_json: Value =
            serde_json::from_slice(&to_bytes(verify.into_body(), 1024 * 1024).await?)?;
        assert_eq!(
            verify_json,
            serde_json::json!({"verified": 1, "failed": []})
        );

        let cached_verify = router
            .clone()
            .oneshot(
                Request::post("/api/library/cleanup/verify")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(r#"{"names":["Andrey Vinogradov"]}"#))?,
            )
            .await?;
        let cached_json: Value =
            serde_json::from_slice(&to_bytes(cached_verify.into_body(), 1024 * 1024).await?)?;
        assert_eq!(
            cached_json,
            serde_json::json!({"verified": 0, "failed": []})
        );
        assert_eq!(
            cleanup_lookup.calls.lock().await.as_slice(),
            ["Andrey Vinogradov"]
        );

        let cleanup_batches = router
            .clone()
            .oneshot(
                Request::get("/api/library/cleanup/batches")
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(cleanup_batches.status(), StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<Value>(
                &to_bytes(cleanup_batches.into_body(), 1024 * 1024).await?
            )?,
            serde_json::json!([])
        );
        let missing_cleanup_batch = router
            .clone()
            .oneshot(
                Request::get("/api/library/cleanup/batches/999")
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(missing_cleanup_batch.status(), StatusCode::NOT_FOUND);

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
