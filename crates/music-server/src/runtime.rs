use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;

#[cfg(test)]
#[path = "runtime_cleanup_rejection_tests.rs"]
mod cleanup_rejection_tests;
use music_analysis::{AnalysisExecutor, AudioContextAnalyzer, FfmpegContextAnalyzer, VoiceBackend};
use music_application::assistant::{
    AssistantRepository, AssistantService, LocalAnalysisRepository, LocalAnalysisService,
    ModelEvaluationRepository, ProviderCredentialSource, ProviderRepository, VoiceAnalyzerState,
};
use music_application::cleanup::{
    CleanupMutationRepository, CleanupNameLookup, CleanupRepository, CleanupService,
    CleanupVerificationRepository, CleanupVerificationService,
};
use music_application::cleanup_sources::{
    CleanupSourceRepository, CleanupSourceRuntime, CleanupSourceService,
};
use music_application::jobs::{
    JobCoordinatorError, JobHandler, JobRepository, JobService, SpawnedJobCoordinator,
    start_job_coordinator,
};
use music_application::library::{
    LibraryCatalogSink, LibraryCoordinatorHandle, LibraryRepository, LibraryService,
    ReconciliationStatus, SpawnedLibraryCoordinator, start_library_coordinator,
};
use music_application::modes::{
    ModeCatalogSink, ModeCoordinatorHandle, ModeLoadState, ModeMutationDataEffects,
    SpawnedModeCoordinator, start_mutable_mode_coordinator_with_data,
};
use music_application::playback::{
    CatalogSnapshot, PlaybackActorConfig, PlaybackActorHandle, SpawnedPlaybackActor,
    SystemPlaybackClock, SystemQueueRandom, start_playback_actor,
};
use music_application::playlists::{PlaylistRepository, PlaylistService};
use music_application::recovery::RecoveryJournalRepository;
use music_application::sfx::SfxCoordinator;
use music_media::{
    FfmpegTools, FilesystemLibraryDiscovery, FilesystemLibraryMutations,
    FilesystemModeCatalogSource, FilesystemModeMutations, FilesystemSfxEffects, LibraryRoot,
    MetadataAdapter, SfxRoot,
};
use music_storage::{SqliteStorage, SqliteStorageOptions};
use tokio::net::TcpListener;
use tokio::time::{Instant, MissedTickBehavior};
use tracing_subscriber::EnvFilter;

use crate::admin::{BackupService, MaintenanceGate, pending_restore_journal};
use crate::analysis::ContextAnalysisJobHandler;
use crate::auth::RuntimeAuth;
use crate::blocking::BlockingMediaExecutor;
use crate::cleanup::MusicBrainzNameLookup;
use crate::cleanup_enrichment::{
    CleanupConnectorConfig, CleanupEnrichmentServices, cleanup_enrichment_handler,
};
use crate::config::AppConfig;
use crate::devices::RuntimeDevices;
use crate::error::RuntimeError;
use crate::health::{Component, ComponentStatus, HealthRegistry, ReadinessSnapshot};
use crate::http::{RuntimeServices, build_router};
use crate::library::RuntimeLibrary;
use crate::model_jobs::{model_evaluation_job_handlers, model_feature_job_handlers};
use crate::provider_api::{RuntimeProviders, provider_runtime_contract_digest};
use crate::provider_credentials::RuntimeCredentialStore;
use crate::provider_transport::ProviderNetworkBoundary;
use crate::sfx::RuntimeSfx;
use crate::supervisor::{CriticalTaskError, TaskSupervisor};

const DATABASE_HEALTH_INTERVAL: Duration = Duration::from_secs(30);
const DATABASE_HEALTH_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug)]
pub struct AppRuntime {
    config: Arc<AppConfig>,
    health: HealthRegistry,
    supervisor: TaskSupervisor,
    storage: Arc<SqliteStorage>,
    playback: PlaybackActorHandle,
    auth: Arc<RuntimeAuth>,
    assistant: Arc<AssistantService>,
    local_analysis: Arc<LocalAnalysisService>,
    backup: Arc<BackupService>,
    devices: Arc<RuntimeDevices>,
    library: LibraryCoordinatorHandle,
    modes: ModeCoordinatorHandle,
    library_service: Arc<LibraryService>,
    cleanup_service: Arc<CleanupService>,
    cleanup_verification_service: Arc<CleanupVerificationService>,
    cleanup_source_service: Arc<CleanupSourceService>,
    playlist_service: Arc<PlaylistService>,
    providers: Arc<RuntimeProviders>,
    jobs: Arc<JobService>,
    sfx: Arc<RuntimeSfx>,
    library_root: LibraryRoot,
    library_metadata: MetadataAdapter,
    media_executor: BlockingMediaExecutor,
}

impl AppRuntime {
    pub async fn start(config: AppConfig) -> Result<Self, RuntimeError> {
        if let Some(journal) = pending_restore_journal(&config.database_path) {
            return Err(RuntimeError::PendingRestore { journal });
        }
        let config = Arc::new(config);
        let health = HealthRegistry::new();
        health.set_component(Component::Configuration, ComponentStatus::Ready);
        health.set_component(Component::Filesystem, ComponentStatus::Starting);
        health.set_component(Component::InstanceLock, ComponentStatus::Starting);
        health.set_component(Component::Database, ComponentStatus::Starting);
        health.set_component(Component::DatabaseSchema, ComponentStatus::Starting);
        health.set_component(Component::Playback, ComponentStatus::Starting);
        health.set_component(Component::Authentication, ComponentStatus::Starting);
        health.set_component(Component::LibraryCoordinator, ComponentStatus::Starting);
        health.set_component(Component::Library, ComponentStatus::Starting);
        health.set_component(Component::BackgroundJobs, ComponentStatus::Starting);
        health.set_component(Component::VoiceAnalysis, ComponentStatus::Starting);
        health.set_component(Component::ModeCoordinator, ComponentStatus::Starting);
        health.set_component(Component::Modes, ComponentStatus::Starting);
        health.set_component(Component::Sfx, ComponentStatus::Starting);
        health.set_component(Component::Runtime, ComponentStatus::Starting);

        initialize_directories(&config, &health)?;
        let storage =
            match SqliteStorage::open(SqliteStorageOptions::new(&config.database_path)).await {
                Ok(storage) => Arc::new(storage),
                Err(error) => {
                    health.set_component(Component::InstanceLock, ComponentStatus::Failed);
                    health.set_component(Component::Database, ComponentStatus::Failed);
                    health.set_component(Component::DatabaseSchema, ComponentStatus::Failed);
                    return Err(error.into());
                }
            };
        if let Err(error) = storage.healthcheck().await {
            health.set_component(Component::Database, ComponentStatus::Failed);
            return Err(error.into());
        }
        let retention = &storage.migration_outcome().backup_retention;
        if !retention.removed.is_empty() {
            tracing::info!(removed = ?retention.removed, "removed older pre-migration backups");
        }
        for (path, error) in &retention.failed {
            tracing::warn!(path = %path.display(), error, "could not remove an older pre-migration backup");
        }
        health.set_component(Component::InstanceLock, ComponentStatus::Ready);
        health.set_component(Component::Database, ComponentStatus::Ready);
        health.set_component(Component::DatabaseSchema, ComponentStatus::Ready);

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
                "applied pending database migrations"
            );
        }

        let auth = Arc::new(RuntimeAuth::new(Arc::clone(&storage), &config)?);
        let assistant_repository: Arc<dyn AssistantRepository> = storage.clone();
        let provider_repository: Arc<dyn ProviderRepository> = storage.clone();
        let evaluation_repository: Arc<dyn ModelEvaluationRepository> = storage.clone();
        let provider_credentials = Arc::new(RuntimeCredentialStore::new(&config));
        let provider_network = Arc::new(ProviderNetworkBoundary::new());
        let providers = Arc::new(RuntimeProviders::new(
            storage.clone(),
            provider_repository,
            evaluation_repository,
            Arc::clone(&provider_credentials),
            provider_network,
            provider_runtime_contract_digest(),
            crate::provider_api::provider_role_contract_digests(),
        ));
        let local_analysis_repository: Arc<dyn LocalAnalysisRepository> = storage.clone();
        let ffmpeg = ffmpeg_executable();
        let voice_backend =
            VoiceBackend::initialize(config.assistant_voice_model_path.as_deref(), ffmpeg.clone());
        let voice_analyzer = voice_backend.status;
        let voice_worker_factory = voice_backend.worker_factory;
        health.set_component(
            Component::VoiceAnalysis,
            if voice_analyzer.status == VoiceAnalyzerState::Unavailable {
                ComponentStatus::Degraded
            } else {
                ComponentStatus::Ready
            },
        );
        let local_analysis = Arc::new(LocalAnalysisService::with_voice_analyzer(
            Arc::clone(&local_analysis_repository),
            voice_analyzer.clone(),
        ));
        let assistant = Arc::new(
            AssistantService::new(assistant_repository)
                .with_model_review(providers.provider_service(), Arc::clone(&local_analysis)),
        );
        let backup = Arc::new(BackupService::new(
            Arc::clone(&storage),
            Arc::clone(&config),
            MaintenanceGate::default(),
        ));
        let devices = Arc::new(RuntimeDevices::new(Arc::clone(&storage)));
        health.set_component(Component::Authentication, ComponentStatus::Ready);

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
                health.set_component(Component::Playback, ComponentStatus::Failed);
                storage.close().await;
                return Err(error.into());
            }
        };
        health.set_component(Component::Playback, ComponentStatus::Ready);
        let mode_source = Arc::new(FilesystemModeCatalogSource::open(&config.modes_dir)?);
        let mode_sink: Arc<dyn ModeCatalogSink> = Arc::new(playback.clone());
        let mode_journal: Arc<dyn RecoveryJournalRepository> = storage.clone();
        let mode_data_effects: Arc<dyn ModeMutationDataEffects> = storage.clone();
        let mode_effects = Arc::new(FilesystemModeMutations::open(&config.modes_dir).map_err(
            |_| {
                music_application::modes::ModeCoordinatorError::MutationRecovery(
                    "mode mutation filesystem could not be initialized",
                )
            },
        )?);
        let spawned_modes = start_mutable_mode_coordinator_with_data(
            mode_source,
            mode_sink,
            mode_journal,
            mode_effects,
            mode_data_effects,
        )
        .await?;
        let modes = supervise_modes(&supervisor, spawned_modes, health.clone())?;
        let initial_mode_status = modes.wait_until_initialized().await?;
        health.set_component(Component::ModeCoordinator, ComponentStatus::Ready);
        apply_mode_health(&health, initial_mode_status.state);
        let media_executor = BlockingMediaExecutor::default();
        let sfx_root = SfxRoot::open(&config.sfx_library_dir)?;
        let sfx_journal: Arc<dyn RecoveryJournalRepository> = storage.clone();
        let sfx_effects = Arc::new(FilesystemSfxEffects::new(sfx_root.clone()));
        let sfx_coordinator = Arc::new(SfxCoordinator::start(sfx_journal, sfx_effects).await?);
        let sfx = Arc::new(RuntimeSfx {
            coordinator: sfx_coordinator,
            root: sfx_root,
            max_upload_files: config.max_upload_files,
            max_upload_file_bytes: config.max_upload_file_bytes,
            media_executor: media_executor.clone(),
        });
        health.set_component(Component::Sfx, ComponentStatus::Ready);
        let library_root = LibraryRoot::open(&config.music_dir)?;
        let library_metadata =
            MetadataAdapter::with_ffmpeg(FfmpegTools::new(ffmpeg.clone(), ffprobe_executable()));
        let discovery = Arc::new(FilesystemLibraryDiscovery::new(
            library_root.clone(),
            library_metadata.clone(),
        ));
        let mutation_repository: Arc<dyn CleanupMutationRepository> = storage.clone();
        let read_repository: Arc<dyn LibraryRepository> = storage.clone();
        let cleanup_repository: Arc<dyn CleanupRepository> = storage.clone();
        let cleanup_verification_repository: Arc<dyn CleanupVerificationRepository> =
            storage.clone();
        let cleanup_source_repository: Arc<dyn CleanupSourceRepository> = storage.clone();
        let cleanup_lookup = Arc::new(MusicBrainzNameLookup::new().map_err(|source| {
            RuntimeError::io(
                "initialize the MusicBrainz HTTP client",
                io::Error::other(source),
            )
        })?);
        let cleanup_name_lookup: Arc<dyn CleanupNameLookup> = cleanup_lookup.clone();
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
            cleanup_name_lookup,
        ));
        let cleanup_connector_config = CleanupConnectorConfig::new(
            config
                .cleanup_acoustid_api_key
                .as_ref()
                .map(|secret| music_storage::SecretString::new(secret.expose_secret())),
            config
                .cleanup_lastfm_api_key
                .as_ref()
                .map(|secret| music_storage::SecretString::new(secret.expose_secret())),
            config.cleanup_fpcalc_path.clone(),
        );
        let cleanup_source_service = Arc::new(CleanupSourceService::new(
            cleanup_source_repository,
            provider_credentials as Arc<dyn ProviderCredentialSource>,
            CleanupSourceRuntime {
                acoustid_configured: cleanup_connector_config.acoustid_configured(),
                fpcalc_available: cleanup_connector_config.fpcalc_available().await,
                lastfm_configured: cleanup_connector_config.lastfm_configured(),
            },
        ));
        let playlist_repository: Arc<dyn PlaylistRepository> = storage.clone();
        let playlist_service = Arc::new(PlaylistService::new(playlist_repository));
        let job_repository: Arc<dyn JobRepository> = storage.clone();
        let analysis_executor = AnalysisExecutor::new(config.assistant_library_context_workers)
            .map_err(|source| {
                RuntimeError::io(
                    "start the fixed analysis executor",
                    io::Error::other(source),
                )
            })?;
        let context_analyzer: Arc<dyn AudioContextAnalyzer> =
            Arc::new(FfmpegContextAnalyzer::new(ffmpeg, ffprobe_executable()));
        let mut job_handlers: Vec<Arc<dyn JobHandler>> = vec![
            Arc::new(
                cleanup_enrichment_handler(
                    CleanupEnrichmentServices {
                        cleanup: Arc::clone(&cleanup_service),
                        cache: storage.clone(),
                        analyses: Arc::clone(&local_analysis_repository),
                        assistant: Arc::clone(&assistant),
                        sources: Arc::clone(&cleanup_source_service),
                        musicbrainz: cleanup_lookup,
                    },
                    library_root.clone(),
                    cleanup_connector_config,
                )
                .map_err(|source| {
                    RuntimeError::io(
                        "initialize the cleanup connector HTTP client",
                        io::Error::other(source),
                    )
                })?,
            ),
            Arc::new(ContextAnalysisJobHandler::new(
                Arc::clone(&local_analysis_repository),
                library_root.clone(),
                analysis_executor,
                context_analyzer,
                voice_analyzer,
                voice_worker_factory,
            )),
        ];
        job_handlers.push(Arc::new(
            music_application::cleanup_enrichment::ai::CleanupAiJobHandler {
                cleanup: Arc::clone(&cleanup_service),
                cache: storage.clone(),
                sources: Arc::clone(&cleanup_source_service),
                quality: providers.quality_service(),
                transport: providers.network_boundary(),
            },
        ));
        job_handlers.extend(model_evaluation_job_handlers(
            providers.quality_service(),
            providers.network_boundary(),
        ));
        job_handlers.extend(model_feature_job_handlers(
            providers.quality_service(),
            providers.network_boundary(),
            Arc::clone(&assistant),
            Arc::clone(&local_analysis),
            local_analysis_repository,
            Some(Arc::new(music_application::assistant::ModelBatchServices {
                repository: storage.clone(),
                transport: providers.network_boundary(),
                providers: providers.provider_service(),
            })),
        ));
        let jobs = supervise_jobs(
            &supervisor,
            start_job_coordinator(job_repository, job_handlers).await?,
        )?;
        health.set_component(Component::BackgroundJobs, ComponentStatus::Ready);
        start_model_batch_monitor(&supervisor, providers.batches.clone(), Arc::clone(&jobs))?;
        health.set_component(Component::LibraryCoordinator, ComponentStatus::Ready);
        apply_library_health(&health, library.status().status);
        library.request_reconciliation()?;
        start_database_monitor(&supervisor, Arc::clone(&storage))?;
        health.set_component(Component::Runtime, ComponentStatus::Ready);

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
            assistant,
            local_analysis,
            backup,
            devices,
            library,
            modes,
            library_service,
            cleanup_service,
            cleanup_verification_service,
            cleanup_source_service,
            playlist_service,
            providers,
            jobs,
            sfx,
            library_root,
            library_metadata,
            media_executor,
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
                shutdown: self.supervisor.cancellation_token(),
                assistant: Arc::clone(&self.assistant),
                local_analysis: Arc::clone(&self.local_analysis),
                playback: self.playback.clone(),
                auth: Arc::clone(&self.auth),
                backup: Arc::clone(&self.backup),
                devices: Arc::clone(&self.devices),
                library: Arc::new(RuntimeLibrary {
                    service: Arc::clone(&self.library_service),
                    cleanup: Arc::clone(&self.cleanup_service),
                    cleanup_verification: Arc::clone(&self.cleanup_verification_service),
                    cleanup_sources: Arc::clone(&self.cleanup_source_service),
                    coordinator: self.library.clone(),
                    root: self.library_root.clone(),
                    metadata: self.library_metadata.clone(),
                    max_upload_files: self.config.max_upload_files,
                    max_upload_file_bytes: self.config.max_upload_file_bytes,
                    media_executor: self.media_executor.clone(),
                }),
                modes: self.modes.clone(),
                playlists: Arc::clone(&self.playlist_service),
                providers: Arc::clone(&self.providers),
                jobs: Arc::clone(&self.jobs),
                sfx: Arc::clone(&self.sfx),
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
                    .set_component(Component::Http, ComponentStatus::Failed);
                let _ = self.shutdown().await;
                return Err(error);
            }
        };
        self.health
            .set_component(Component::Http, ComponentStatus::Ready);
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
            .set_component(Component::Http, ComponentStatus::Starting);
        self.jobs.shutdown();
        let supervisor_result = self.supervisor.shutdown(SHUTDOWN_TIMEOUT).await;
        self.storage.close().await;
        supervisor_result
    }
}

fn supervise_playback(
    supervisor: &TaskSupervisor,
    spawned: SpawnedPlaybackActor,
) -> Result<PlaybackActorHandle, RuntimeError> {
    let handle = spawned.handle;
    let shutdown_handle = handle.clone();
    let cancellation = supervisor.cancellation_token();
    let mut task = spawned.task;
    supervisor.spawn_critical("playback-owner", Component::Playback, async move {
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
    supervisor.spawn_critical("mode-owner", Component::ModeCoordinator, async move {
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

fn supervise_jobs(
    supervisor: &TaskSupervisor,
    spawned: SpawnedJobCoordinator,
) -> Result<Arc<JobService>, RuntimeError> {
    let service = spawned.service;
    let shutdown_service = Arc::clone(&service);
    let cancellation = supervisor.cancellation_token();
    let mut local_task = spawned.local_task;
    let mut provider_task = spawned.provider_task;
    supervisor.spawn_critical(
        "background-job-owner",
        Component::BackgroundJobs,
        async move {
            tokio::select! {
                local_result = &mut local_task => {
                    shutdown_service.shutdown();
                    let provider_result = provider_task.await;
                    map_job_exits(local_result, provider_result)
                }
                provider_result = &mut provider_task => {
                    shutdown_service.shutdown();
                    let local_result = local_task.await;
                    map_job_exits(local_result, provider_result)
                }
                () = cancellation.cancelled() => {
                    shutdown_service.shutdown();
                    let (local_result, provider_result) = tokio::join!(local_task, provider_task);
                    map_job_exits(local_result, provider_result)
                }
            }
        },
    )?;
    Ok(service)
}

fn map_job_exits(
    local: Result<Result<(), JobCoordinatorError>, tokio::task::JoinError>,
    provider: Result<Result<(), JobCoordinatorError>, tokio::task::JoinError>,
) -> Result<(), CriticalTaskError> {
    for result in [local, provider] {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(_)) => return Err(CriticalTaskError::new("background_job_owner_failed")),
            Err(_) => return Err(CriticalTaskError::new("background_job_owner_panicked")),
        }
    }
    Ok(())
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
    supervisor.spawn_critical("library-owner", Component::LibraryCoordinator, async move {
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
        Component::Library,
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
    health.set_component(Component::Modes, health_status);
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
    supervisor.spawn_critical("database-monitor", Component::Database, async move {
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

fn start_model_batch_monitor(
    supervisor: &TaskSupervisor,
    repository: Arc<dyn music_application::assistant::ModelBatchRepository>,
    jobs: Arc<music_application::jobs::JobService>,
) -> Result<(), RuntimeError> {
    let cancellation = supervisor.cancellation_token();
    supervisor.spawn_critical("model-batch-monitor", Component::BackgroundJobs, async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                () = cancellation.cancelled() => return Ok(()),
                _ = interval.tick() => {
                    match repository.pending_model_batch().await {
                        Ok(Some(batch)) if batch.remote_batch_id.is_some() => {
                            if jobs.enqueue_unique_active(music_application::assistant::MODEL_TAGGING_BATCH_COLLECT_JOB_KIND,
                                serde_json::json!({"batch_id":batch.id,"role_id":"music_tagger"})).await.is_err() {
                                tracing::warn!("model_batch_reconciliation_enqueue_failed");
                            }
                        }
                        Err(_) => tracing::warn!("model_batch_reconciliation_lookup_failed"),
                        _ => {}
                    }
                }
            }
        }
    })
}

fn initialize_directories(config: &AppConfig, health: &HealthRegistry) -> Result<(), RuntimeError> {
    crate::storage_admin::create_storage_directories(config)
        .map_err(|source| RuntimeError::io("initialize storage directories", source))?;
    health.set_component(Component::Filesystem, ComponentStatus::Ready);

    match crate::storage_admin::seed_modes_if_empty(config) {
        Ok(
            crate::storage_admin::ModeSeedOutcome::NotConfigured
            | crate::storage_admin::ModeSeedOutcome::NotRequested
            | crate::storage_admin::ModeSeedOutcome::Seeded
            | crate::storage_admin::ModeSeedOutcome::TargetNotEmpty,
        ) => {
            health.set_component(Component::ModeSeed, ComponentStatus::Ready);
        }
        Ok(crate::storage_admin::ModeSeedOutcome::SourceUnavailable) => {
            health.set_component(Component::ModeSeed, ComponentStatus::Degraded);
            tracing::warn!("configured mode seed is unavailable; continuing without seeding");
        }
        Err(error) => {
            health.set_component(Component::ModeSeed, ComponentStatus::Degraded);
            tracing::warn!(error_kind = ?error.kind(), "mode seeding was incomplete");
        }
    }
    Ok(())
}

#[cfg(test)]
fn ffmpeg_executable() -> std::ffi::OsString {
    std::env::var_os("MUSIC_TEST_FFMPEG").unwrap_or_else(|| "ffmpeg".into())
}

#[cfg(not(test))]
fn ffmpeg_executable() -> std::ffi::OsString {
    "ffmpeg".into()
}

#[cfg(test)]
fn ffprobe_executable() -> std::ffi::OsString {
    std::env::var_os("MUSIC_TEST_FFPROBE").unwrap_or_else(|| "ffprobe".into())
}

#[cfg(not(test))]
fn ffprobe_executable() -> std::ffi::OsString {
    "ffprobe".into()
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
