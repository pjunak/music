#[tokio::test]
async fn model_review_rechecks_configuration_and_evidence_inside_the_transaction()
-> Result<(), Box<dyn Error + Send + Sync>> {
    use music_application::assistant::{
        AnalysisWrite, Confidence, LocalAnalysisRepository, ModelAnalysisWrite,
        ModelRoleReviewIdentity, model_tag_source_signature,
    };
    use std::sync::Arc;
    for mutation in [
        "none",
        "role",
        "credential",
        "vocabulary",
        "metadata",
        "profile",
        "context",
        "malformed",
        "missing-guard",
    ] {
        let (_directory, storage) = storage().await?;
        sqlx::query("INSERT INTO assistant_provider_connections (id,name,adapter_id,base_url,encrypted_api_key,api_key_nonce,api_key_hint,allow_private_network,verification_status,verified_models_json,verified_capabilities_json,created_at,updated_at) VALUES ('fixture','Fixture','openai-compatible/v1','https://example.test/v1','cipher','nonce','fixture',0,'never','[]','[]',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)").execute(&storage.pool).await?;
        sqlx::query("INSERT INTO assistant_model_roles (role_id,connection_id,model_id,enabled,timeout_seconds,max_output_tokens,thinking_mode,conformance_status,updated_at) VALUES ('music_tagger','fixture','fixture-model',0,30,8000,'provider_default','never',CURRENT_TIMESTAMP)").execute(&storage.pool).await?;
        let storage = Arc::new(storage);
        let vocabulary = AssistantService::new(storage.clone()).vocabulary().await?;
        let track = storage.tracks().await?.remove(0).track;
        let mut tx = storage.pool.begin().await?;
        let role = crate::providers::load_role_tx(&mut tx, "music_tagger")
            .await?
            .ok_or("role missing")?;
        let connection = crate::providers::load_connection_tx(&mut tx, "fixture")
            .await?
            .ok_or("connection missing")?;
        tx.rollback().await?;
        let guard = ModelTagReviewGuard {
            role: ModelRoleReviewIdentity {
                runtime_fingerprint: "a".repeat(64),
                configuration_fingerprint: role.configuration_fingerprint(),
                connection_fingerprint: connection.fingerprint(),
            },
            vocabulary_fingerprint: vocabulary.fingerprint.clone(),
            voice_signature: None,
        };
        let signature = model_tag_source_signature(
            &track,
            &guard.role.runtime_fingerprint,
            &vocabulary.fingerprint,
            None,
        )?;
        let target = AnalysisReviewTarget {
            track_id: track.id,
            tag: "calm".to_owned(),
            analyzer_id: MODEL_TAG_ANALYZER_ID.to_owned(),
            source_signature: signature.clone(),
        };
        assert_eq!(
            storage
                .store_model_analysis(
                    MODEL_TAG_ANALYZER_ID,
                    "fixture-job",
                    &guard.role.runtime_fingerprint,
                    &vocabulary.fingerprint,
                    None,
                    &[ModelAnalysisWrite {
                        profile: AnalysisWrite {
                            track_id: track.id,
                            source_signature: signature,
                            energy: 0.5,
                            brightness: 0.5,
                            tension: 0.5,
                            moods: vec!["calm".to_owned()],
                            evidence: vec!["Synthetic evidence".to_owned()],
                            metrics:
                                serde_json::json!({"contract":"assistant-music-tagger-output/v3"})
                                    .as_object()
                                    .cloned()
                                    .ok_or("metrics missing")?,
                            confidence: Confidence::High,
                        }
                    }]
                )
                .await?,
            1
        );
        match mutation {
            "context" => {
                use music_application::assistant::{
                    ContextWrite, LOCAL_CONTEXT_ANALYZER_ID, LOCAL_CONTEXT_IMPLEMENTATION_ID,
                    context_source_signature,
                };
                let context = ContextWrite {
                    track_id: track.id,
                    source_signature: context_source_signature(
                        &track,
                        LOCAL_CONTEXT_IMPLEMENTATION_ID,
                        None,
                    )?,
                    completeness: "full".to_owned(),
                    confidence: "medium".to_owned(),
                    summary: serde_json::json!({"schema_version":LOCAL_CONTEXT_ANALYZER_ID})
                        .as_object()
                        .cloned()
                        .ok_or("summary missing")?,
                    timeline: Vec::new(),
                    sections: Vec::new(),
                    technical: Map::new(),
                    stages: Map::new(),
                };
                assert!(
                    storage
                        .store_context(
                            LOCAL_CONTEXT_ANALYZER_ID,
                            LOCAL_CONTEXT_IMPLEMENTATION_ID,
                            None,
                            "context-job",
                            &context
                        )
                        .await?
                );
            }
            "malformed" => {
                sqlx::query("UPDATE track_analyses SET confidence = 'invalid'")
                    .execute(&storage.pool)
                    .await?;
            }
            "role" => {
                sqlx::query("UPDATE assistant_model_roles SET model_id = 'changed'")
                    .execute(&storage.pool)
                    .await?;
            }
            "credential" => {
                sqlx::query(
                    "UPDATE assistant_provider_connections SET encrypted_api_key = 'replacement'",
                )
                .execute(&storage.pool)
                .await?;
            }
            "vocabulary" => {
                let mut document = vocabulary.document.clone();
                document.groups[0].tags[0].description.push_str(" Changed.");
                storage
                    .replace_vocabulary(vocabulary.revision, &document)
                    .await?;
            }
            "metadata" => {
                sqlx::query("UPDATE tracks SET album = 'Changed' WHERE id = ?")
                    .bind(track.id.get())
                    .execute(&storage.pool)
                    .await?;
            }
            "profile" => {
                sqlx::query("UPDATE track_analyses SET source_signature = ?")
                    .bind("b".repeat(64))
                    .execute(&storage.pool)
                    .await?;
            }
            _ => {}
        }
        let outcome = storage
            .review_analysis(
                std::slice::from_ref(&target),
                AnalysisReviewDecision::Accepted,
                (mutation != "missing-guard").then_some(&guard),
            )
            .await?;
        if mutation == "none" {
            assert_eq!(outcome.applied.len(), 1);
            for decision in [
                AnalysisReviewDecision::Rejected,
                AnalysisReviewDecision::Pending,
            ] {
                assert_eq!(
                    storage
                        .review_analysis(std::slice::from_ref(&target), decision, Some(&guard))
                        .await?
                        .applied
                        .len(),
                    1
                );
                assert_eq!(storage.tracks().await?[0].manual_tags, vec!["calm"]);
            }
            assert!(storage.tracks().await?[0].reviews.is_empty());
        } else {
            assert!(outcome.applied.is_empty(), "{mutation}");
            assert_eq!(
                outcome.failures[0].code,
                AnalysisReviewFailureCode::Stale,
                "{mutation}"
            );
            assert!(
                storage.tracks().await?[0].manual_tags.is_empty(),
                "{mutation}"
            );
        }
    }
    Ok(())
}
