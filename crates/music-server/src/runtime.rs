use std::fs;
use std::future::Future;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use music_application::playback::{
    CatalogSnapshot, PlaybackActorConfig, PlaybackActorHandle, SpawnedPlaybackActor,
    SystemPlaybackClock, SystemQueueRandom, start_playback_actor,
};
use music_storage::{SqliteStorage, SqliteStorageOptions};
use tokio::net::TcpListener;
use tokio::time::{Instant, MissedTickBehavior};
use tracing_subscriber::EnvFilter;

use crate::config::AppConfig;
use crate::error::RuntimeError;
use crate::health::{ComponentStatus, HealthRegistry, ReadinessSnapshot};
use crate::http::build_router;
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

    pub fn router(&self) -> Result<Router, RuntimeError> {
        build_router(&self.config, self.health.clone(), self.playback.clone())
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
            axum::serve(listener, router)
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

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
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
}
