use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(5);
const MAX_JOB_KIND_BYTES: usize = 128;
const MAX_PROGRESS_PHASE_BYTES: usize = 128;
const MAX_PROGRESS_MESSAGE_BYTES: usize = 512;
const MAX_ERROR_BYTES: usize = 2_000;

pub type JobDependencyError = Box<dyn Error + Send + Sync>;
pub type JobFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, JobDependencyError>> + Send + 'a>>;
pub type JobHandlerFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Value, JobHandlerError>> + Send + 'a>>;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    CancelRequested,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::CancelRequested => "cancel_requested",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Result<Self, JobValidationError> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "cancel_requested" => Ok(Self::CancelRequested),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(JobValidationError::InvalidStatus),
        }
    }

    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::CancelRequested)
    }

    #[must_use]
    pub const fn can_retry(self) -> bool {
        matches!(self, Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobLane {
    Local,
    Provider,
}

impl JobLane {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Provider => "provider",
        }
    }

    pub fn parse(value: &str) -> Result<Self, JobValidationError> {
        match value {
            "local" => Ok(Self::Local),
            "provider" => Ok(Self::Provider),
            _ => Err(JobValidationError::InvalidLane),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobCheckpointPolicy {
    Replace,
}

impl JobCheckpointPolicy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Replace => "replace",
        }
    }

    pub fn parse(value: &str) -> Result<Self, JobValidationError> {
        match value {
            "replace" => Ok(Self::Replace),
            _ => Err(JobValidationError::InvalidCheckpointPolicy),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct JobDefinition {
    pub kind: &'static str,
    pub schema_version: u32,
    pub lane: JobLane,
    pub restartable: bool,
    pub checkpoint_policy: JobCheckpointPolicy,
}

impl JobDefinition {
    pub fn validate(&self) -> Result<(), JobValidationError> {
        if self.kind.is_empty() || self.kind.len() > MAX_JOB_KIND_BYTES {
            return Err(JobValidationError::InvalidKind);
        }
        if self.schema_version == 0 {
            return Err(JobValidationError::InvalidSchemaVersion);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobRecord {
    pub id: String,
    pub kind: String,
    pub status: JobStatus,
    pub parameters: Map<String, Value>,
    pub result: Option<Map<String, Value>>,
    pub error: Option<String>,
    pub progress_current: u64,
    pub progress_total: Option<u64>,
    pub progress_phase: String,
    pub progress_message: String,
    pub attempts: u32,
    pub retry_of_id: Option<String>,
    pub created_at_unix_seconds: i64,
    pub updated_at_unix_seconds: i64,
    pub started_at_unix_seconds: Option<i64>,
    pub finished_at_unix_seconds: Option<i64>,
    pub lane: JobLane,
    pub schema_version: u32,
    pub restartable: bool,
    pub checkpoint_policy: JobCheckpointPolicy,
    pub execution_id: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct JobListFilter {
    pub kind: Option<String>,
    pub status: Option<JobStatus>,
    pub limit: u16,
}

impl Default for JobListFilter {
    fn default() -> Self {
        Self {
            kind: None,
            status: None,
            limit: 25,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewJob {
    pub id: String,
    pub definition: JobDefinition,
    pub parameters: Map<String, Value>,
    pub retry_of_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobClaim {
    pub job: JobRecord,
    pub execution_id: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum JobLeaseState {
    Active,
    CancellationRequested,
    Lost,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum JobValidationError {
    InvalidKind,
    InvalidStatus,
    InvalidLane,
    InvalidCheckpointPolicy,
    InvalidSchemaVersion,
    InvalidParameters,
    InvalidResult,
    InvalidProgress,
    DuplicateKind,
}

impl Display for JobValidationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidKind => "job kind must contain between 1 and 128 bytes",
            Self::InvalidStatus => "stored job status is invalid",
            Self::InvalidLane => "stored job lane is invalid",
            Self::InvalidCheckpointPolicy => "stored job checkpoint policy is invalid",
            Self::InvalidSchemaVersion => "job schema version must be positive",
            Self::InvalidParameters => "job parameters must be a JSON object",
            Self::InvalidResult => "job result must be a JSON object",
            Self::InvalidProgress => "job progress must satisfy 0 <= current <= total",
            Self::DuplicateKind => "job kind is registered more than once",
        })
    }
}

impl Error for JobValidationError {}

pub trait JobRepository: std::fmt::Debug + Send + Sync {
    fn create<'a>(&'a self, job: &'a NewJob) -> JobFuture<'a, JobRecord>;
    fn create_unique_active<'a>(&'a self, job: &'a NewJob) -> JobFuture<'a, (JobRecord, bool)>;
    fn get<'a>(&'a self, id: &'a str) -> JobFuture<'a, Option<JobRecord>>;
    fn list<'a>(&'a self, filter: &'a JobListFilter) -> JobFuture<'a, Vec<JobRecord>>;
    fn request_cancellation<'a>(&'a self, id: &'a str) -> JobFuture<'a, Option<(JobRecord, bool)>>;
    fn claim_next<'a>(&'a self, lane: JobLane) -> JobFuture<'a, Option<JobClaim>>;
    fn lease_state<'a>(
        &'a self,
        id: &'a str,
        execution_id: &'a str,
    ) -> JobFuture<'a, JobLeaseState>;
    fn update_progress<'a>(
        &'a self,
        claim: &'a JobClaim,
        progress: &'a JobProgress,
    ) -> JobFuture<'a, JobLeaseState>;
    fn checkpoint<'a>(
        &'a self,
        claim: &'a JobClaim,
        result: &'a Map<String, Value>,
    ) -> JobFuture<'a, JobLeaseState>;
    fn finish<'a>(
        &'a self,
        claim: &'a JobClaim,
        finish: &'a JobFinish,
    ) -> JobFuture<'a, JobLeaseState>;
    fn recover_interrupted<'a>(
        &'a self,
        definitions: &'a BTreeMap<String, JobDefinition>,
    ) -> JobFuture<'a, usize>;
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct JobProgress {
    pub current: u64,
    pub total: Option<u64>,
    pub phase: String,
    pub message: String,
}

impl JobProgress {
    pub fn new(
        current: u64,
        total: Option<u64>,
        phase: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<Self, JobValidationError> {
        if total.is_some_and(|total| current > total) {
            return Err(JobValidationError::InvalidProgress);
        }
        Ok(Self {
            current,
            total,
            phase: truncate_utf8(phase.into(), MAX_PROGRESS_PHASE_BYTES),
            message: truncate_utf8(message.into(), MAX_PROGRESS_MESSAGE_BYTES),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum JobFinish {
    Succeeded(Map<String, Value>),
    Failed(String),
    Cancelled,
    Interrupted { restartable: bool },
}

pub trait JobHandler: std::fmt::Debug + Send + Sync {
    fn definition(&self) -> JobDefinition;
    fn execute<'a>(
        &'a self,
        context: &'a JobExecutionContext,
        parameters: Map<String, Value>,
    ) -> JobHandlerFuture<'a>;
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct JobHandlerError {
    detail: String,
    kind: JobHandlerErrorKind,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum JobHandlerErrorKind {
    Failed,
    Cancelled,
    Stopping,
    LeaseLost,
    Dependency,
}

impl JobHandlerError {
    #[must_use]
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: truncate_utf8(detail.into(), MAX_ERROR_BYTES),
            kind: JobHandlerErrorKind::Failed,
        }
    }

    #[must_use]
    pub fn from_execution(error: JobExecutionError) -> Self {
        let kind = match error {
            JobExecutionError::Cancelled => JobHandlerErrorKind::Cancelled,
            JobExecutionError::Stopping => JobHandlerErrorKind::Stopping,
            JobExecutionError::LeaseLost => JobHandlerErrorKind::LeaseLost,
            JobExecutionError::Dependency => JobHandlerErrorKind::Dependency,
        };
        Self {
            detail: error.to_string(),
            kind,
        }
    }

    #[must_use]
    pub fn into_detail(self) -> String {
        self.detail
    }
}

impl Display for JobHandlerError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl Error for JobHandlerError {}

#[derive(Debug)]
pub struct JobRegistry {
    definitions: BTreeMap<String, JobDefinition>,
    handlers: BTreeMap<String, Arc<dyn JobHandler>>,
}

impl JobRegistry {
    pub fn new(handlers: Vec<Arc<dyn JobHandler>>) -> Result<Self, JobValidationError> {
        let mut definitions = BTreeMap::new();
        let mut registered = BTreeMap::new();
        for handler in handlers {
            let definition = handler.definition();
            definition.validate()?;
            if definitions
                .insert(definition.kind.to_owned(), definition.clone())
                .is_some()
            {
                return Err(JobValidationError::DuplicateKind);
            }
            registered.insert(definition.kind.to_owned(), handler);
        }
        Ok(Self {
            definitions,
            handlers: registered,
        })
    }

    #[must_use]
    pub fn definition(&self, kind: &str) -> Option<&JobDefinition> {
        self.definitions.get(kind)
    }

    #[must_use]
    pub fn definitions(&self) -> &BTreeMap<String, JobDefinition> {
        &self.definitions
    }

    fn handler(&self, kind: &str) -> Option<&Arc<dyn JobHandler>> {
        self.handlers.get(kind)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum JobServiceError {
    UnknownKind,
    JobNotFound,
    NotRetryable,
    AlreadyTerminal(JobStatus),
    InvalidParameters,
    Dependency,
}

impl Display for JobServiceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnknownKind => "the job type is not registered",
            Self::JobNotFound => "job not found",
            Self::NotRetryable => "only failed or cancelled jobs can be retried",
            Self::AlreadyTerminal(_) => "job is already in a terminal state",
            Self::InvalidParameters => "job parameters must be a JSON object",
            Self::Dependency => "job storage is unavailable",
        })
    }
}

impl Error for JobServiceError {}

#[derive(Debug)]
pub struct JobService {
    repository: Arc<dyn JobRepository>,
    registry: Arc<JobRegistry>,
    local_wake: Notify,
    provider_wake: Notify,
    shutdown: CancellationToken,
}

impl JobService {
    fn new(repository: Arc<dyn JobRepository>, registry: Arc<JobRegistry>) -> Self {
        Self {
            repository,
            registry,
            local_wake: Notify::new(),
            provider_wake: Notify::new(),
            shutdown: CancellationToken::new(),
        }
    }

    #[must_use]
    pub fn registry(&self) -> &JobRegistry {
        &self.registry
    }

    pub async fn enqueue(
        &self,
        kind: &str,
        parameters: Value,
    ) -> Result<JobRecord, JobServiceError> {
        self.enqueue_with_retry(kind, parameters, None).await
    }

    pub async fn enqueue_unique_active(
        &self,
        kind: &str,
        parameters: Value,
    ) -> Result<(JobRecord, bool), JobServiceError> {
        let job = self.new_job(kind, parameters, None)?;
        let result = self
            .repository
            .create_unique_active(&job)
            .await
            .map_err(|_| JobServiceError::Dependency)?;
        if result.1 {
            self.wake(job.definition.lane);
        }
        Ok(result)
    }

    pub async fn get(&self, id: &str) -> Result<Option<JobRecord>, JobServiceError> {
        self.repository
            .get(id)
            .await
            .map_err(|_| JobServiceError::Dependency)
    }

    pub async fn list(&self, filter: &JobListFilter) -> Result<Vec<JobRecord>, JobServiceError> {
        self.repository
            .list(filter)
            .await
            .map_err(|_| JobServiceError::Dependency)
    }

    pub async fn cancel(&self, id: &str) -> Result<JobRecord, JobServiceError> {
        let Some((job, changed)) = self
            .repository
            .request_cancellation(id)
            .await
            .map_err(|_| JobServiceError::Dependency)?
        else {
            return Err(JobServiceError::JobNotFound);
        };
        if !changed {
            return Err(JobServiceError::AlreadyTerminal(job.status));
        }
        self.wake(job.lane);
        Ok(job)
    }

    pub async fn retry(&self, id: &str) -> Result<JobRecord, JobServiceError> {
        let previous = self.get(id).await?.ok_or(JobServiceError::JobNotFound)?;
        if !previous.status.can_retry() {
            return Err(JobServiceError::NotRetryable);
        }
        if self.registry.definition(&previous.kind).is_none() {
            return Err(JobServiceError::UnknownKind);
        }
        self.enqueue_with_retry(
            &previous.kind,
            Value::Object(previous.parameters),
            Some(previous.id),
        )
        .await
    }

    pub fn shutdown(&self) {
        self.shutdown.cancel();
        self.local_wake.notify_waiters();
        self.provider_wake.notify_waiters();
    }

    fn new_job(
        &self,
        kind: &str,
        parameters: Value,
        retry_of_id: Option<String>,
    ) -> Result<NewJob, JobServiceError> {
        let definition = self
            .registry
            .definition(kind)
            .cloned()
            .ok_or(JobServiceError::UnknownKind)?;
        let Value::Object(parameters) = parameters else {
            return Err(JobServiceError::InvalidParameters);
        };
        Ok(NewJob {
            id: Uuid::new_v4().simple().to_string(),
            definition,
            parameters,
            retry_of_id,
        })
    }

    async fn enqueue_with_retry(
        &self,
        kind: &str,
        parameters: Value,
        retry_of_id: Option<String>,
    ) -> Result<JobRecord, JobServiceError> {
        let job = self.new_job(kind, parameters, retry_of_id)?;
        let record = self
            .repository
            .create(&job)
            .await
            .map_err(|_| JobServiceError::Dependency)?;
        self.wake(job.definition.lane);
        Ok(record)
    }

    fn wake(&self, lane: JobLane) {
        match lane {
            JobLane::Local => self.local_wake.notify_one(),
            JobLane::Provider => self.provider_wake.notify_one(),
        }
    }

    fn notification(&self, lane: JobLane) -> &Notify {
        match lane {
            JobLane::Local => &self.local_wake,
            JobLane::Provider => &self.provider_wake,
        }
    }
}

#[derive(Debug)]
pub struct JobExecutionContext {
    repository: Arc<dyn JobRepository>,
    claim: JobClaim,
    shutdown: CancellationToken,
}

impl JobExecutionContext {
    #[must_use]
    pub fn job_id(&self) -> &str {
        &self.claim.job.id
    }

    /// Persisted timestamps have one-second resolution. Clock reversal is unknown.
    #[must_use]
    pub fn queue_wait_seconds(&self) -> Option<u64> {
        self.claim
            .job
            .started_at_unix_seconds
            .and_then(|started| started.checked_sub(self.claim.job.created_at_unix_seconds))
            .and_then(|elapsed| u64::try_from(elapsed).ok())
    }

    #[must_use]
    pub const fn progress_current(&self) -> u64 {
        self.claim.job.progress_current
    }

    #[must_use]
    pub const fn progress_total(&self) -> Option<u64> {
        self.claim.job.progress_total
    }

    pub async fn update_progress(&self, progress: JobProgress) -> Result<(), JobExecutionError> {
        self.ensure_active(
            self.repository
                .update_progress(&self.claim, &progress)
                .await
                .map_err(|_| JobExecutionError::Dependency)?,
        )
    }

    pub async fn checkpoint(&self, result: Map<String, Value>) -> Result<(), JobExecutionError> {
        self.ensure_active(
            self.repository
                .checkpoint(&self.claim, &result)
                .await
                .map_err(|_| JobExecutionError::Dependency)?,
        )
    }

    pub async fn check_cancelled(&self) -> Result<(), JobExecutionError> {
        if self.shutdown.is_cancelled() {
            return Err(JobExecutionError::Stopping);
        }
        self.ensure_active(
            self.repository
                .lease_state(&self.claim.job.id, &self.claim.execution_id)
                .await
                .map_err(|_| JobExecutionError::Dependency)?,
        )
    }

    pub async fn related_job(&self, id: &str) -> Result<Option<JobRecord>, JobExecutionError> {
        self.repository
            .get(id)
            .await
            .map_err(|_| JobExecutionError::Dependency)
    }

    fn ensure_active(&self, state: JobLeaseState) -> Result<(), JobExecutionError> {
        if self.shutdown.is_cancelled() {
            return Err(JobExecutionError::Stopping);
        }
        match state {
            JobLeaseState::Active => Ok(()),
            JobLeaseState::CancellationRequested => Err(JobExecutionError::Cancelled),
            JobLeaseState::Lost => Err(JobExecutionError::LeaseLost),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum JobExecutionError {
    Cancelled,
    Stopping,
    LeaseLost,
    Dependency,
}

impl Display for JobExecutionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Cancelled => "job cancellation was requested",
            Self::Stopping => "job runner is stopping",
            Self::LeaseLost => "job execution lease was lost",
            Self::Dependency => "job storage is unavailable",
        })
    }
}

impl Error for JobExecutionError {}

#[derive(Debug)]
pub struct SpawnedJobCoordinator {
    pub service: Arc<JobService>,
    pub local_task: JoinHandle<Result<(), JobCoordinatorError>>,
    pub provider_task: JoinHandle<Result<(), JobCoordinatorError>>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum JobCoordinatorError {
    Recovery,
    Dependency,
}

impl Display for JobCoordinatorError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Recovery => "background job recovery failed",
            Self::Dependency => "background job storage failed",
        })
    }
}

impl Error for JobCoordinatorError {}

pub async fn start_job_coordinator(
    repository: Arc<dyn JobRepository>,
    handlers: Vec<Arc<dyn JobHandler>>,
) -> Result<SpawnedJobCoordinator, JobCoordinatorError> {
    let registry = Arc::new(JobRegistry::new(handlers).map_err(|_| JobCoordinatorError::Recovery)?);
    repository
        .recover_interrupted(registry.definitions())
        .await
        .map_err(|_| JobCoordinatorError::Recovery)?;
    let service = Arc::new(JobService::new(repository, registry));
    let local_task = tokio::spawn(run_lane(Arc::clone(&service), JobLane::Local));
    let provider_task = tokio::spawn(run_lane(Arc::clone(&service), JobLane::Provider));
    Ok(SpawnedJobCoordinator {
        service,
        local_task,
        provider_task,
    })
}

async fn run_lane(service: Arc<JobService>, lane: JobLane) -> Result<(), JobCoordinatorError> {
    loop {
        if service.shutdown.is_cancelled() {
            return Ok(());
        }
        let claim = service
            .repository
            .claim_next(lane)
            .await
            .map_err(|_| JobCoordinatorError::Dependency)?;
        if let Some(claim) = claim {
            execute_claim(&service, claim).await?;
            continue;
        }
        tokio::select! {
            () = service.shutdown.cancelled() => return Ok(()),
            () = service.notification(lane).notified() => {},
            () = tokio::time::sleep(IDLE_POLL_INTERVAL) => {},
        }
    }
}

async fn execute_claim(
    service: &Arc<JobService>,
    claim: JobClaim,
) -> Result<(), JobCoordinatorError> {
    let Some(handler) = service.registry.handler(&claim.job.kind).cloned() else {
        let _ = service
            .repository
            .finish(
                &claim,
                &JobFinish::Failed("No handler is registered for this job type.".to_owned()),
            )
            .await
            .map_err(|_| JobCoordinatorError::Dependency)?;
        return Ok(());
    };
    let definition = handler.definition();
    if definition.schema_version != claim.job.schema_version
        || definition.lane != claim.job.lane
        || definition.restartable != claim.job.restartable
    {
        let _ = service
            .repository
            .finish(
                &claim,
                &JobFinish::Failed("Stored job contract is no longer available.".to_owned()),
            )
            .await
            .map_err(|_| JobCoordinatorError::Dependency)?;
        return Ok(());
    }
    let context = JobExecutionContext {
        repository: Arc::clone(&service.repository),
        claim: claim.clone(),
        shutdown: service.shutdown.clone(),
    };
    let execution = handler.execute(&context, claim.job.parameters.clone());
    let finish = tokio::select! {
        result = execution => match result {
            Ok(Value::Object(result)) => match context.check_cancelled().await {
                Ok(()) => JobFinish::Succeeded(result),
                Err(JobExecutionError::Cancelled) => JobFinish::Cancelled,
                Err(JobExecutionError::Stopping) => JobFinish::Interrupted {
                    restartable: definition.restartable,
                },
                Err(JobExecutionError::LeaseLost) => return Ok(()),
                Err(JobExecutionError::Dependency) => return Err(JobCoordinatorError::Dependency),
            },
            Ok(_) => JobFinish::Failed("Job handler returned a non-object result.".to_owned()),
            Err(error) => match error.kind {
                JobHandlerErrorKind::Failed => JobFinish::Failed(error.into_detail()),
                JobHandlerErrorKind::Cancelled => JobFinish::Cancelled,
                JobHandlerErrorKind::Stopping => JobFinish::Interrupted {
                    restartable: definition.restartable,
                },
                JobHandlerErrorKind::LeaseLost => return Ok(()),
                JobHandlerErrorKind::Dependency => {
                    return Err(JobCoordinatorError::Dependency);
                }
            },
        },
        () = service.shutdown.cancelled() => JobFinish::Interrupted {
            restartable: definition.restartable,
        },
    };
    let _ = service
        .repository
        .finish(&claim, &finish)
        .await
        .map_err(|_| JobCoordinatorError::Dependency)?;
    Ok(())
}

fn truncate_utf8(mut value: String, maximum_bytes: usize) -> String {
    if value.len() <= maximum_bytes {
        return value;
    }
    let mut boundary = maximum_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::Value;

    use super::{
        JobCheckpointPolicy, JobDefinition, JobHandler, JobHandlerError, JobHandlerFuture, JobLane,
        JobProgress, JobRegistry, JobValidationError,
    };

    #[derive(Debug)]
    struct TestHandler(&'static str);

    impl JobHandler for TestHandler {
        fn definition(&self) -> JobDefinition {
            JobDefinition {
                kind: self.0,
                schema_version: 1,
                lane: JobLane::Local,
                restartable: true,
                checkpoint_policy: JobCheckpointPolicy::Replace,
            }
        }

        fn execute<'a>(
            &'a self,
            _context: &'a super::JobExecutionContext,
            _parameters: serde_json::Map<String, Value>,
        ) -> JobHandlerFuture<'a> {
            Box::pin(async { Err(JobHandlerError::new("unused")) })
        }
    }

    #[test]
    fn registry_rejects_duplicate_kinds() {
        let result = JobRegistry::new(vec![
            Arc::new(TestHandler("test")),
            Arc::new(TestHandler("test")),
        ]);
        assert!(matches!(result, Err(JobValidationError::DuplicateKind)));
    }

    #[test]
    fn progress_is_bounded_and_unicode_safe() -> Result<(), Box<dyn std::error::Error>> {
        assert!(JobProgress::new(2, Some(1), "", "").is_err());
        let progress = JobProgress::new(1, Some(1), "x".repeat(200), "é".repeat(400))?;
        assert_eq!(progress.phase.len(), 128);
        assert!(progress.message.len() <= 512);
        assert!(progress.message.is_char_boundary(progress.message.len()));
        Ok(())
    }
}
