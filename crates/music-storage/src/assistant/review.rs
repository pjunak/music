//! Transactional freshness checks and execution of explicit tag-review decisions.
//!
//! Planning never writes. All reads and writes here share the caller's transaction
//! and write admission, including mutable model identity and vocabulary checks.
use super::*;

mod plan;

pub(super) async fn review_in_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    targets: &[AnalysisReviewTarget],
    decision: AnalysisReviewDecision,
    model_guard: Option<&ModelTagReviewGuard>,
) -> Result<AnalysisReviewBatch, AssistantDependencyErrorAlias> {
    let mut valid = Vec::<AnalysisReviewTarget>::new();
    let mut failures = Vec::new();
    // Recheck mutable model configuration inside the same transaction as
    // manual-tag acceptance. A preflight read alone is insufficient.
    let model_vocabulary = match model_guard {
        Some(guard) => current_review_vocabulary(transaction, guard).await?,
        None => None,
    };
    for target in targets {
        let Some(track) = load_track(transaction, target.track_id).await? else {
            failures.push(review_failure(
                target,
                AnalysisReviewFailureCode::NotFound,
                "Track not found",
            ));
            continue;
        };
        if !matches!(
            target.analyzer_id.as_str(),
            LOCAL_METADATA_ANALYZER_ID | CATALOG_TAG_ANALYZER_ID | MODEL_TAG_ANALYZER_ID
        ) {
            failures.push(review_failure(
                target,
                AnalysisReviewFailureCode::NotFound,
                "Analysis profile not found",
            ));
            continue;
        }
        let row = sqlx::query(
            "SELECT analyzer_id, source_signature, moods_json, evidence_json, metrics_json, \
             energy, brightness, tension, confidence, job_id, \
             CAST(strftime('%s', updated_at) AS INTEGER) AS updated_at_unix_seconds FROM track_analyses \
             WHERE track_id = ? AND analyzer_id = ?",
        )
        .bind(target.track_id.get())
        .bind(&target.analyzer_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(box_storage)?;
        let Some(row) = row else {
            failures.push(review_failure(
                target,
                AnalysisReviewFailureCode::NotFound,
                "Analysis profile not found",
            ));
            continue;
        };
        let stored_signature: String = row.try_get("source_signature").map_err(box_storage)?;
        if stored_signature != target.source_signature {
            failures.push(review_failure(
                target,
                AnalysisReviewFailureCode::Stale,
                "Analysis changed; refresh before reviewing this tag",
            ));
            continue;
        }
        let current_signature = if target.analyzer_id == MODEL_TAG_ANALYZER_ID {
            let Some((guard, vocabulary)) = model_guard.zip(model_vocabulary.as_ref()) else {
                failures.push(review_failure(
                    target,
                    AnalysisReviewFailureCode::Stale,
                    "Model settings or vocabulary changed; refresh before reviewing",
                ));
                continue;
            };
            if !vocabulary
                .groups
                .iter()
                .flat_map(|group| &group.tags)
                .any(|tag| tag.name == target.tag)
            {
                failures.push(review_failure(
                    target,
                    AnalysisReviewFailureCode::Stale,
                    "Tag is no longer in the model vocabulary",
                ));
                continue;
            }
            let context = crate::analysis::current_model_context(
                transaction,
                &track,
                guard.voice_signature.as_deref(),
            )
            .await?;
            music_application::assistant::model_tag_source_signature(
                &track,
                &guard.role.inference_fingerprint,
                &guard.vocabulary_fingerprint,
                context.as_ref(),
            )
        } else if target.analyzer_id == CATALOG_TAG_ANALYZER_ID {
            music_application::assistant::catalog_tag_source_signature(
                &track,
                crate::catalog_evidence::revision(transaction)
                    .await
                    .map_err(box_storage)?,
            )
        } else {
            metadata_source_signature(&track)
        }
        .map_err(|_| {
            box_storage(StorageError::InvalidAssistantRecord(
                "invalid source signature",
            ))
        })?;
        if current_signature != target.source_signature {
            failures.push(review_failure(
                target,
                AnalysisReviewFailureCode::Stale,
                "Track or analyzer settings changed; rerun analysis before reviewing this tag",
            ));
            continue;
        }
        if target.analyzer_id == MODEL_TAG_ANALYZER_ID
            && !analysis_from_row(&row).is_some_and(|profile| {
                music_application::assistant::model_tag_profile_is_current(
                    &profile,
                    &current_signature,
                )
            })
        {
            failures.push(review_failure(
                target,
                AnalysisReviewFailureCode::Stale,
                "Model profile is invalid; regenerate suggestions before reviewing",
            ));
            continue;
        }
        let moods: Vec<String> =
            serde_json::from_str(row.try_get::<&str, _>("moods_json").map_err(box_storage)?)
                .map_err(StorageError::AssistantSerialization)
                .map_err(box_storage)?;
        if !moods.iter().any(|tag| tag == &target.tag) {
            failures.push(review_failure(
                target,
                AnalysisReviewFailureCode::NotFound,
                "Tag is not present in the current analysis profile",
            ));
            continue;
        }
        valid.push(target.clone());
    }
    let track_ids = valid
        .iter()
        .map(|target| target.track_id.get())
        .collect::<BTreeSet<_>>();
    let manual = load_manual_tags(transaction, &track_ids).await?;
    let plan = plan::plan_decisions(valid, decision, manual);
    failures.extend(plan.failures);
    let mut applied = Vec::new();
    for planned in plan.operations {
        let target = planned.target;
        match decision {
            AnalysisReviewDecision::Pending => {
                sqlx::query(
                    "DELETE FROM track_analysis_tag_reviews \
                     WHERE track_id = ? AND analyzer_id = ? AND tag = ?",
                )
                .bind(target.track_id.get())
                .bind(&target.analyzer_id)
                .bind(&target.tag)
                .execute(&mut **transaction)
                .await
                .map_err(box_storage)?;
            }
            AnalysisReviewDecision::Accepted | AnalysisReviewDecision::Rejected => {
                if decision == AnalysisReviewDecision::Accepted && planned.insert_manual_tag {
                    sqlx::query(
                        "INSERT INTO track_user_tags (track_id, tag, created_at) \
                         VALUES (?, ?, CURRENT_TIMESTAMP)",
                    )
                    .bind(target.track_id.get())
                    .bind(&target.tag)
                    .execute(&mut **transaction)
                    .await
                    .map_err(box_storage)?;
                }
                sqlx::query(
                    "INSERT INTO track_analysis_tag_reviews \
                     (track_id, analyzer_id, tag, source_signature, decision, reviewed_at) \
                     VALUES (?, ?, ?, ?, ?, CURRENT_TIMESTAMP) \
                     ON CONFLICT(track_id, analyzer_id, tag) DO UPDATE SET \
                       source_signature = excluded.source_signature, \
                       decision = excluded.decision, reviewed_at = excluded.reviewed_at",
                )
                .bind(target.track_id.get())
                .bind(&target.analyzer_id)
                .bind(&target.tag)
                .bind(&target.source_signature)
                .bind(decision.as_str())
                .execute(&mut **transaction)
                .await
                .map_err(box_storage)?;
            }
        }
        applied.push(AnalysisReviewOutcome {
            track_id: target.track_id,
            tag: target.tag,
            analyzer_id: target.analyzer_id,
            source_signature: target.source_signature,
            decision,
        });
    }
    Ok(AnalysisReviewBatch {
        requested_items: targets.len(),
        applied,
        failures,
    })
}
