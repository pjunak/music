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
    allow_standard: AtomicBool,
    standard_tracks: Mutex<Vec<String>>,
    standard_calls: AtomicUsize,
    block_standard_call: AtomicUsize,
    release_standard: tokio::sync::Notify,
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
        request: &'a StructuredModelRequest,
    ) -> ModelTransportFuture<'a> {
        assert!(
            self.allow_standard.load(Ordering::SeqCst),
            "collection must never start new inference"
        );
        Box::pin(async move {
            let call = self.standard_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if self.block_standard_call.load(Ordering::SeqCst) == call {
                self.release_standard.notified().await;
                return StructuredModelResult {
                    succeeded: false,
                    outcome: ProviderAttemptOutcome::Uncertain,
                    error_code: Some("provider_timeout".to_owned()),
                    payload: None,
                    provider_model_id: None,
                    finish_reason: None,
                    input_tokens: None,
                    output_tokens: None,
                    token_details: Default::default(),
                };
            }
            let input: Value = serde_json::from_str(&request.user_prompt)
                .unwrap_or_else(|_| panic!("invalid fixture input"));
            let tracks = input["tracks"]
                .as_array()
                .unwrap_or_else(|| panic!("fixture tracks missing"));
            self.standard_tracks
                .lock()
                .unwrap_or_else(|_| panic!("fixture poisoned"))
                .extend(
                    tracks
                        .iter()
                        .map(|track| track["album"].as_str().unwrap_or_default().to_owned()),
                );
            StructuredModelResult {
                succeeded: true,
                outcome: ProviderAttemptOutcome::ResponseReceived,
                error_code: None,
                payload: Some(
                    json!({"schema_version":MODEL_TAGGER_OUTPUT_CONTRACT,"tracks":tracks.iter().map(|track| json!({"track_id":track["track_id"],"tag_ids":[],"confidence":"low","evidence":["Insufficient musical evidence"]})).collect::<Vec<_>>()}),
                ),
                provider_model_id: Some("fixture-model".to_owned()),
                finish_reason: Some("stop".to_owned()),
                input_tokens: Some(100),
                output_tokens: Some(10),
                token_details: Default::default(),
            }
        })
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

#[tokio::test]
async fn empty_tagging_pilots_stop_and_continue_without_rebilling_current_tracks() -> TestResult {
    let (_directory, storage, transport, handlers) = fixture().await?;
    transport.allow_standard.store(true, Ordering::SeqCst);
    sqlx::query("UPDATE assistant_provider_connections SET verification_status='verified', verified_models_json='[\"fixture-model\"]', verified_capabilities_json='[\"structured-text/v1\"]' WHERE id='fixture'")
        .execute(&storage.pool).await?;
    let providers = Arc::new(ProviderService::new(
        storage.clone(),
        Arc::new(Credentials(Arc::new(crate::CredentialVault::from_key(
            [7; 32],
        )?))),
        Arc::new(Policy),
        "new-certification-digest".to_owned(),
    ));
    let connection = storage
        .provider_connection("fixture")
        .await?
        .ok_or("fixture connection")?;
    let mut role = ModelRoleRecord {
        role_id: "music_tagger".to_owned(),
        connection_id: "fixture".to_owned(),
        model_id: "fixture-model".to_owned(),
        enabled: true,
        timeout_seconds: 30,
        max_output_tokens: 8_000,
        thinking_mode: "provider_default".to_owned(),
        conformance_status: "passed".to_owned(),
        conformance_error_code: None,
        conformance_fingerprint: None,
        last_conformance_at_unix_seconds: Some(1_800_000_000),
        updated_at_unix_seconds: 0,
    };
    role.conformance_fingerprint = Some(providers.role_runtime_fingerprint(&role, &connection));
    storage
        .save_model_role(&connection.fingerprint(), &role, false)
        .await?;
    let execution = providers.prepare_role_execution("music_tagger").await?;
    // Synthetic certification is seeded only in this isolated database. The
    // production job still exercises every normal execution and storage guard.
    sqlx::query("INSERT INTO background_jobs (id,kind,status,parameters_json,result_json,error,progress_current,progress_total,progress_phase,progress_message,attempts,retry_of_id,created_at,updated_at,lane,schema_version,restartable,checkpoint_policy) VALUES ('synthetic','assistant.model.test','succeeded','{}','{}',NULL,1,1,'Done','Done',1,NULL,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,'provider',1,0,'replace')")
        .execute(&storage.pool).await?;
    sqlx::query("INSERT INTO assistant_model_evaluations (role_id,evaluation_id,role_fingerprint,status,suite_id,engine_id,passed_cases,total_cases,job_id,evaluated_at) VALUES ('music_tagger',?,?, 'passed',?,?,1,1,'synthetic',CURRENT_TIMESTAMP)")
        .bind(TAGGING_QUALITY_EVALUATION_ID).bind(&execution.fingerprint).bind(TAGGING_QUALITY_SUITE_ID).bind(MODEL_TAG_ANALYZER_ID).execute(&storage.pool).await?;
    for index in 0..50 {
        sqlx::query("INSERT INTO tracks (path,title,artist,album_artist,album,genre,length_s,display_title,origin,size_bytes,mtime,added_at) VALUES (?,'Misleading private filename','','',?,'',120,'Private display','','1',1,CURRENT_TIMESTAMP)")
            .bind(format!("private-{index}.mp3")).bind(format!("Fixture {index}")).execute(&storage.pool).await?;
    }
    let vocabulary = AssistantService::new(storage.clone()).vocabulary().await?;
    let coordinator = start_job_coordinator(storage.clone(), handlers).await?;
    let parameters = json!({
        "inference_fingerprint":execution.inference_fingerprint,"role_fingerprint":execution.fingerprint,
        "vocabulary_fingerprint":vocabulary.fingerprint,"role_id":"music_tagger",
        "quality_evaluation_id":TAGGING_QUALITY_EVALUATION_ID,"disclosure_version":"assistant-model-music-tagging-disclosure/v14","consent":true,
        "scope":{"type":"all","path":"","recursive":false,"track_ids":[]},"context_policy":"include","force":false,
        "limits":{"max_tracks":35,"max_requests":10,"max_token_reservation":1_000_000,"stop_on_empty_batch":true}
    });
    for (index, expected) in [
        (20, (0, 15, 15, true)),
        (20, (20, 0, 10, true)),
        (10, (40, 0, 0, false)),
    ] {
        let mut next = parameters.clone();
        if expected.0 > 0 {
            next["limits"]["max_tracks"] = json!(50);
        }
        let queued = coordinator
            .service
            .enqueue(MODEL_TAGGING_JOB_KIND, next)
            .await?;
        let completed = wait_for_job(&storage, &queued.id, |job| {
            matches!(job.status, JobStatus::Succeeded | JobStatus::Failed)
        })
        .await?;
        assert_eq!(
            completed.status,
            JobStatus::Succeeded,
            "{:?}",
            completed.error
        );
        let result = completed.result.ok_or("result missing")?;
        assert_eq!(result["updated_profiles"], index);
        assert_eq!(result["tracks_without_suggestions"], index);
        assert_eq!(result["suggested_tags"], 0);
        assert_eq!(result["unchanged_profiles"], expected.0);
        assert_eq!(result["deferred_tracks"], expected.1);
        assert_eq!(result["remaining_tracks"], expected.2);
        assert_eq!(result["stopped_empty_batch"], expected.3);
        assert_eq!(result["usage"]["attempted_requests"], 1);
    }
    {
        let sent = transport
            .standard_tracks
            .lock()
            .map_err(|_| "fixture poisoned")?;
        assert_eq!(sent.len(), 50);
        assert_eq!(
            sent.iter().collect::<std::collections::BTreeSet<_>>().len(),
            50
        );
    }
    let tracks = AssistantRepository::tracks(storage.as_ref()).await?;
    assert!(tracks.iter().all(|track| track.manual_tags.is_empty()));
    assert!(
        tracks
            .iter()
            .all(|track| track.analyses.iter().any(|profile| profile.evidence
                == ["Insufficient musical evidence"]
                && !profile.metrics["input_snapshot"]
                    .to_string()
                    .contains("private-")))
    );

    let mut batch = parameters.clone();
    batch["execution_mode"] = json!("batch");
    batch["force"] = json!(true);
    let queued = coordinator
        .service
        .enqueue(MODEL_TAGGING_JOB_KIND, batch)
        .await?;
    let rejected =
        wait_for_job(&storage, &queued.id, |job| job.status == JobStatus::Failed).await?;
    assert_eq!(rejected.error.as_deref(), Some("batch_pilot_required"));
    assert_eq!(transport.standard_calls.load(Ordering::SeqCst), 3);

    // Deliberate override continues past an empty request. Cancel while a later
    // request is uncertain: its write-ahead checkpoint must retain prior yield.
    transport.block_standard_call.store(5, Ordering::SeqCst);
    let mut override_run = parameters.clone();
    override_run["force"] = json!(true);
    override_run["limits"]["stop_on_empty_batch"] = json!(false);
    let queued = coordinator
        .service
        .enqueue(MODEL_TAGGING_JOB_KIND, override_run)
        .await?;
    wait_for_job(&storage, &queued.id, |job| {
        job.result
            .as_ref()
            .is_some_and(|result| result["usage"]["attempted_requests"] == 2)
    })
    .await?;
    coordinator.service.cancel(&queued.id).await?;
    transport.release_standard.notify_one();
    let cancelled = wait_for_job(&storage, &queued.id, |job| {
        job.status == JobStatus::Cancelled
    })
    .await?;
    let partial = cancelled.result.ok_or("partial checkpoint missing")?;
    assert_eq!(partial["feature_progress"]["processed_tracks"], 20);
    assert_eq!(
        partial["feature_progress"]["tracks_without_suggestions"],
        20
    );
    assert_eq!(partial["feature_progress"]["updated_profiles"], 20);
    assert_eq!(
        partial["feature_progress"]["track_results"]
            .as_array()
            .map(Vec::len),
        Some(20)
    );
    assert_eq!(partial["usage"]["uncertain_requests"], 1);
    stop_coordinator(coordinator).await
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
        "evaluation_id":"music-tagging-quality-v1", "disclosure_version":"assistant-model-music-tagging-disclosure/v14",
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
            "role_id":"music_tagger","quality_evaluation_id":"music-tagging-quality-v1","disclosure_version":"assistant-model-music-tagging-disclosure/v14","consent":true,
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
    assert_eq!(result["processed_tracks"], 0);
    assert_eq!(result["suggested_tags"], 0);
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
