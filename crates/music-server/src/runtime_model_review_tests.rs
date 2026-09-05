#[tokio::test]
async fn model_tag_review_routes_expose_current_proposals_and_preserve_manual_decisions()
-> Result<(), Box<dyn Error>> {
    use music_application::assistant::{
        AnalysisWrite, Confidence, LocalAnalysisRepository, MODEL_TAG_ANALYZER_ID,
        ModelAnalysisWrite, ModelRoleRecord, ProviderConnectionRecord, ProviderRepository,
        model_tag_source_signature,
    };
    let directory = tempdir()?;
    fs::create_dir_all(directory.path().join("music"))?;
    fs::write(
        directory.path().join("music/Review.wav"),
        reference_wav().map_err(|_| "missing fixture")?,
    )?;
    let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
    tokio::time::timeout(Duration::from_secs(3), async {
        while runtime
            .library_service
            .all_tracks()
            .await
            .is_ok_and(|tracks| tracks.is_empty())
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    let (router, cookie) = operator_router(&runtime).await?;
    let connection = ProviderConnectionRecord {
        id: "fixture".to_owned(),
        name: "Review fixture".to_owned(),
        adapter_id: "openai-compatible/v1".to_owned(),
        base_url: "https://example.test/v1".to_owned(),
        encrypted_api_key: String::new(),
        api_key_nonce: String::new(),
        api_key_hint: String::new(),
        allow_private_network: false,
        verification_status: "never".to_owned(),
        verification_error_code: None,
        verified_models: Vec::new(),
        verified_capability_ids: Vec::new(),
        last_verified_at_unix_seconds: None,
        created_at_unix_seconds: 0,
        updated_at_unix_seconds: 0,
    };
    runtime
        .storage
        .create_provider_connection(&connection)
        .await
        .map_err(|_| "connection fixture failed")?;
    let role = ModelRoleRecord {
        role_id: "music_tagger".to_owned(),
        connection_id: connection.id.clone(),
        model_id: "fixture-model".to_owned(),
        enabled: false,
        timeout_seconds: 30,
        max_output_tokens: 8000,
        thinking_mode: "provider_default".to_owned(),
        conformance_status: "never".to_owned(),
        conformance_error_code: None,
        conformance_fingerprint: None,
        last_conformance_at_unix_seconds: None,
        updated_at_unix_seconds: 0,
    };
    runtime
        .storage
        .save_model_role(&connection.fingerprint(), &role, false)
        .await
        .map_err(|_| "role fixture failed")?;
    let fingerprint = runtime
        .providers
        .provider_service()
        .current_role_runtime_fingerprint("music_tagger")
        .await?
        .ok_or("role missing")?;
    let vocabulary = runtime.assistant.vocabulary().await?;
    let track = runtime.assistant.tracks().await?.remove(0).track;
    let signature =
        model_tag_source_signature(&track, &fingerprint, &vocabulary.fingerprint, None)?;
    assert_eq!(
        runtime
            .storage
            .store_model_analysis(
                MODEL_TAG_ANALYZER_ID,
                "synthetic-model-job",
                &fingerprint,
                &vocabulary.fingerprint,
                None,
                &[ModelAnalysisWrite {
                    profile: AnalysisWrite {
                        track_id: track.id,
                        source_signature: signature.clone(),
                        energy: 0.5,
                        brightness: 0.5,
                        tension: 0.5,
                        moods: vec!["calm".to_owned()],
                        evidence: vec!["Synthetic metadata".to_owned()],
                        metrics: json!({"contract":"assistant-music-tagger-output/v3"})
                            .as_object()
                            .cloned()
                            .ok_or("metrics missing")?,
                        confidence: Confidence::High,
                    }
                }]
            )
            .await
            .map_err(|_| "profile fixture failed")?,
        1
    );
    for decision in ["pending", "accepted", "rejected", "pending"] {
        if decision != "pending" || !runtime.assistant.tracks().await?[0].reviews.is_empty() {
            let response = router.clone().oneshot(Request::put(format!("/api/assistant/library-tags/{}/analysis-tags/review", track.id.get()))
                .header("cookie", &cookie).header("content-type", "application/json")
                .body(Body::from(json!({"tag":"calm","analyzer_id":MODEL_TAG_ANALYZER_ID,"source_signature":signature,"decision":decision}).to_string()))?).await?;
            assert_eq!(response.status(), StatusCode::OK);
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await?)?;
            assert_eq!(body["manual_tags"], json!(["calm"]));
        }
        let response = router
            .clone()
            .oneshot(
                Request::post("/api/assistant/library-tags/query")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"scope":{"type":"all"},"review":decision,"offset":0,"limit":100})
                            .to_string(),
                    ))?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await?)?;
        assert_eq!(body["total"], 1, "{decision}: {body}");
        assert_eq!(
            body["items"][0]["analysis_suggestions"][0]["status"],
            decision
        );
    }
    let patched = runtime
        .assistant
        .patch_track(track.id, &["authored".to_owned()], &[])
        .await?;
    assert_eq!(patched.analysis_suggestions.len(), 1);
    let mut changed = vocabulary.document;
    changed.groups[0].tags[0]
        .description
        .push_str(" Changed definition.");
    runtime
        .assistant
        .replace_vocabulary(vocabulary.revision, changed)
        .await?;
    let page = runtime
        .assistant
        .tag_page(music_application::assistant::ManualTagQuery::default())
        .await?;
    assert!(page.items[0].analysis_suggestions.is_empty());
    assert_eq!(page.items[0].manual_tags, vec!["authored", "calm"]);
    runtime.shutdown().await?;
    Ok(())
}
