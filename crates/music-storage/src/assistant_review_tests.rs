
fn review_fixture_decisions(tags: &[&str]) -> Vec<music_application::assistant::TagDecision> {
    tags.iter().map(|tag| music_application::assistant::TagDecision {
        tag: (*tag).to_owned(), support: music_application::assistant::TagSupport::Tentative,
        evidence: vec!["Synthetic evidence".to_owned()], evidence_ids: vec!["metadata.length_s".to_owned()], contradiction_ids: Vec::new(),
    }).collect()
}

#[tokio::test]
async fn review_write_failure_rolls_back_manual_tags_and_earlier_decisions()
-> Result<(), Box<dyn Error + Send + Sync>> {
    use music_application::assistant::{AnalysisWrite, LocalAnalysisRepository};
    let (_directory, storage) = storage().await?;
    music_application::cleanup_sources::CleanupSourceRepository::set_cleanup_source_enabled(&storage, "lastfm", true).await?;
    let storage = std::sync::Arc::new(storage);
    let tracks = AssistantRepository::tracks(storage.as_ref()).await?;
    let mut targets = Vec::new();
    for evidence in &tracks {
        let revision = music_application::cleanup_enrichment::CleanupEnrichmentRepository::catalog_evidence_revision(storage.as_ref()).await?;
        let source_signature = music_application::assistant::catalog_tag_source_signature(&evidence.track, revision)?;
        storage.store_catalog_analysis(
            CATALOG_TAG_ANALYZER_ID, "fixture", &[AnalysisWrite {
                track_id: evidence.track.id, source_signature: source_signature.clone(),
                decisions: review_fixture_decisions(&["calm"]),
                moods: vec!["calm".to_owned()], evidence: vec!["Synthetic metadata".to_owned()],
                metrics: serde_json::json!({"evidence_revision":revision}).as_object().cloned().ok_or("metrics")?,
            }],
        ).await?;
        targets.push(AnalysisReviewTarget {
            track_id: evidence.track.id, tag: "calm".to_owned(),
            analyzer_id: CATALOG_TAG_ANALYZER_ID.to_owned(), source_signature,
        });
    }
    assert_eq!(targets.len(), 2);
    // The second write fails after the first track's tag and review have been
    // written. Neither the first decision nor either manual tag may escape.
    sqlx::query("CREATE TRIGGER fail_second_review BEFORE INSERT ON track_analysis_tag_reviews \
        WHEN NEW.track_id = 2 BEGIN SELECT RAISE(ABORT, 'synthetic review failure'); END")
        .execute(&storage.pool).await?;
    assert!(AssistantRepository::review_analysis(
        storage.as_ref(), &targets, AnalysisReviewDecision::Accepted, None,
    ).await.is_err());
    for table in ["track_user_tags", "track_analysis_tag_reviews"] {
        let query = format!("SELECT COUNT(*) FROM {table}");
        assert_eq!(sqlx::query_scalar::<_, i64>(AssertSqlSafe(query)).fetch_one(&storage.pool).await?, 0);
    }
    sqlx::query("DROP TRIGGER fail_second_review").execute(&storage.pool).await?;
    let retry = AssistantRepository::review_analysis(
        storage.as_ref(), &targets, AnalysisReviewDecision::Accepted, None,
    ).await?;
    assert_eq!(retry.applied.len(), 2);
    assert!(retry.failures.is_empty());
    assert!(AssistantRepository::tracks(storage.as_ref()).await?.iter().all(|evidence| evidence.manual_tags == ["calm"]));
    Ok(())
}

#[tokio::test]
async fn review_summary_preserves_denominators_and_tracks_real_decisions()
-> Result<(), Box<dyn Error + Send + Sync>> {
    use music_application::assistant::{
        AnalysisWrite, ContextScope, LocalAnalysisRepository, ManualTagQuery,
        TagReviewCounts,
    };
    use std::sync::Arc;
    let (_directory, storage) = storage().await?;
    sqlx::query("UPDATE tracks SET path = 'Sub/b.mp3' WHERE id = 2")
        .execute(&storage.pool)
        .await?;
    music_application::cleanup_sources::CleanupSourceRepository::set_cleanup_source_enabled(&storage, "lastfm", true).await?;
    let storage = Arc::new(storage);
    let service = AssistantService::new(storage.clone());
    let tracks = service.tracks().await?;
    let mut targets = Vec::new();
    for evidence in &tracks {
        let revision = music_application::cleanup_enrichment::CleanupEnrichmentRepository::catalog_evidence_revision(storage.as_ref()).await?;
        let source_signature = music_application::assistant::catalog_tag_source_signature(&evidence.track, revision)?;
        assert_eq!(
            storage
                .store_catalog_analysis(
                    CATALOG_TAG_ANALYZER_ID,
                    "fixture",
                    &[AnalysisWrite {
                        track_id: evidence.track.id,
                        source_signature: source_signature.clone(),
                        decisions: review_fixture_decisions(&["calm"]),
                moods: vec!["calm".to_owned()],
                        evidence: vec!["Synthetic metadata".to_owned()],
                        metrics: serde_json::json!({"evidence_revision":revision}).as_object().cloned().ok_or("metrics")?,
                    }]
                )
                .await?,
            1
        );
        targets.push(AnalysisReviewTarget {
            track_id: evidence.track.id,
            tag: "calm".to_owned(),
            analyzer_id: CATALOG_TAG_ANALYZER_ID.to_owned(),
            source_signature,
        });
    }
    service
        .patch_tags(&[tracks[0].track.id], &["calm".to_owned()], &[])
        .await?;
    let initial = service.tag_page(ManualTagQuery::default()).await?;
    assert_eq!(
        initial.review_summary.sources[CATALOG_TAG_ANALYZER_ID].pending,
        2
    );
    for (target, decision) in targets.iter().zip([
        AnalysisReviewDecision::Accepted,
        AnalysisReviewDecision::Rejected,
    ]) {
        assert_eq!(
            service
                .review_analysis(std::slice::from_ref(target), decision)
                .await?
                .applied
                .len(),
            1
        );
    }
    let expected = service
        .tag_page(ManualTagQuery::default())
        .await?
        .review_summary;
    assert_eq!(expected.matching_tracks, 2);
    assert_eq!(
        expected.sources[CATALOG_TAG_ANALYZER_ID],
        TagReviewCounts {
            pending: 0,
            accepted: 1,
            rejected: 1
        }
    );
    for review in [
        None,
        Some(AnalysisReviewDecision::Pending),
        Some(AnalysisReviewDecision::Accepted),
        Some(AnalysisReviewDecision::Rejected),
    ] {
        for offset in [0, 1, 50] {
            let page = service
                .tag_page(ManualTagQuery {
                    review,
                    offset,
                    limit: 1,
                    ..ManualTagQuery::default()
                })
                .await?;
            assert_eq!(page.review_summary, expected);
            assert_eq!(
                page.total,
                match review {
                    None => 2,
                    Some(AnalysisReviewDecision::Pending) => 0,
                    _ => 1,
                }
            );
            assert!(page.items.len() <= 1);
            if offset >= page.total {
                assert!(page.items.is_empty());
            }
        }
    }
    for query in [
        ManualTagQuery {
            search: "A.mp3".to_owned(),
            ..ManualTagQuery::default()
        },
        ManualTagQuery {
            tag: Some("calm".to_owned()),
            ..ManualTagQuery::default()
        },
        ManualTagQuery {
            scope: Some(ContextScope::Folder {
                path: None,
                recursive: false,
            }),
            ..ManualTagQuery::default()
        },
        ManualTagQuery {
            scope: Some(ContextScope::Tracks(vec![tracks[0].track.id])),
            ..ManualTagQuery::default()
        },
    ] {
        let summary = service.tag_page(query).await?.review_summary;
        assert_eq!(summary.matching_tracks, 1);
        assert_eq!(summary.sources[CATALOG_TAG_ANALYZER_ID].accepted, 1);
        assert_eq!(summary.sources[CATALOG_TAG_ANALYZER_ID].rejected, 0);
    }
    let empty = service
        .tag_page(ManualTagQuery {
            search: "absent".to_owned(),
            ..ManualTagQuery::default()
        })
        .await?;
    assert_eq!(empty.review_summary.matching_tracks, 0);
    assert!(empty.review_summary.sources.is_empty());
    let other_analyzer = service
        .tag_page(ManualTagQuery {
            analyzer_ids: Some(vec![MODEL_TAG_ANALYZER_ID.to_owned()]),
            ..ManualTagQuery::default()
        })
        .await?;
    assert_eq!(other_analyzer.review_summary.matching_tracks, 2);
    assert!(other_analyzer.review_summary.sources.is_empty());
    service
        .review_analysis(&targets[..1], AnalysisReviewDecision::Pending)
        .await?;
    sqlx::query("UPDATE tracks SET title = 'Changed' WHERE id = 2")
        .execute(&storage.pool)
        .await?;
    let reopened = service.tag_page(ManualTagQuery::default()).await?;
    assert_eq!(
        reopened.review_summary.sources[CATALOG_TAG_ANALYZER_ID],
        TagReviewCounts {
            pending: 1,
            accepted: 0,
            rejected: 0
        }
    );
    assert_eq!(reopened.items[0].manual_tags, vec!["calm"]);
    Ok(())
}

#[tokio::test]
async fn model_review_rechecks_configuration_and_evidence_inside_the_transaction()
-> Result<(), Box<dyn Error + Send + Sync>> {
    use music_application::assistant::{
        AnalysisWrite, LocalAnalysisRepository, ModelAnalysisWrite,
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
        "missing-reference",
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
                inference_fingerprint: "a".repeat(64),
                runtime_fingerprint: "a".repeat(64),
                configuration_fingerprint: role.configuration_fingerprint(),
                connection_fingerprint: connection.fingerprint(),
            },
            vocabulary_fingerprint: vocabulary.fingerprint.clone(),
            voice_signature: None,
        };
        let signature = model_tag_source_signature(
            &track,
            &guard.role.inference_fingerprint,
            &vocabulary.fingerprint,
            None,
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
                    &guard.role.inference_fingerprint,
                    &vocabulary.fingerprint,
                    None,
                    &[ModelAnalysisWrite {
                        profile: AnalysisWrite {
                            track_id: track.id,
                            source_signature: signature,
                            decisions: review_fixture_decisions(&["calm"]),
                moods: vec!["calm".to_owned()],
                            evidence: vec!["Synthetic evidence".to_owned()],
                            metrics:
                                serde_json::json!({"contract":"assistant-music-tagger-output/v5","input_snapshot":music_application::assistant::model_tag_track_input(&track,None,None)})
                                    .as_object()
                                    .cloned()
                                    .ok_or("metrics missing")?,
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
                sqlx::query("UPDATE track_analyses SET decisions_json = '[]'")
                    .execute(&storage.pool)
                    .await?;
            }
            "missing-reference" => {
                sqlx::query("UPDATE track_analyses SET decisions_json = json_set(decisions_json, '$[0].evidence_ids[0]', 'audio.missing')")
                    .execute(&storage.pool).await?;
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
