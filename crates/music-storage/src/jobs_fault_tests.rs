use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use music_application::jobs::{
    JobCheckpointPolicy, JobClaim, JobCoordinatorError, JobDefinition, JobDependencyError,
    JobFinish, JobFuture, JobHandler, JobHandlerError, JobHandlerFuture, JobLane, JobLeaseState,
    JobListFilter, JobProgress, JobRecord, JobRepository, JobStatus, NewJob, SpawnedJobCoordinator,
    start_job_coordinator,
};
use serde_json::{Map, Value, json};
use tempfile::tempdir;
use tokio::sync::Notify;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Instant, sleep, timeout};

use crate::{SqliteStorage, SqliteStorageOptions};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

const TEST_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum FaultPoint {
    Claim,
    Checkpoint,
    Completion,
}

#[derive(Debug)]
struct FaultInjectingRepository {
    inner: Arc<SqliteStorage>,
    point: FaultPoint,
    fired: AtomicBool,
}

impl FaultInjectingRepository {
    fn new(inner: Arc<SqliteStorage>, point: FaultPoint) -> Self {
        Self {
            inner,
            point,
            fired: AtomicBool::new(false),
        }
    }

    fn trip(&self, point: FaultPoint) -> bool {
        self.point == point && !self.fired.swap(true, Ordering::SeqCst)
    }
}

#[derive(Debug)]
struct InjectedRepositoryFailure(&'static str);

impl Display for InjectedRepositoryFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for InjectedRepositoryFailure {}

fn injected_failure(detail: &'static str) -> JobDependencyError {
    Box::new(InjectedRepositoryFailure(detail))
}

impl JobRepository for FaultInjectingRepository {
    fn create<'a>(&'a self, job: &'a NewJob) -> JobFuture<'a, JobRecord> {
        self.inner.create(job)
    }

    fn create_unique_active<'a>(&'a self, job: &'a NewJob) -> JobFuture<'a, (JobRecord, bool)> {
        self.inner.create_unique_active(job)
    }

    fn get<'a>(&'a self, id: &'a str) -> JobFuture<'a, Option<JobRecord>> {
        self.inner.get(id)
    }

    fn list<'a>(&'a self, filter: &'a JobListFilter) -> JobFuture<'a, Vec<JobRecord>> {
        self.inner.list(filter)
    }

    fn request_cancellation<'a>(&'a self, id: &'a str) -> JobFuture<'a, Option<(JobRecord, bool)>> {
        self.inner.request_cancellation(id)
    }

    fn claim_next<'a>(&'a self, lane: JobLane) -> JobFuture<'a, Option<JobClaim>> {
        Box::pin(async move {
            let claim = self.inner.claim_next(lane).await?;
            if claim.is_some() && self.trip(FaultPoint::Claim) {
                return Err(injected_failure("injected failure after claim commit"));
            }
            Ok(claim)
        })
    }

    fn lease_state<'a>(
        &'a self,
        id: &'a str,
        execution_id: &'a str,
    ) -> JobFuture<'a, JobLeaseState> {
        self.inner.lease_state(id, execution_id)
    }

    fn update_progress<'a>(
        &'a self,
        claim: &'a JobClaim,
        progress: &'a JobProgress,
    ) -> JobFuture<'a, JobLeaseState> {
        self.inner.update_progress(claim, progress)
    }

    fn checkpoint<'a>(
        &'a self,
        claim: &'a JobClaim,
        result: &'a Map<String, Value>,
    ) -> JobFuture<'a, JobLeaseState> {
        Box::pin(async move {
            let state = self.inner.checkpoint(claim, result).await?;
            if self.trip(FaultPoint::Checkpoint) {
                return Err(injected_failure("injected failure after checkpoint commit"));
            }
            Ok(state)
        })
    }

    fn finish<'a>(
        &'a self,
        claim: &'a JobClaim,
        finish: &'a JobFinish,
    ) -> JobFuture<'a, JobLeaseState> {
        Box::pin(async move {
            let state = self.inner.finish(claim, finish).await?;
            if self.trip(FaultPoint::Completion) {
                return Err(injected_failure("injected failure after completion commit"));
            }
            Ok(state)
        })
    }

    fn recover_interrupted<'a>(
        &'a self,
        definitions: &'a BTreeMap<String, JobDefinition>,
    ) -> JobFuture<'a, usize> {
        self.inner.recover_interrupted(definitions)
    }
}

#[derive(Debug)]
struct ImmediateHandler {
    definition: JobDefinition,
    effects: AtomicUsize,
}

impl ImmediateHandler {
    fn new(kind: &'static str, lane: JobLane, restartable: bool) -> Self {
        Self {
            definition: definition(kind, lane, restartable),
            effects: AtomicUsize::new(0),
        }
    }

    fn effect_count(&self) -> usize {
        self.effects.load(Ordering::SeqCst)
    }
}

impl JobHandler for ImmediateHandler {
    fn definition(&self) -> JobDefinition {
        self.definition.clone()
    }

    fn execute<'a>(
        &'a self,
        _context: &'a music_application::jobs::JobExecutionContext,
        _parameters: Map<String, Value>,
    ) -> JobHandlerFuture<'a> {
        let effects = self.effects.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            Ok(Value::Object(Map::from_iter([(
                "effects".to_owned(),
                json!(effects),
            )])))
        })
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum PausePoint {
    Never,
    BeforeFirstCheckpoint,
    AfterFirstCheckpoint,
}

#[derive(Debug)]
struct DurableEffectHandler {
    definition: JobDefinition,
    marker: PathBuf,
    pause: PausePoint,
    attempts: AtomicUsize,
    effects: AtomicUsize,
}

impl DurableEffectHandler {
    fn new(
        kind: &'static str,
        lane: JobLane,
        restartable: bool,
        marker: PathBuf,
        pause: PausePoint,
    ) -> Self {
        Self {
            definition: definition(kind, lane, restartable),
            marker,
            pause,
            attempts: AtomicUsize::new(0),
            effects: AtomicUsize::new(0),
        }
    }

    fn attempt_count(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }

    fn effect_count(&self) -> usize {
        self.effects.load(Ordering::SeqCst)
    }

    fn apply_idempotent_effect(&self) -> Result<(), JobHandlerError> {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.marker)
        {
            Ok(mut file) => {
                file.write_all(b"applied")
                    .and_then(|()| file.sync_all())
                    .map_err(|_| JobHandlerError::new("test effect write failed"))?;
                self.effects.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let contents = fs::read(&self.marker)
                    .map_err(|_| JobHandlerError::new("test effect read failed"))?;
                if contents == b"applied" {
                    Ok(())
                } else {
                    Err(JobHandlerError::new("test effect marker was inconsistent"))
                }
            }
            Err(_) => Err(JobHandlerError::new("test effect creation failed")),
        }
    }
}

impl JobHandler for DurableEffectHandler {
    fn definition(&self) -> JobDefinition {
        self.definition.clone()
    }

    fn execute<'a>(
        &'a self,
        context: &'a music_application::jobs::JobExecutionContext,
        _parameters: Map<String, Value>,
    ) -> JobHandlerFuture<'a> {
        Box::pin(async move {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            let checkpointed = context
                .related_job(context.job_id())
                .await
                .map_err(JobHandlerError::from_execution)?
                .and_then(|job| job.result)
                .and_then(|result| result.get("effect_applied").cloned())
                .and_then(|value| value.as_bool())
                .unwrap_or(false);

            if !checkpointed {
                self.apply_idempotent_effect()?;
                if attempt == 0 && self.pause == PausePoint::BeforeFirstCheckpoint {
                    std::future::pending::<()>().await;
                }
                context
                    .checkpoint(Map::from_iter([(
                        "effect_applied".to_owned(),
                        Value::Bool(true),
                    )]))
                    .await
                    .map_err(JobHandlerError::from_execution)?;
            }

            if attempt == 0 && self.pause == PausePoint::AfterFirstCheckpoint {
                std::future::pending::<()>().await;
            }

            Ok(Value::Object(Map::from_iter([
                ("complete".to_owned(), Value::Bool(true)),
                ("effect_applied".to_owned(), Value::Bool(true)),
            ])))
        })
    }
}

#[derive(Debug)]
struct ReleasableHandler {
    definition: JobDefinition,
    release: Notify,
    effects: AtomicUsize,
}

impl ReleasableHandler {
    fn new(kind: &'static str) -> Self {
        Self {
            definition: definition(kind, JobLane::Local, true),
            release: Notify::new(),
            effects: AtomicUsize::new(0),
        }
    }

    fn release(&self) {
        self.release.notify_one();
    }

    fn effect_count(&self) -> usize {
        self.effects.load(Ordering::SeqCst)
    }
}

impl JobHandler for ReleasableHandler {
    fn definition(&self) -> JobDefinition {
        self.definition.clone()
    }

    fn execute<'a>(
        &'a self,
        context: &'a music_application::jobs::JobExecutionContext,
        _parameters: Map<String, Value>,
    ) -> JobHandlerFuture<'a> {
        Box::pin(async move {
            self.effects.fetch_add(1, Ordering::SeqCst);
            context
                .checkpoint(Map::from_iter([(
                    "effect_applied".to_owned(),
                    Value::Bool(true),
                )]))
                .await
                .map_err(JobHandlerError::from_execution)?;
            self.release.notified().await;
            Ok(Value::Object(Map::from_iter([(
                "complete".to_owned(),
                Value::Bool(true),
            )])))
        })
    }
}

fn definition(kind: &'static str, lane: JobLane, restartable: bool) -> JobDefinition {
    JobDefinition {
        kind,
        schema_version: 1,
        lane,
        restartable,
        checkpoint_policy: JobCheckpointPolicy::Replace,
    }
}

fn new_job(id: &str, definition: JobDefinition) -> NewJob {
    NewJob {
        id: id.to_owned(),
        definition,
        parameters: Map::new(),
        retry_of_id: None,
    }
}

fn one_handler(handler: Arc<dyn JobHandler>) -> Vec<Arc<dyn JobHandler>> {
    vec![handler]
}

async fn wait_for_job<F>(storage: &SqliteStorage, id: &str, predicate: F) -> TestResult<JobRecord>
where
    F: Fn(&JobRecord) -> bool,
{
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        let job = storage.get(id).await?;
        if let Some(job) = &job
            && predicate(job)
        {
            return Ok(job.clone());
        }
        if Instant::now() >= deadline {
            let state = job.map_or_else(
                || "missing".to_owned(),
                |job| {
                    format!(
                        "status={}, attempts={}, checkpoint={}",
                        job.status.as_str(),
                        job.attempts,
                        job.result.is_some()
                    )
                },
            );
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("job state timed out for {id}: {state}"),
            )
            .into());
        }
        sleep(POLL_INTERVAL).await;
    }
}

async fn await_lane(
    task: JoinHandle<Result<(), JobCoordinatorError>>,
) -> TestResult<Result<(), JobCoordinatorError>> {
    timeout(TEST_TIMEOUT, task)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "job lane timed out"))?
        .map_err(Into::into)
}

async fn stop_coordinator(coordinator: SpawnedJobCoordinator) -> TestResult {
    coordinator.service.shutdown();
    await_lane(coordinator.local_task).await??;
    await_lane(coordinator.provider_task).await??;
    Ok(())
}

async fn stop_other_lane(
    service: &Arc<music_application::jobs::JobService>,
    task: JoinHandle<Result<(), JobCoordinatorError>>,
) -> TestResult {
    service.shutdown();
    await_lane(task).await??;
    Ok(())
}

#[tokio::test]
async fn committed_claim_without_ack_is_recovered_before_any_effect() -> TestResult {
    let directory = tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("db"))).await?,
    );
    let handler = Arc::new(ImmediateHandler::new(
        "test.fault.claim",
        JobLane::Local,
        true,
    ));
    storage
        .create(&new_job("claim", handler.definition()))
        .await?;

    let fault = Arc::new(FaultInjectingRepository::new(
        storage.clone(),
        FaultPoint::Claim,
    ));
    let coordinator = start_job_coordinator(
        fault,
        one_handler(Arc::clone(&handler) as Arc<dyn JobHandler>),
    )
    .await?;
    let local_outcome = await_lane(coordinator.local_task).await?;
    assert_eq!(local_outcome, Err(JobCoordinatorError::Dependency));
    stop_other_lane(&coordinator.service, coordinator.provider_task).await?;

    let interrupted = storage.get("claim").await?.ok_or("claim job missing")?;
    assert_eq!(interrupted.status, JobStatus::Running);
    assert_eq!(interrupted.attempts, 1);
    assert_eq!(handler.effect_count(), 0);

    let resumed = start_job_coordinator(
        storage.clone(),
        one_handler(Arc::clone(&handler) as Arc<dyn JobHandler>),
    )
    .await?;
    let finished =
        wait_for_job(&storage, "claim", |job| job.status == JobStatus::Succeeded).await?;
    assert_eq!(finished.attempts, 2);
    assert_eq!(handler.effect_count(), 1);
    stop_coordinator(resumed).await
}

#[tokio::test]
async fn process_loss_after_external_effect_replays_idempotently() -> TestResult {
    let directory = tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("db"))).await?,
    );
    let handler = Arc::new(DurableEffectHandler::new(
        "test.fault.effect",
        JobLane::Local,
        true,
        directory.path().join("effect.marker"),
        PausePoint::BeforeFirstCheckpoint,
    ));
    storage
        .create(&new_job("effect", handler.definition()))
        .await?;

    let coordinator = start_job_coordinator(
        storage.clone(),
        one_handler(Arc::clone(&handler) as Arc<dyn JobHandler>),
    )
    .await?;
    wait_for_job(&storage, "effect", |job| {
        job.status == JobStatus::Running && handler.attempt_count() == 1
    })
    .await?;
    assert_eq!(handler.effect_count(), 1);
    assert_eq!(
        storage.get("effect").await?.ok_or("effect missing")?.result,
        None
    );

    coordinator.local_task.abort();
    let aborted = timeout(TEST_TIMEOUT, coordinator.local_task)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "job lane abort timed out"))?;
    assert!(matches!(aborted, Err(error) if error.is_cancelled()));
    stop_other_lane(&coordinator.service, coordinator.provider_task).await?;

    let resumed = start_job_coordinator(
        storage.clone(),
        one_handler(Arc::clone(&handler) as Arc<dyn JobHandler>),
    )
    .await?;
    let finished =
        wait_for_job(&storage, "effect", |job| job.status == JobStatus::Succeeded).await?;
    assert_eq!(finished.attempts, 2);
    assert_eq!(handler.attempt_count(), 2);
    assert_eq!(handler.effect_count(), 1);
    stop_coordinator(resumed).await
}

#[tokio::test]
async fn committed_checkpoint_without_ack_resumes_after_the_safe_boundary() -> TestResult {
    let directory = tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("db"))).await?,
    );
    let handler = Arc::new(DurableEffectHandler::new(
        "test.fault.checkpoint",
        JobLane::Local,
        true,
        directory.path().join("checkpoint.marker"),
        PausePoint::Never,
    ));
    storage
        .create(&new_job("checkpoint", handler.definition()))
        .await?;

    let fault = Arc::new(FaultInjectingRepository::new(
        storage.clone(),
        FaultPoint::Checkpoint,
    ));
    let coordinator = start_job_coordinator(
        fault,
        one_handler(Arc::clone(&handler) as Arc<dyn JobHandler>),
    )
    .await?;
    let local_outcome = await_lane(coordinator.local_task).await?;
    assert_eq!(local_outcome, Err(JobCoordinatorError::Dependency));
    stop_other_lane(&coordinator.service, coordinator.provider_task).await?;

    let interrupted = storage
        .get("checkpoint")
        .await?
        .ok_or("checkpoint job missing")?;
    assert_eq!(interrupted.status, JobStatus::Running);
    assert_eq!(
        interrupted
            .result
            .as_ref()
            .and_then(|result| result.get("effect_applied")),
        Some(&Value::Bool(true))
    );
    assert_eq!(handler.effect_count(), 1);

    let resumed = start_job_coordinator(
        storage.clone(),
        one_handler(Arc::clone(&handler) as Arc<dyn JobHandler>),
    )
    .await?;
    let finished = wait_for_job(&storage, "checkpoint", |job| {
        job.status == JobStatus::Succeeded
    })
    .await?;
    assert_eq!(finished.attempts, 2);
    assert_eq!(handler.attempt_count(), 2);
    assert_eq!(handler.effect_count(), 1);
    stop_coordinator(resumed).await
}

#[tokio::test]
async fn committed_completion_without_ack_is_never_replayed() -> TestResult {
    let directory = tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("db"))).await?,
    );
    let handler = Arc::new(ImmediateHandler::new(
        "test.fault.completion",
        JobLane::Local,
        true,
    ));
    storage
        .create(&new_job("completion", handler.definition()))
        .await?;

    let fault = Arc::new(FaultInjectingRepository::new(
        storage.clone(),
        FaultPoint::Completion,
    ));
    let coordinator = start_job_coordinator(
        fault,
        one_handler(Arc::clone(&handler) as Arc<dyn JobHandler>),
    )
    .await?;
    let local_outcome = await_lane(coordinator.local_task).await?;
    assert_eq!(local_outcome, Err(JobCoordinatorError::Dependency));
    stop_other_lane(&coordinator.service, coordinator.provider_task).await?;

    let committed = storage
        .get("completion")
        .await?
        .ok_or("completion job missing")?;
    assert_eq!(committed.status, JobStatus::Succeeded);
    assert_eq!(committed.attempts, 1);
    assert_eq!(handler.effect_count(), 1);

    let resumed = start_job_coordinator(
        storage.clone(),
        one_handler(Arc::clone(&handler) as Arc<dyn JobHandler>),
    )
    .await?;
    sleep(Duration::from_millis(50)).await;
    assert_eq!(handler.effect_count(), 1);
    assert_eq!(
        storage
            .get("completion")
            .await?
            .ok_or("job missing")?
            .attempts,
        1
    );
    stop_coordinator(resumed).await
}

#[tokio::test]
async fn shutdown_requeues_restartable_work_and_fails_provider_work_without_replay() -> TestResult {
    let directory = tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("db"))).await?,
    );
    let local = Arc::new(DurableEffectHandler::new(
        "test.fault.shutdown.local",
        JobLane::Local,
        true,
        directory.path().join("shutdown-local.marker"),
        PausePoint::AfterFirstCheckpoint,
    ));
    let provider = Arc::new(DurableEffectHandler::new(
        "test.fault.shutdown.provider",
        JobLane::Provider,
        false,
        directory.path().join("shutdown-provider.marker"),
        PausePoint::AfterFirstCheckpoint,
    ));
    let handlers: Vec<Arc<dyn JobHandler>> = vec![
        Arc::clone(&local) as Arc<dyn JobHandler>,
        Arc::clone(&provider) as Arc<dyn JobHandler>,
    ];

    let coordinator = start_job_coordinator(storage.clone(), handlers.clone()).await?;
    let local_job = coordinator
        .service
        .enqueue("test.fault.shutdown.local", json!({}))
        .await?;
    let provider_job = coordinator
        .service
        .enqueue("test.fault.shutdown.provider", json!({}))
        .await?;
    wait_for_job(&storage, &local_job.id, |job| {
        job.status == JobStatus::Running && job.result.is_some()
    })
    .await?;
    wait_for_job(&storage, &provider_job.id, |job| {
        job.status == JobStatus::Running && job.result.is_some()
    })
    .await?;
    coordinator.service.shutdown();
    await_lane(coordinator.local_task).await??;
    await_lane(coordinator.provider_task).await??;

    let queued = storage
        .get(&local_job.id)
        .await?
        .ok_or("shutdown local job missing")?;
    let failed = storage
        .get(&provider_job.id)
        .await?
        .ok_or("shutdown provider job missing")?;
    assert_eq!(queued.status, JobStatus::Queued);
    assert_eq!(failed.status, JobStatus::Failed);
    assert_eq!(
        failed.error.as_deref(),
        Some("Job was interrupted during server shutdown.")
    );
    assert_eq!(local.effect_count(), 1);
    assert_eq!(provider.effect_count(), 1);

    let resumed = start_job_coordinator(storage.clone(), handlers).await?;
    let finished = wait_for_job(&storage, &local_job.id, |job| {
        job.status == JobStatus::Succeeded
    })
    .await?;
    sleep(Duration::from_millis(50)).await;
    assert_eq!(finished.attempts, 2);
    assert_eq!(local.attempt_count(), 2);
    assert_eq!(local.effect_count(), 1);
    assert_eq!(provider.attempt_count(), 1);
    assert_eq!(provider.effect_count(), 1);
    assert_eq!(
        storage
            .get(&provider_job.id)
            .await?
            .ok_or("provider job missing")?
            .status,
        JobStatus::Failed
    );
    stop_coordinator(resumed).await
}

#[tokio::test]
async fn cooperative_cancellation_is_terminal_and_preserves_the_checkpoint() -> TestResult {
    let directory = tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("db"))).await?,
    );
    let handler = Arc::new(ReleasableHandler::new("test.fault.cancel"));
    storage
        .create(&new_job("cancel", handler.definition()))
        .await?;
    let coordinator = start_job_coordinator(
        storage.clone(),
        one_handler(Arc::clone(&handler) as Arc<dyn JobHandler>),
    )
    .await?;
    wait_for_job(&storage, "cancel", |job| {
        job.status == JobStatus::Running && job.result.is_some()
    })
    .await?;
    let requested = coordinator.service.cancel("cancel").await?;
    assert_eq!(requested.status, JobStatus::CancelRequested);
    handler.release();
    let cancelled =
        wait_for_job(&storage, "cancel", |job| job.status == JobStatus::Cancelled).await?;
    assert_eq!(cancelled.attempts, 1);
    assert_eq!(handler.effect_count(), 1);
    assert_eq!(
        cancelled
            .result
            .as_ref()
            .and_then(|result| result.get("effect_applied")),
        Some(&Value::Bool(true))
    );
    stop_coordinator(coordinator).await?;

    let resumed = start_job_coordinator(
        storage.clone(),
        one_handler(Arc::clone(&handler) as Arc<dyn JobHandler>),
    )
    .await?;
    sleep(Duration::from_millis(50)).await;
    assert_eq!(handler.effect_count(), 1);
    assert_eq!(
        storage
            .get("cancel")
            .await?
            .ok_or("cancel missing")?
            .attempts,
        1
    );
    stop_coordinator(resumed).await
}

#[tokio::test]
async fn concurrent_claimers_obtain_exactly_one_sqlite_lease() -> TestResult {
    let directory = tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("db"))).await?,
    );
    storage
        .create(&new_job(
            "contended",
            definition("test.fault.contention", JobLane::Local, true),
        ))
        .await?;

    let mut claimers = JoinSet::new();
    for _ in 0..16 {
        let repository = Arc::clone(&storage);
        claimers.spawn(async move { repository.claim_next(JobLane::Local).await });
    }
    let mut claims = Vec::new();
    while let Some(joined) = claimers.join_next().await {
        if let Some(claim) = joined?? {
            claims.push(claim);
        }
    }

    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].job.id, "contended");
    let stored = storage
        .get("contended")
        .await?
        .ok_or("contended job missing")?;
    assert_eq!(stored.status, JobStatus::Running);
    assert_eq!(stored.attempts, 1);
    assert_eq!(stored.execution_id, Some(claims[0].execution_id.clone()));
    Ok(())
}
