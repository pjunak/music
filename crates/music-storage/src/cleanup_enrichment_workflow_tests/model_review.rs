use super::*;
use music_application::assistant::*;
use music_application::cleanup::CleanupScope;
use music_application::cleanup_enrichment::ai::{CLEANUP_AI_JOB_KIND, CleanupAiJobHandler};
use music_application::cleanup_enrichment::cleanup_enrichment_source_signature;

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
        Ok(raw.into())
    }
}
#[derive(Debug, Default)]
struct Transport {
    calls: AtomicUsize,
    timeout: AtomicBool,
    change_folder: tokio::sync::Mutex<Option<(Arc<SqliteStorage>, String)>>,
}
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
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert!(!request.user_prompt.contains("album/song.mp3"));
            assert!(!request.user_prompt.contains(RECORDING));
            if let Some((storage, path)) = self.change_folder.lock().await.take() {
                assert!(
                    sqlx::query("UPDATE tracks SET path = ? WHERE id = 2")
                        .bind(path)
                        .execute(&storage.pool)
                        .await
                        .is_ok()
                );
            }
            let timeout = self.timeout.load(Ordering::SeqCst);
            StructuredModelResult {
                succeeded: !timeout,
                outcome: if timeout { ProviderAttemptOutcome::Uncertain } else { ProviderAttemptOutcome::ResponseReceived },
                error_code: timeout.then(|| "provider_timeout".into()),
                payload: (!timeout).then(|| json!({"schema_version":"assistant-library-cleanup-output/v1", "decision":"select", "candidate_id":"candidate-0", "evidence_ids":["candidate-0-artist","candidate-0-duration"], "reason":"The supplied artist and duration support this candidate; review its title."})),
                provider_model_id: Some("fixture-model".into()), finish_reason: Some("stop".into()),
                input_tokens: Some(100), output_tokens: Some(20), token_details: Default::default(),
            }
        })
    }
}

async fn review(service: &JobService, parameters: Value) -> TestResult<JobRecord> {
    let job = service.enqueue(CLEANUP_AI_JOB_KIND, parameters).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let current = service.get(&job.id).await?.ok_or("missing review")?;
            if matches!(
                current.status,
                JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled
            ) {
                return Ok(current);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?
}

#[tokio::test]
async fn model_review_gates_cost_and_keeps_suggestions_separate_from_authored_metadata()
-> TestResult {
    run_model_review("album/song.mp3", "album/neighbor.mp3").await
}

#[tokio::test]
async fn model_review_rechecks_neighboring_disc_evidence_before_and_after_provider_cost()
-> TestResult {
    run_model_review("album/Disc 1/song.mp3", "album/CD2/neighbor.mp3").await
}

async fn run_model_review(track_path: &str, neighbor_path: &str) -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (catalog, _) = setup(storage.clone()).await?;
    sqlx::query("UPDATE tracks SET path = ? WHERE id = 1")
        .bind(track_path)
        .execute(&storage.pool)
        .await?;
    let vault = Arc::new(crate::CredentialVault::from_key([7; 32])?);
    let encrypted = vault.encrypt("fixture", "synthetic-test-credential")?;
    sqlx::query("INSERT INTO assistant_provider_connections (id,name,adapter_id,base_url,encrypted_api_key,api_key_nonce,api_key_hint,allow_private_network,verification_status,verified_models_json,verified_capabilities_json,created_at,updated_at) VALUES ('fixture','Fixture','openai-responses/v1','https://api.openai.com/v1',?,?,?,0,'verified','[\"fixture-model\"]','[\"structured-text/v1\"]',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)")
        .bind(encrypted.ciphertext).bind(encrypted.nonce).bind(encrypted.hint).execute(&storage.pool).await?;
    let providers = Arc::new(ProviderService::new(
        storage.clone(),
        Arc::new(Credentials(vault)),
        Arc::new(Policy),
        "fixture-runtime".into(),
    ));
    let connection = storage
        .provider_connection("fixture")
        .await?
        .ok_or("missing connection")?;
    let mut role = ModelRoleRecord {
        role_id: "library_cleanup".into(),
        connection_id: "fixture".into(),
        model_id: "fixture-model".into(),
        enabled: true,
        timeout_seconds: 30,
        max_output_tokens: 8000,
        thinking_mode: "provider_default".into(),
        conformance_status: "passed".into(),
        conformance_error_code: None,
        conformance_fingerprint: None,
        last_conformance_at_unix_seconds: Some(1_800_000_000),
        updated_at_unix_seconds: 0,
    };
    role.conformance_fingerprint = Some(providers.role_runtime_fingerprint(&role, &connection));
    storage
        .save_model_role(&connection.fingerprint(), &role, false)
        .await?;
    let execution = providers.prepare_role_execution("library_cleanup").await?;
    let cleanup = Arc::new(CleanupService::new(storage.clone()));
    let tracks = cleanup.tracks(CleanupScope::All).await?;
    let signature =
        cleanup_enrichment_source_signature(&tracks[0]).map_err(std::io::Error::other)?;
    // One track in this folder. The independent fixture hashes that source once.
    use sha2::{Digest, Sha256};
    let folder_signature = format!("{:x}", Sha256::digest(signature.as_bytes()));
    sqlx::query("INSERT INTO tracks (path, title, artist, album_artist, album, track_no, disc_no, year, genre, length_s, bpm, size_bytes, mtime, added_at, display_title, origin) SELECT 'other/neighbor.mp3', title, artist, album_artist, album, track_no, disc_no, year, genre, length_s, bpm, size_bytes, mtime, added_at, display_title, origin FROM tracks WHERE id = 1").execute(&storage.pool).await?;
    let revision = storage.catalog_evidence_revision().await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let mut catalog_result = json!({"plans":[{"track_id":1,"status":"unmatched","source_signature":signature,"indexed_folder_signature":folder_signature,"evidence_revision":revision,"retrieved_at":now,
        "candidates":[{"id":RECORDING,"title":"Reviewed Song","artist":"Artist","length_ms":120000,"provider_score":0.8,"releases":[]}]}]});
    sqlx::query("INSERT INTO background_jobs (id,kind,status,parameters_json,result_json,progress_current,progress_total,progress_phase,progress_message,attempts,created_at,updated_at,lane,schema_version,restartable,checkpoint_policy) VALUES ('catalog',?,'succeeded','{}',?,1,1,'Done','Done',1,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,'provider',1,0,'replace')")
        .bind(CLEANUP_ENRICHMENT_JOB_KIND).bind(catalog_result.to_string()).execute(&storage.pool).await?;
    let transport = Arc::new(Transport::default());
    let handler = CleanupAiJobHandler {
        cleanup,
        cache: storage.clone(),
        sources: catalog.sources.clone(),
        quality: Arc::new(ModelQualityService::new(storage.clone(), providers)),
        transport: transport.clone(),
    };
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    let params = json!({"track_id":1,"catalog_job_id":"catalog","consent":true,"disclosure_version":LIBRARY_CLEANUP_DISCLOSURE,"role_fingerprint":execution.fingerprint});
    // Missing quality and consent both fail before transport execution.
    assert_eq!(
        review(&coordinator.service, params.clone()).await?.status,
        JobStatus::Failed
    );
    let mut no_consent = params.clone();
    no_consent["consent"] = json!(false);
    assert_eq!(
        review(&coordinator.service, no_consent).await?.status,
        JobStatus::Failed
    );
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    // Seed certification only in this isolated test database.
    sqlx::query("INSERT INTO assistant_model_evaluations (role_id,evaluation_id,role_fingerprint,status,suite_id,engine_id,passed_cases,total_cases,job_id,evaluated_at) VALUES ('library_cleanup',?,?,'passed',?,?,8,8,'catalog',CURRENT_TIMESTAMP)")
        .bind(LIBRARY_CLEANUP_QUALITY_ID).bind(&execution.fingerprint).bind(LIBRARY_CLEANUP_SUITE_ID).bind(LIBRARY_CLEANUP_ENGINE_ID).execute(&storage.pool).await?;
    catalog_result["plans"][0]["retrieved_at"] = json!(now - 21601);
    sqlx::query("UPDATE background_jobs SET result_json=? WHERE id='catalog'")
        .bind(catalog_result.to_string())
        .execute(&storage.pool)
        .await?;
    let stale = review(&coordinator.service, params.clone()).await?;
    assert_eq!(stale.error.as_deref(), Some("cleanup_evidence_stale"));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    catalog_result["plans"][0]["retrieved_at"] = json!(now);
    sqlx::query("UPDATE background_jobs SET result_json=? WHERE id='catalog'")
        .bind(catalog_result.to_string())
        .execute(&storage.pool)
        .await?;
    let succeeded = result(&review(&coordinator.service, params.clone()).await?)?;
    assert_eq!(succeeded["ops"][0]["new"], "Reviewed Song");
    assert_eq!(succeeded["ops"][0]["confidence"], "low");
    assert_eq!(succeeded["ops"][0]["verified"], false);
    let title: String = sqlx::query_scalar("SELECT title FROM tracks WHERE id=1")
        .fetch_one(&storage.pool)
        .await?;
    assert_eq!(title, "Song");
    // An unrelated folder is harmless, but a changed sibling set invalidates
    // candidate evidence before cost and again before publishing model proposals.
    sqlx::query("UPDATE tracks SET path = ? WHERE id = 2")
        .bind(neighbor_path)
        .execute(&storage.pool)
        .await?;
    let changed = review(&coordinator.service, params.clone()).await?;
    assert_eq!(changed.error.as_deref(), Some("cleanup_evidence_stale"));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    sqlx::query("UPDATE tracks SET path = 'other/neighbor.mp3' WHERE id = 2")
        .execute(&storage.pool)
        .await?;
    *transport.change_folder.lock().await = Some((storage.clone(), neighbor_path.into()));
    let raced = review(&coordinator.service, params.clone()).await?;
    assert_eq!(raced.error.as_deref(), Some("cleanup_evidence_stale"));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 2);
    assert!(
        raced
            .result
            .as_ref()
            .is_some_and(|result| !result.contains_key("ops"))
    );
    sqlx::query("UPDATE tracks SET path = 'other/neighbor.mp3' WHERE id = 2")
        .execute(&storage.pool)
        .await?;
    transport.timeout.store(true, Ordering::SeqCst);
    let uncertain = review(&coordinator.service, params).await?;
    assert_eq!(uncertain.status, JobStatus::Failed);
    assert_eq!(uncertain.attempts, 1);
    assert!(!uncertain.restartable);
    assert!(
        uncertain.result.is_some(),
        "paid attempt checkpoint must survive failure"
    );
    assert_eq!(transport.calls.load(Ordering::SeqCst), 3);
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}
