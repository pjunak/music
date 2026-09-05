use super::*;
use music_application::assistant::{
    ModelReviewDestination, ModelRunManifest, ModelTaskError, ModelTransportFuture,
    ProviderAttemptOutcome, ProviderExecutionTarget, ProviderSecret, ProviderUsageAccumulator,
    ResolvedRoleExecution, StructuredModelRequest, StructuredModelResult, StructuredModelTransport,
    ThinkingMode, execute_recorded_provider_request,
};

const KIND: &str = "test.provider-attempt";

#[derive(Debug, Default)]
struct Transport {
    calls: AtomicUsize,
    entered: Notify,
    release: Notify,
    wait: bool,
    reject: bool,
}

impl StructuredModelTransport for Transport {
    fn validate_request(
        &self,
        _: &ProviderExecutionTarget,
        _: &StructuredModelRequest,
    ) -> Result<(), ModelTaskError> {
        if self.reject {
            Err(ModelTaskError::new("request_too_large"))
        } else {
            Ok(())
        }
    }

    fn execute_structured_model_request<'a>(
        &'a self,
        _: &'a ProviderExecutionTarget,
        _: &'a StructuredModelRequest,
    ) -> ModelTransportFuture<'a> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            if self.wait {
                self.release.notified().await;
            }
            StructuredModelResult {
                outcome: ProviderAttemptOutcome::ResponseReceived,
                succeeded: true,
                error_code: None,
                payload: Some(json!({"accepted": true})),
                provider_model_id: Some(format!("reported-{call}")),
                finish_reason: None,
                input_tokens: call.is_multiple_of(2).then_some(10),
                output_tokens: (!call.is_multiple_of(2)).then_some(5),
            }
        })
    }
}

#[derive(Debug)]
struct Handler {
    transport: Arc<Transport>,
    max_attempts: usize,
    requests: usize,
    change_role: bool,
    change_output_limit: bool,
}

impl Handler {
    fn new(transport: Transport) -> Arc<Self> {
        Arc::new(Self {
            transport: Arc::new(transport),
            max_attempts: 1,
            requests: 1,
            change_role: false,
            change_output_limit: false,
        })
    }
}

impl JobHandler for Handler {
    fn definition(&self) -> JobDefinition {
        definition(KIND, JobLane::Provider, false)
    }

    fn execute<'a>(
        &'a self,
        context: &'a music_application::jobs::JobExecutionContext,
        _: Map<String, Value>,
    ) -> JobHandlerFuture<'a> {
        Box::pin(async move {
            let mut role = ResolvedRoleExecution {
                role_id: "eq_assistant".to_owned(),
                fingerprint: "a".repeat(64),
                role_configuration_fingerprint: "b".repeat(64),
                connection_fingerprint: "c".repeat(64),
                connection_name: "private connection name".to_owned(),
                execution: ProviderExecutionTarget {
                    adapter_id: "openai-compatible".to_owned(),
                    base_url: "https://private.invalid".to_owned(),
                    api_key: ProviderSecret::new("private-credential"),
                    allow_private_network: false,
                    model_id: "chosen-model".to_owned(),
                    timeout_seconds: 30,
                    max_output_tokens: 100,
                    thinking_mode: ThinkingMode::Enabled,
                },
            };
            let manifest = ModelRunManifest::new(
                context,
                &role,
                "test-quality",
                Some("test-disclosure"),
                &json!({"scope": "private-scope"}),
                &json!({"evidence": "private-evidence"}),
                self.max_attempts,
                ModelReviewDestination::EqAuthoring,
            )?;
            let mut usage = ProviderUsageAccumulator::for_run(manifest);
            if self.change_role {
                role.connection_fingerprint = "d".repeat(64);
            }
            if self.change_output_limit {
                role.execution.max_output_tokens = 200;
            }
            for index in 0..self.requests {
                execute_recorded_provider_request(
                    context,
                    self.transport.as_ref(),
                    &role,
                    &StructuredModelRequest {
                        system_prompt: "private-system-prompt".to_owned(),
                        user_prompt: format!("private-user-prompt-{index}"),
                        max_output_tokens: 200,
                        output_schema_name: None,
                        output_schema: None,
                    },
                    &mut usage,
                )
                .await?;
            }
            Ok(Value::Object(usage.checkpoint()))
        })
    }
}

async fn setup(storage: &Arc<SqliteStorage>, handler: &Arc<Handler>) -> TestResult {
    storage
        .create(&new_job("attempt", handler.definition()))
        .await?;
    Ok(())
}

fn usage(job: &JobRecord) -> TestResult<&Value> {
    job.result
        .as_ref()
        .and_then(|result| result.get("usage"))
        .ok_or_else(|| "usage missing".into())
}

#[tokio::test]
async fn shutdown_preserves_write_ahead_attempt_and_never_replays() -> TestResult {
    let directory = tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("db"))).await?,
    );
    let handler = Handler::new(Transport {
        wait: true,
        ..Transport::default()
    });
    setup(&storage, &handler).await?;
    sqlx::query("UPDATE background_jobs SET created_at = datetime(CURRENT_TIMESTAMP, '-12 seconds') WHERE id = 'attempt'")
        .execute(&storage.pool).await?;
    let coordinator = start_job_coordinator(storage.clone(), one_handler(handler.clone())).await?;
    timeout(TEST_TIMEOUT, handler.transport.entered.notified()).await?;
    let running = storage.get("attempt").await?.ok_or("job missing")?;
    let recorded = usage(&running)?;
    assert_eq!(recorded["attempted_requests"], 1);
    assert_eq!(recorded["uncertain_requests"], 1);
    assert_eq!(recorded["attempts"][0]["elapsed_ms"], Value::Null);
    assert!(
        recorded["run_manifest"]["queue_wait_seconds"]
            .as_u64()
            .is_some_and(|value| value >= 12)
    );
    stop_coordinator(coordinator).await?;
    let failed = storage.get("attempt").await?.ok_or("job missing")?;
    assert_eq!(failed.status, JobStatus::Failed);
    assert_eq!(usage(&failed)?, recorded);
    let resumed = start_job_coordinator(storage.clone(), one_handler(handler.clone())).await?;
    assert_eq!(
        storage.get("attempt").await?.ok_or("job missing")?.status,
        JobStatus::Failed
    );
    assert_eq!(handler.transport.calls.load(Ordering::SeqCst), 1);
    stop_coordinator(resumed).await
}

#[tokio::test]
async fn checkpoint_failure_prevents_io_and_completion_failure_preserves_uncertainty() -> TestResult
{
    for point in [
        FaultPoint::BeforeCheckpoint,
        FaultPoint::Checkpoint,
        FaultPoint::AttemptCompletion,
    ] {
        let directory = tempdir()?;
        let storage = Arc::new(
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("db"))).await?,
        );
        let handler = Handler::new(Transport::default());
        setup(&storage, &handler).await?;
        let fault = Arc::new(FaultInjectingRepository::new(storage.clone(), point));
        let coordinator = start_job_coordinator(fault, one_handler(handler.clone())).await?;
        assert_eq!(
            await_lane(coordinator.provider_task).await?,
            Err(JobCoordinatorError::Dependency)
        );
        stop_other_lane(&coordinator.service, coordinator.local_task).await?;
        let resumed = start_job_coordinator(storage.clone(), one_handler(handler.clone())).await?;
        let failed = storage.get("attempt").await?.ok_or("job missing")?;
        assert_eq!(failed.status, JobStatus::Failed);
        assert_eq!(
            handler.transport.calls.load(Ordering::SeqCst),
            usize::from(point == FaultPoint::AttemptCompletion)
        );
        if point == FaultPoint::BeforeCheckpoint {
            assert!(failed.result.is_none());
        } else {
            assert_eq!(usage(&failed)?["uncertain_requests"], 1);
        }
        stop_coordinator(resumed).await?;
    }
    Ok(())
}

#[tokio::test]
async fn cancellation_preserves_response_facts_without_replaying() -> TestResult {
    let directory = tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("db"))).await?,
    );
    let handler = Handler::new(Transport {
        wait: true,
        ..Transport::default()
    });
    setup(&storage, &handler).await?;
    let coordinator = start_job_coordinator(storage.clone(), one_handler(handler.clone())).await?;
    timeout(TEST_TIMEOUT, handler.transport.entered.notified()).await?;
    coordinator.service.cancel("attempt").await?;
    handler.transport.release.notify_one();
    let cancelled = wait_for_job(&storage, "attempt", |job| {
        job.status == JobStatus::Cancelled
    })
    .await?;
    assert_eq!(usage(&cancelled)?["response_received_requests"], 1);
    assert_eq!(usage(&cancelled)?["uncertain_requests"], 0);
    assert_eq!(usage(&cancelled)?["input_tokens"], 10);
    assert_eq!(handler.transport.calls.load(Ordering::SeqCst), 1);
    stop_coordinator(coordinator).await
}

#[tokio::test]
async fn budgets_bound_requests_and_records_without_losing_aggregate_usage() -> TestResult {
    let directory = tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("db"))).await?,
    );
    let handler = Arc::new(Handler {
        transport: Arc::new(Transport::default()),
        max_attempts: 129,
        requests: 130,
        change_role: false,
        change_output_limit: false,
    });
    setup(&storage, &handler).await?;
    let coordinator = start_job_coordinator(storage.clone(), one_handler(handler.clone())).await?;
    let failed = wait_for_job(&storage, "attempt", |job| job.status == JobStatus::Failed).await?;
    assert_eq!(
        failed.error.as_deref(),
        Some("model_run_request_budget_exhausted")
    );
    let recorded = usage(&failed)?;
    assert_eq!(handler.transport.calls.load(Ordering::SeqCst), 129);
    assert_eq!(recorded["attempted_requests"], 129);
    assert_eq!(recorded["response_received_requests"], 129);
    assert_eq!(recorded["responses_missing_usage"], 129);
    assert_eq!(recorded["input_tokens"], 650);
    assert_eq!(recorded["output_tokens"], 320);
    assert_eq!(
        recorded["provider_model_ids"]
            .as_array()
            .ok_or("model IDs missing")?
            .len(),
        8
    );
    assert_eq!(recorded["provider_model_ids_truncated"], true);
    assert_eq!(
        recorded["attempts"]
            .as_array()
            .ok_or("attempts missing")?
            .len(),
        128
    );
    assert_eq!(recorded["attempts_truncated"], true);
    assert_eq!(recorded["attempts"][0]["sequence"], 2);
    assert_eq!(recorded["attempts"][127]["sequence"], 129);
    assert_eq!(recorded["attempts"][127]["max_output_tokens"], 100);
    assert_ne!(
        recorded["attempts"][0]["request_fingerprint"],
        recorded["attempts"][1]["request_fingerprint"]
    );
    assert_eq!(recorded["run_manifest"]["output_token_ceiling"], 12_900);
    assert_eq!(recorded["run_manifest"]["thinking_mode"], "enabled");
    assert_eq!(
        recorded["run_manifest"]["scope_fingerprint"]
            .as_str()
            .ok_or("scope missing")?
            .len(),
        64
    );
    assert!(!serde_json::to_string(recorded)?.contains("private-"));
    stop_coordinator(coordinator).await
}

#[tokio::test]
async fn preflight_rejection_and_changed_role_never_send() -> TestResult {
    for (change_role, change_output_limit) in [(false, false), (true, false), (false, true)] {
        let directory = tempdir()?;
        let storage = Arc::new(
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("db"))).await?,
        );
        let handler = Arc::new(Handler {
            transport: Arc::new(Transport {
                reject: true,
                ..Transport::default()
            }),
            max_attempts: 1,
            requests: 1,
            change_role,
            change_output_limit,
        });
        setup(&storage, &handler).await?;
        let coordinator =
            start_job_coordinator(storage.clone(), one_handler(handler.clone())).await?;
        let finished = wait_for_job(&storage, "attempt", |job| {
            matches!(job.status, JobStatus::Failed | JobStatus::Succeeded)
        })
        .await?;
        assert_eq!(handler.transport.calls.load(Ordering::SeqCst), 0);
        if change_role || change_output_limit {
            assert_eq!(finished.error.as_deref(), Some("role_changed"));
        } else {
            assert_eq!(usage(&finished)?["preflight_rejected_requests"], 1);
            assert_eq!(usage(&finished)?["uncertain_requests"], 0);
            assert_eq!(usage(&finished)?["responses_missing_usage"], 0);
        }
        stop_coordinator(coordinator).await?;
    }
    Ok(())
}
