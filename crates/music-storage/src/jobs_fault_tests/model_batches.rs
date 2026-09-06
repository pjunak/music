use super::*;
use music_application::assistant::*;
use std::sync::Mutex;

#[derive(Debug)]
struct Credentials(Arc<crate::CredentialVault>);
impl ProviderCredentialSource for Credentials {
    fn current_cipher(&self) -> ProviderCredentialFuture<'_> {
        Box::pin(async { Ok(self.0.clone() as Arc<dyn ProviderCredentialCipher>) })
    }
}

#[derive(Debug)]
struct Policy;
impl ProviderConnectionPolicy for Policy {
    fn normalize_base_url(
        &self,
        _: &str,
        raw: &str,
        _: bool,
    ) -> Result<String, ProviderPolicyError> {
        Ok(raw.to_owned())
    }
}

#[derive(Debug, Default)]
struct Transport {
    state: Mutex<String>,
    status_calls: AtomicUsize,
    result_calls: AtomicUsize,
    cancel_calls: AtomicUsize,
    delete_calls: AtomicUsize,
    fail_delete: AtomicBool,
    return_partial_results: AtomicBool,
}

// Deliberate tripwires: any attempt to create new paid work fails the test.
#[allow(clippy::panic)]
impl StructuredModelTransport for Transport {
    fn validate_request(
        &self,
        _: &ProviderExecutionTarget,
        _: &StructuredModelRequest,
    ) -> Result<(), ModelTaskError> {
        Ok(())
    }
    fn execute_structured_model_request<'a>(
        &'a self,
        _: &'a ProviderExecutionTarget,
        _: &'a StructuredModelRequest,
    ) -> ModelTransportFuture<'a> {
        panic!("collection must never start new inference")
    }
}

#[allow(clippy::panic)]
impl ModelBatchTransport for Transport {
    fn validate(
        &self,
        _: &ProviderExecutionTarget,
        _: &[StructuredModelRequest],
    ) -> Result<(), ModelTaskError> {
        Ok(())
    }
    fn upload<'a>(
        &'a self,
        _: &'a ProviderExecutionTarget,
        _: &'a [StructuredModelRequest],
    ) -> BatchTransportFuture<'a, String> {
        panic!("collection must never upload")
    }
    fn submit<'a>(
        &'a self,
        _: &'a ProviderExecutionTarget,
        _: &'a str,
        _: &'a str,
    ) -> BatchTransportFuture<'a, String> {
        panic!("collection must never submit")
    }
    fn status<'a>(
        &'a self,
        _: &'a ProviderExecutionTarget,
        id: &'a str,
    ) -> BatchTransportFuture<'a, ProviderBatchStatus> {
        Box::pin(async move {
            self.status_calls.fetch_add(1, Ordering::SeqCst);
            Ok(ProviderBatchStatus {
                run_id: if id == "batch_wrong" {
                    "another-run"
                } else {
                    "paid-run"
                }
                .to_owned(),
                state: self
                    .state
                    .lock()
                    .map_err(|_| ModelTaskError::new("fixture_poisoned"))?
                    .clone(),
                input_file_id: "file-input".to_owned(),
                output_file_id: Some("file-output".to_owned()),
                error_file_id: None,
            })
        })
    }
    fn results<'a>(
        &'a self,
        _: &'a ProviderExecutionTarget,
        _: &'a str,
    ) -> BatchTransportFuture<'a, Vec<ProviderBatchResult>> {
        Box::pin(async {
            self.result_calls.fetch_add(1, Ordering::SeqCst);
            if self.return_partial_results.load(Ordering::SeqCst) {
                return Ok(vec![ProviderBatchResult {
                    custom_id: "request-1".to_owned(),
                    result: StructuredModelResult {
                        succeeded: true,
                        outcome: ProviderAttemptOutcome::ResponseReceived,
                        error_code: None,
                        payload: Some(
                            json!({"schema_version":MODEL_TAGGER_OUTPUT_CONTRACT,"tracks":[{"track_id":1,"tag_ids":[],"confidence":"low","evidence":["Insufficient evidence"]}]}),
                        ),
                        provider_model_id: Some("fixture".to_owned()),
                        finish_reason: Some("stop".to_owned()),
                        input_tokens: Some(100),
                        output_tokens: Some(10),
                        token_details: ModelTokenDetails {
                            cached_input_tokens: Some(80),
                            ..Default::default()
                        },
                    },
                }]);
            }
            Ok(Vec::new())
        })
    }
    fn cancel<'a>(
        &'a self,
        _: &'a ProviderExecutionTarget,
        _: &'a str,
    ) -> BatchTransportFuture<'a, ()> {
        Box::pin(async {
            self.cancel_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
    fn delete_file<'a>(
        &'a self,
        _: &'a ProviderExecutionTarget,
        _: &'a str,
    ) -> BatchTransportFuture<'a, ()> {
        Box::pin(async {
            self.delete_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_delete.load(Ordering::SeqCst) {
                Err(ModelTaskError::new("transport_error"))
            } else {
                Ok(())
            }
        })
    }
}

async fn fixture() -> TestResult<(
    tempfile::TempDir,
    Arc<SqliteStorage>,
    Arc<Transport>,
    Vec<Arc<dyn JobHandler>>,
)> {
    let directory = tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("db"))).await?,
    );
    let vault = Arc::new(crate::CredentialVault::from_key([7; 32])?);
    let encrypted = vault.encrypt("fixture", "synthetic-test-credential")?;
    sqlx::query("INSERT INTO assistant_provider_connections (id,name,adapter_id,base_url,encrypted_api_key,api_key_nonce,api_key_hint,allow_private_network,verification_status,verified_models_json,verified_capabilities_json,created_at,updated_at) VALUES ('fixture','Fixture','openai-responses/v1','https://api.openai.com/v1',?,?,?,0,'never','[]','[]',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)")
        .bind(encrypted.ciphertext).bind(encrypted.nonce).bind(encrypted.hint).execute(&storage.pool).await?;
    let providers = Arc::new(ProviderService::new(
        storage.clone(),
        Arc::new(Credentials(vault)),
        Arc::new(Policy),
        "new-certification-digest".to_owned(),
    ));
    let transport = Arc::new(Transport::default());
    *transport.state.lock().map_err(|_| "fixture poisoned")? = "in_progress".to_owned();
    let handlers = model_feature_job_handlers(
        Arc::new(ModelQualityService::new(storage.clone(), providers.clone())),
        transport.clone(),
        Arc::new(AssistantService::new(storage.clone())),
        Arc::new(LocalAnalysisService::new(storage.clone())),
        storage.clone(),
        Some(Arc::new(ModelBatchServices {
            repository: storage.clone(),
            transport: transport.clone(),
            providers,
        })),
    );
    Ok((directory, storage, transport, handlers))
}

async fn action(
    storage: &SqliteStorage,
    coordinator: &SpawnedJobCoordinator,
    handlers: &[Arc<dyn JobHandler>],
    _id: &str,
    extra: Value,
) -> TestResult<JobRecord> {
    let handler = handlers
        .iter()
        .find(|handler| handler.definition().kind == MODEL_TAGGING_BATCH_COLLECT_JOB_KIND)
        .ok_or("collector missing")?;
    let mut job = new_job("fixture", handler.definition());
    job.parameters = json!({"batch_id":"paid-run","role_id":"music_tagger"})
        .as_object()
        .ok_or("missing batch fixture value")?
        .clone();
    job.parameters.extend(
        extra
            .as_object()
            .ok_or("missing batch fixture value")?
            .clone(),
    );
    let queued = coordinator
        .service
        .enqueue(handler.definition().kind, Value::Object(job.parameters))
        .await?;
    wait_for_job(storage, &queued.id, |job| {
        matches!(job.status, JobStatus::Failed | JobStatus::Succeeded)
    })
    .await
}

#[tokio::test]
async fn uncertain_batches_require_owned_recovery_and_cleanup_survives_restart() -> TestResult {
    let (_directory, storage, transport, handlers) = fixture().await?;
    let mut record = ModelBatchRecord {
        id: "paid-run".to_owned(),
        connection_id: "fixture".to_owned(),
        state: "submitting".to_owned(),
        input_file_id: None,
        remote_batch_id: None,
        document: json!({}),
    };
    assert!(storage.create_model_batch(&record).await?);
    record.input_file_id = Some("file-input".to_owned());
    assert!(storage.update_model_batch("submitting", &record).await?);
    let coordinator = start_job_coordinator(storage.clone(), handlers.clone()).await?;
    let uncertain = action(&storage, &coordinator, &handlers, "uncertain", json!({})).await?;
    assert_eq!(
        uncertain.error.as_deref(),
        Some("batch_submission_uncertain_recover_id")
    );
    assert_eq!(transport.status_calls.load(Ordering::SeqCst), 0);
    let wrong = action(
        &storage,
        &coordinator,
        &handlers,
        "wrong",
        json!({"remote_batch_id":"batch_wrong"}),
    )
    .await?;
    assert_eq!(wrong.error.as_deref(), Some("batch_identity_mismatch"));
    assert!(
        storage
            .model_batch("paid-run")
            .await?
            .ok_or("missing batch fixture value")?
            .remote_batch_id
            .is_none()
    );
    assert_eq!(
        action(
            &storage,
            &coordinator,
            &handlers,
            "recover",
            json!({"remote_batch_id":"batch_owned"})
        )
        .await?
        .status,
        JobStatus::Succeeded
    );
    assert_eq!(
        action(
            &storage,
            &coordinator,
            &handlers,
            "cancel",
            json!({"cancel":true})
        )
        .await?
        .status,
        JobStatus::Succeeded
    );
    assert_eq!(transport.cancel_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        storage
            .model_batch("paid-run")
            .await?
            .ok_or("missing batch fixture value")?
            .state,
        "cancelling"
    );

    // Simulate the durable point after strict result validation and profile writes.
    // A failed remote file deletion must not fetch/reapply results after restart.
    record = storage
        .model_batch("paid-run")
        .await?
        .ok_or("missing batch fixture value")?;
    record.state = "results_saved".to_owned();
    record.document = json!({"result":{"updated_profiles":3},"terminal_state":"cancelled","output_file_id":"file-output"});
    assert!(storage.update_model_batch("cancelling", &record).await?);
    transport.fail_delete.store(true, Ordering::SeqCst);
    assert_eq!(
        action(
            &storage,
            &coordinator,
            &handlers,
            "delete-failed",
            json!({})
        )
        .await?
        .status,
        JobStatus::Failed
    );
    assert_eq!(
        storage
            .pending_model_batch()
            .await?
            .ok_or("missing batch fixture value")?
            .state,
        "results_saved"
    );
    stop_coordinator(coordinator).await?;
    transport.fail_delete.store(false, Ordering::SeqCst);
    let resumed = start_job_coordinator(storage.clone(), handlers.clone()).await?;
    let collected = action(&storage, &resumed, &handlers, "collect", json!({})).await?;
    assert_eq!(
        collected
            .result
            .as_ref()
            .ok_or("missing batch fixture value")?["updated_profiles"],
        3
    );
    assert!(storage.pending_model_batch().await?.is_none());
    assert_eq!(transport.result_calls.load(Ordering::SeqCst), 0);
    let deletes = transport.delete_calls.load(Ordering::SeqCst);
    assert_eq!(
        action(&storage, &resumed, &handlers, "again", json!({}))
            .await?
            .result,
        collected.result
    );
    assert_eq!(transport.delete_calls.load(Ordering::SeqCst), deletes);
    stop_coordinator(resumed).await
}

#[tokio::test]
async fn interrupted_uploads_cancel_locally_but_uncertain_submission_requires_explicit_abandon()
-> TestResult {
    for state in ["uploading", "submitting"] {
        let (_directory, storage, transport, handlers) = fixture().await?;
        let record = ModelBatchRecord {
            id: "paid-run".to_owned(),
            connection_id: "fixture".to_owned(),
            state: state.to_owned(),
            input_file_id: None,
            remote_batch_id: None,
            document: json!({}),
        };
        assert!(storage.create_model_batch(&record).await?);
        let coordinator = start_job_coordinator(storage.clone(), handlers.clone()).await?;
        let cancelled = action(
            &storage,
            &coordinator,
            &handlers,
            "cancel",
            json!({"cancel":true}),
        )
        .await?;
        if state == "submitting" {
            assert_eq!(cancelled.status, JobStatus::Failed);
            assert!(storage.pending_model_batch().await?.is_some());
            let abandoned = action(
                &storage,
                &coordinator,
                &handlers,
                "abandon",
                json!({"cancel":true,"abandon_uncertain":true}),
            )
            .await?;
            assert_eq!(
                abandoned.result.ok_or("missing batch fixture value")?["submission_still_uncertain"],
                true
            );
        } else {
            assert_eq!(
                cancelled.result.ok_or("missing batch fixture value")?["no_inference_submitted"],
                true
            );
        }
        assert!(storage.pending_model_batch().await?.is_none());
        assert_eq!(transport.cancel_calls.load(Ordering::SeqCst), 0);
        assert_eq!(transport.status_calls.load(Ordering::SeqCst), 0);
        stop_coordinator(coordinator).await?;
    }
    Ok(())
}

#[tokio::test]
async fn partial_batch_results_preserve_missing_usage_and_reject_stale_inference() -> TestResult {
    let (_directory, storage, transport, handlers) = fixture().await?;
    *transport.state.lock().map_err(|_| "fixture poisoned")? = "expired".to_owned();
    transport
        .return_partial_results
        .store(true, Ordering::SeqCst);
    let vocabulary = AssistantService::new(storage.clone()).vocabulary().await?;
    let manifest: ModelRunManifest = serde_json::from_value(json!({
        "schema_version":"assistant-model-run/v1", "job_id":"paid-run", "role_id":"music_tagger",
        "role_fingerprint":"old-certification", "role_configuration_fingerprint":"configuration", "connection_fingerprint":"connection",
        "adapter_id":"openai-responses/v1", "model_id":"fixture", "thinking_mode":"disabled", "timeout_seconds":30,
        "max_output_tokens_per_request":2000, "max_attempts":2,"output_token_ceiling":4000,
        "evaluation_id":"music-tagging-quality-v1", "disclosure_version":"assistant-model-music-tagging-disclosure/v12",
        "scope_fingerprint":"scope","evidence_fingerprint":"evidence","review_destination":"track_tag_review","queue_wait_seconds":0
    }))?;
    let mut usage = ProviderUsageAccumulator::for_run(manifest)
        .summary()
        .clone();
    usage.attempted_requests = 2;
    usage.uncertain_requests = 2;
    usage.reserved_tokens = 100_000;
    usage.attempts = serde_json::from_value(json!([
        {"sequence":1,"request_fingerprint":"a","max_output_tokens":2000,"outcome":"uncertain","elapsed_ms":null},
        {"sequence":2,"request_fingerprint":"b","max_output_tokens":2000,"outcome":"uncertain","elapsed_ms":null}
    ]))?;
    let inputs = [101,102].map(|id| json!({"track_id":id,"artist":"Artist","album":"Album","origin":"","genre":"folk","length_s":120}));
    let templates = [101,102].map(|id| json!({"track_id":id,"source_signature":"a".repeat(64),"energy":0.5,"brightness":0.5,"tension":0.5,"moods":[],"evidence":[],"metrics":{},"confidence":"low"}));
    let mut record = ModelBatchRecord {
        id: "paid-run".to_owned(),
        connection_id: "fixture".to_owned(),
        state: "submitted".to_owned(),
        input_file_id: None,
        remote_batch_id: None,
        document: json!({"parameters":{
            "inference_fingerprint":"old-inference", "role_fingerprint":"old-certification", "vocabulary_fingerprint":vocabulary.fingerprint,
            "role_id":"music_tagger","quality_evaluation_id":"music-tagging-quality-v1","disclosure_version":"assistant-model-music-tagging-disclosure/v12","consent":true,
            "scope":{"type":"all","path":"","recursive":false,"track_ids":[]},"context_policy":"include","force":false
        },"inputs":inputs,"templates":templates,"ranges":[{"start":0,"end":1},{"start":1,"end":2}],"submission_usage":usage}),
    };
    assert!(storage.create_model_batch(&record).await?);
    record.input_file_id = Some("file-input".to_owned());
    record.remote_batch_id = Some("batch-owned".to_owned());
    assert!(storage.update_model_batch("submitted", &record).await?);
    let coordinator = start_job_coordinator(storage.clone(), handlers.clone()).await?;
    let job = action(&storage, &coordinator, &handlers, "partial", json!({})).await?;
    assert_eq!(job.status, JobStatus::Succeeded, "{:?}", job.error);
    let result = job.result.ok_or("result missing")?;
    assert_eq!(result["stale_configuration"], true);
    assert_eq!(result["updated_profiles"], 0);
    assert_eq!(result["rejected_tracks"], 1);
    assert_eq!(result["unavailable_or_changed_tracks"], 1);
    assert_eq!(result["usage"]["uncertain_requests"], 1);
    assert_eq!(result["usage"]["response_received_requests"], 1);
    assert_eq!(result["usage"]["input_tokens"], 100);
    assert_eq!(result["usage"]["token_details"]["cached_input_tokens"], 80);
    let saved = storage
        .model_batch("paid-run")
        .await?
        .ok_or("batch missing")?;
    assert_eq!(saved.state, "expired");
    assert!(saved.document.get("inputs").is_none());
    assert_eq!(transport.delete_calls.load(Ordering::SeqCst), 2);
    stop_coordinator(coordinator).await
}
