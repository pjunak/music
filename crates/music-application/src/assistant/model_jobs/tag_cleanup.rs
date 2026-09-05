use super::*;

impl ModelEvaluationJobHandler {
    pub(super) async fn execute_tag_cleanup(
        &self,
        context: &JobExecutionContext,
        parameters: &ModelEvaluationJobParameters,
    ) -> Result<Map<String, Value>, JobHandlerError> {
        let suite = tag_cleanup_quality_suite().map_err(model_task_failure)?;
        update_progress(
            context,
            0,
            suite.cases.len(),
            "Preparing evaluation",
            "Loading fixed synthetic tag-cleanup cases",
        )
        .await?;
        let execution = self.prepare(parameters).await?;
        let vocabulary = default_vocabulary_snapshot().map_err(model_task_failure)?;
        let mut usage = ProviderUsageAccumulator::default();
        let mut results = Vec::with_capacity(suite.cases.len());
        for (index, case) in suite.cases.iter().enumerate() {
            let mut task = ModelTagCleanupTask::new(&case.usage(), vocabulary.clone())
                .map_err(model_task_failure)?;
            let mut failure = None;
            while let Some(request) = task.next_request() {
                let model_result = self
                    .execute_model(context, &execution.role, &request, &mut usage)
                    .await?;
                if let Err(error) = task.accept(model_result) {
                    failure = Some(error);
                    break;
                }
            }
            let result = match failure {
                Some(error) => Err(error),
                None => task
                    .finish()
                    .ok_or_else(|| ModelTaskError::new("model_cleanup_incomplete")),
            };
            results.push(case.assess(result));
            update_progress(
                context,
                index + 1,
                suite.cases.len(),
                "Evaluating tag cleanup model",
                format!(
                    "Completed {} of {} synthetic cases",
                    index + 1,
                    suite.cases.len()
                ),
            )
            .await?;
        }
        let result = TagCleanupQualityEvaluationResult::from_cases(&suite, results);
        context
            .check_cancelled()
            .await
            .map_err(JobHandlerError::from_execution)?;
        self.quality
            .record_evaluation(
                &execution,
                context.job_id(),
                MODEL_TAG_CLEANUP_ENGINE_ID,
                result.passed,
                result.passed_cases,
                result.total_cases,
            )
            .await
            .map_err(|error| JobHandlerError::new(error.code()))?;
        quality_result(parameters, "full_suite", &result, &usage)
    }
}

impl ModelFeatureJobHandler {
    pub(super) async fn execute_tag_cleanup(
        &self,
        context: &JobExecutionContext,
        parameters: ModelTagCleanupJobParameters,
    ) -> Result<Value, JobHandlerError> {
        validate_feature_header(
            &parameters.role_id,
            "tag_cleanup",
            &parameters.quality_evaluation_id,
            TAG_CLEANUP_QUALITY_EVALUATION_ID,
            &parameters.disclosure_version,
            "assistant-model-tag-cleanup-disclosure/v3",
            parameters.consent,
            &parameters.role_fingerprint,
        )?;
        let role = self
            .quality
            .prepare_quality_gated_role_execution(
                &parameters.role_id,
                &parameters.quality_evaluation_id,
            )
            .await
            .map_err(|error| JobHandlerError::new(error.code()))?;
        if role.fingerprint != parameters.role_fingerprint {
            return Err(JobHandlerError::new("role_changed"));
        }
        let usage_snapshot = self
            .assistant
            .tag_usage()
            .await
            .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?;
        let vocabulary = self
            .assistant
            .vocabulary()
            .await
            .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?;
        let current_catalog_signature = catalog_signature(&usage_snapshot)
            .map_err(|_| JobHandlerError::new("tag_catalog_invalid"))?;
        if current_catalog_signature != parameters.catalog_signature {
            return Err(JobHandlerError::new("tag_catalog_changed"));
        }
        if vocabulary.fingerprint != parameters.vocabulary_fingerprint {
            return Err(JobHandlerError::new("tag_vocabulary_changed"));
        }
        if usage_snapshot.is_empty() {
            return Err(JobHandlerError::new("tag_catalog_empty"));
        }
        if usage_snapshot.len() > MAX_MODEL_CLEANUP_TAGS {
            return Err(JobHandlerError::new("tag_catalog_too_large"));
        }
        let mut task = ModelTagCleanupTask::new(&usage_snapshot, vocabulary.clone())
            .map_err(model_task_failure)?;
        let total_batches = task.total_model_batches();
        let progress_total = total_batches.max(1);
        update_progress(
            context,
            0,
            progress_total,
            if total_batches == 0 {
                "Applying deterministic cleanup rules"
            } else {
                "Waiting for tag cleanup model"
            },
            if total_batches == 0 {
                "All cleanup candidates were resolved locally".to_owned()
            } else {
                format!("Reviewing unresolved tag names in {total_batches} bounded batches")
            },
        )
        .await?;
        let mut provider_usage = ProviderUsageAccumulator::default();
        while let Some(request) = task.next_request() {
            ensure_feature_role_unchanged(
                &self.quality,
                &parameters.role_id,
                &parameters.quality_evaluation_id,
                &parameters.role_fingerprint,
            )
            .await?;
            ensure_vocabulary_unchanged(&self.assistant, &parameters.vocabulary_fingerprint)
                .await?;
            let result = execute_provider_request(
                context,
                self.transport.as_ref(),
                &role,
                &request,
                &mut provider_usage,
            )
            .await?;
            task.accept(result).map_err(model_task_failure)?;
            update_progress(
                context,
                task.completed_model_batches(),
                progress_total,
                "Waiting for tag cleanup model",
                format!(
                    "Completed {} of {} provider batches",
                    task.completed_model_batches(),
                    total_batches
                ),
            )
            .await?;
        }
        let suggestions = task
            .finish()
            .ok_or_else(|| JobHandlerError::new("model_cleanup_incomplete"))?;
        ensure_feature_role_unchanged(
            &self.quality,
            &parameters.role_id,
            &parameters.quality_evaluation_id,
            &parameters.role_fingerprint,
        )
        .await?;
        ensure_vocabulary_unchanged(&self.assistant, &parameters.vocabulary_fingerprint).await?;
        let final_usage = self
            .assistant
            .tag_usage()
            .await
            .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?;
        if catalog_signature(&final_usage)
            .map_err(|_| JobHandlerError::new("tag_catalog_invalid"))?
            != parameters.catalog_signature
        {
            return Err(JobHandlerError::new("tag_catalog_changed"));
        }
        let counts = usage_snapshot
            .iter()
            .map(|item| (item.tag.as_str(), item.track_count))
            .collect::<BTreeMap<_, _>>();
        let local_pairs = build_cleanup_preview(&usage_snapshot, &vocabulary)
            .map_err(|_| JobHandlerError::new("tag_cleanup_preview_failed"))?
            .suggestions
            .into_iter()
            .map(|item| (item.source, item.target))
            .collect::<BTreeSet<_>>();
        let output = suggestions
            .iter()
            .map(|suggestion| {
                json!({
                    "id": model_tag_cleanup_suggestion_id(
                        &parameters.role_fingerprint,
                        &parameters.catalog_signature,
                        &parameters.vocabulary_fingerprint,
                        suggestion,
                    ),
                    "source": suggestion.source,
                    "target": suggestion.target,
                    "origin": if local_pairs.contains(&(suggestion.source.clone(), suggestion.target.clone())) {
                        "local-rule"
                    } else {
                        "model"
                    },
                    "confidence": suggestion.confidence.as_str(),
                    "reason": suggestion.reason,
                    "source_track_count": counts.get(suggestion.source.as_str()).copied().unwrap_or(0),
                    "target_track_count": counts.get(suggestion.target.as_str()).copied().unwrap_or(0),
                    "merged": counts.contains_key(suggestion.target.as_str()),
                })
            })
            .collect::<Vec<_>>();
        update_progress(
            context,
            progress_total,
            progress_total,
            "Saving cleanup proposal",
            format!("Saved {} review-only suggestions", output.len()),
        )
        .await?;
        Ok(json!({
            "schema_version": "assistant-model-tag-cleanup-job-result/v3",
            "disclosure_version": parameters.disclosure_version,
            "role_id": parameters.role_id,
            "role_fingerprint": parameters.role_fingerprint,
            "engine_id": MODEL_TAG_CLEANUP_ENGINE_ID,
            "catalog_signature": parameters.catalog_signature,
            "vocabulary_fingerprint": parameters.vocabulary_fingerprint,
            "catalog_tags": usage_snapshot.len(),
            "suggestions": output,
            "usage": provider_usage.summary(),
        }))
    }
}
