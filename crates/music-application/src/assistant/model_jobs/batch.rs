use super::*;
use crate::assistant::{
    ModelBatchRecord, ModelTaggerBatch, PlannedTaggerBatch, ProviderUsageSummary,
};

impl ModelFeatureJobHandler {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn submit_tagging_batch(
        &self,
        context: &JobExecutionContext,
        parameters: &ModelTaggingJobParameters,
        role: &ResolvedRoleExecution,
        inputs: &[Value],
        batches: &[PlannedTaggerBatch],
        templates: Vec<AnalysisWrite>,
    ) -> Result<Value, JobHandlerError> {
        let services = self
            .batch
            .as_ref()
            .ok_or_else(|| JobHandlerError::new("batch_unavailable"))?;
        let requests = batches
            .iter()
            .map(|batch| batch.task.request(false))
            .collect::<Vec<_>>();
        services
            .transport
            .validate(&role.execution, &requests)
            .map_err(model_task_failure)?;
        if requests.len() > parameters.limits.max_requests {
            return Err(JobHandlerError::new("model_run_request_budget_exhausted"));
        }
        if parameters.limits.stop_on_empty_batch && requests.len() > 1 {
            return Err(JobHandlerError::new("batch_pilot_required"));
        }
        let mut usage = start_model_run(
            context,
            role,
            TAGGING_QUALITY_EVALUATION_ID,
            Some(&parameters.disclosure_version),
            parameters,
            &templates
                .iter()
                .map(|profile| &profile.source_signature)
                .collect::<Vec<_>>(),
            requests.len(),
            ModelReviewDestination::TrackTagReview,
        )
        .await?;
        usage.limit_token_reservation(parameters.limits.max_token_reservation);
        usage.reserve_batch(role, &requests)?;
        let mut record = ModelBatchRecord {
            id: context.job_id().to_owned(),
            connection_id: role.connection_id.clone(),
            state: "uploading".to_owned(),
            input_file_id: None,
            remote_batch_id: None,
            document: json!({"parameters":parameters,"inputs":inputs,"templates":templates,
                "ranges":batches.iter().map(|batch| batch.input_range.clone()).collect::<Vec<_>>(),
                "submission_usage":usage.summary()}),
        };
        context
            .check_cancelled()
            .await
            .map_err(JobHandlerError::from_execution)?;
        if !services
            .repository
            .create_model_batch(&record)
            .await
            .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?
        {
            return Err(JobHandlerError::new("model_batch_pending"));
        }
        // Failure at either network step leaves a durable uncertain state. A
        // generic job retry encounters the pending record and cannot resubmit.
        context
            .checkpoint(usage.checkpoint())
            .await
            .map_err(JobHandlerError::from_execution)?;
        record.input_file_id = Some(
            services
                .transport
                .upload(&role.execution, &requests)
                .await
                .map_err(model_task_failure)?,
        );
        persist(services, &mut record, "uploaded").await?;
        context
            .check_cancelled()
            .await
            .map_err(JobHandlerError::from_execution)?;
        persist(services, &mut record, "submitting").await?;
        record.remote_batch_id = Some(
            services
                .transport
                .submit(
                    &role.execution,
                    record
                        .input_file_id
                        .as_deref()
                        .ok_or_else(|| JobHandlerError::new("batch_missing_file"))?,
                    &record.id,
                )
                .await
                .map_err(model_task_failure)?,
        );
        persist(services, &mut record, "submitted").await?;
        Ok(
            json!({"schema_version":"assistant-model-batch-submitted/v1", "batch_id":record.id,
            "remote_batch_id":record.remote_batch_id, "state":record.state, "usage":usage.summary()}),
        )
    }

    pub(super) async fn collect_tagging_batch(
        &self,
        context: &JobExecutionContext,
        parameters: Map<String, Value>,
    ) -> Result<Value, JobHandlerError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Action {
            batch_id: String,
            role_id: String,
            #[serde(default)]
            cancel: bool,
            #[serde(default)]
            remote_batch_id: Option<String>,
            #[serde(default)]
            abandon_uncertain: bool,
        }
        let action: Action = serde_json::from_value(Value::Object(parameters))
            .map_err(|_| JobHandlerError::new("invalid_batch_action"))?;
        if action.role_id != "music_tagger" {
            return Err(JobHandlerError::new("invalid_batch_action"));
        }
        let services = self
            .batch
            .as_ref()
            .ok_or_else(|| JobHandlerError::new("batch_unavailable"))?;
        let mut record = services
            .repository
            .model_batch(&action.batch_id)
            .await
            .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?
            .ok_or_else(|| JobHandlerError::new("batch_not_found"))?;
        if !record.pending() {
            return Ok(record.document["result"].clone());
        }
        let target = services
            .providers
            .prepare_batch_management(&record.connection_id)
            .await
            .map_err(|error| JobHandlerError::new(error.code()))?;
        if record.remote_batch_id.is_none() {
            if record.state == "submitting" && action.cancel && action.abandon_uncertain {
                record.document = json!({"result":{"batch_id":record.id,"state":"cancelled","submission_still_uncertain":true,"usage":record.document["submission_usage"],
                    "note":"Operator abandoned the local record after checking the provider account; this does not cancel an unknown remote batch."}});
                persist(services, &mut record, "cancelled").await?;
                return Ok(record.document["result"].clone());
            } else if record.state == "submitting" {
                let remote = action
                    .remote_batch_id
                    .ok_or_else(|| JobHandlerError::new("batch_submission_uncertain_recover_id"))?;
                let status = services
                    .transport
                    .status(&target, &remote)
                    .await
                    .map_err(model_task_failure)?;
                if status.run_id != record.id
                    || Some(status.input_file_id.as_str()) != record.input_file_id.as_deref()
                {
                    return Err(JobHandlerError::new("batch_identity_mismatch"));
                }
                record.remote_batch_id = Some(remote);
                persist(services, &mut record, "submitted").await?;
            } else if action.cancel {
                if let Some(file) = &record.input_file_id {
                    services
                        .transport
                        .delete_file(&target, file)
                        .await
                        .map_err(model_task_failure)?;
                }
                record.document = json!({"result":{"batch_id":record.id,"state":"cancelled","no_inference_submitted":true}});
                persist(services, &mut record, "cancelled").await?;
                return Ok(record.document["result"].clone());
            } else {
                return Err(JobHandlerError::new(
                    "batch_upload_interrupted_cancel_before_retry",
                ));
            }
        }
        let remote = record
            .remote_batch_id
            .clone()
            .ok_or_else(|| JobHandlerError::new("batch_missing_id"))?;
        if record.state != "results_saved" {
            context
                .check_cancelled()
                .await
                .map_err(JobHandlerError::from_execution)?;
            let status = services
                .transport
                .status(&target, &remote)
                .await
                .map_err(model_task_failure)?;
            if status.run_id != record.id
                || Some(status.input_file_id.as_str()) != record.input_file_id.as_deref()
            {
                return Err(JobHandlerError::new("batch_identity_mismatch"));
            }
            if !matches!(
                status.state.as_str(),
                "completed" | "failed" | "expired" | "cancelled"
            ) {
                if action.cancel && status.state != "cancelling" {
                    services
                        .transport
                        .cancel(&target, &remote)
                        .await
                        .map_err(model_task_failure)?;
                }
                let state = if action.cancel {
                    "cancelling"
                } else {
                    status.state.as_str()
                };
                persist(services, &mut record, state).await?;
                return Ok(
                    json!({"batch_id":record.id,"state":record.state,"remote_batch_id":remote}),
                );
            }
            let parameters: ModelTaggingJobParameters =
                serde_json::from_value(record.document["parameters"].clone())
                    .map_err(|_| JobHandlerError::new("batch_record_invalid"))?;
            let inputs: Vec<Value> = serde_json::from_value(record.document["inputs"].clone())
                .map_err(|_| JobHandlerError::new("batch_record_invalid"))?;
            let templates: Vec<AnalysisWrite> =
                serde_json::from_value(record.document["templates"].clone())
                    .map_err(|_| JobHandlerError::new("batch_record_invalid"))?;
            let ranges: Vec<std::ops::Range<usize>> =
                serde_json::from_value(record.document["ranges"].clone())
                    .map_err(|_| JobHandlerError::new("batch_record_invalid"))?;
            let summary: ProviderUsageSummary =
                serde_json::from_value(record.document["submission_usage"].clone())
                    .map_err(|_| JobHandlerError::new("batch_record_invalid"))?;
            let mut usage = ProviderUsageAccumulator::resume(summary);
            let mut responses = BTreeMap::new();
            for file in [&status.output_file_id, &status.error_file_id]
                .into_iter()
                .flatten()
            {
                for item in services
                    .transport
                    .results(&target, file)
                    .await
                    .map_err(model_task_failure)?
                {
                    let index = item
                        .custom_id
                        .strip_prefix("request-")
                        .and_then(|id| id.parse::<usize>().ok())
                        .filter(|index| {
                            *index < ranges.len() && item.custom_id == format!("request-{index}")
                        })
                        .ok_or_else(|| JobHandlerError::new("batch_unknown_result"))?;
                    if responses.insert(index, item.result).is_some() {
                        return Err(JobHandlerError::new("batch_duplicate_result"));
                    }
                }
            }
            let vocabulary = self
                .assistant
                .vocabulary()
                .await
                .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?;
            // Collection cannot start new inference. Preserve paid results across
            // operational recertification while checking their inference identity.
            let current = services
                .providers
                .current_role_review_identity("music_tagger")
                .await
                .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?
                .is_some_and(|role| role.inference_fingerprint == parameters.inference_fingerprint)
                && vocabulary.fingerprint == parameters.vocabulary_fingerprint;
            let mut updated = 0;
            let mut rejected = 0;
            let mut processed = 0;
            let mut with_suggestions = 0;
            let mut suggested_tags = 0;
            let mut track_results = Vec::new();
            for (index, result) in responses {
                context
                    .check_cancelled()
                    .await
                    .map_err(JobHandlerError::from_execution)?;
                usage.observe_batch_result(index as u64 + 1, &result);
                context
                    .checkpoint(usage.checkpoint())
                    .await
                    .map_err(JobHandlerError::from_execution)?;
                let range = ranges[index].clone();
                let batch_inputs = inputs
                    .get(range.clone())
                    .ok_or_else(|| JobHandlerError::new("batch_record_invalid"))?;
                let task = ModelTaggerBatch::new(batch_inputs.to_vec(), vocabulary.clone())
                    .map_err(model_task_failure)?;
                let profiles = match task.finish(result) {
                    Ok(profiles) if current => profiles,
                    _ => {
                        rejected += range.len();
                        continue;
                    }
                };
                let mut writes = Vec::new();
                for template in templates
                    .get(range)
                    .ok_or_else(|| JobHandlerError::new("batch_record_invalid"))?
                {
                    let Some(model) = profiles.get(&template.track_id.get()) else {
                        return Err(JobHandlerError::new("model_output_track_set_mismatch"));
                    };
                    let mut profile = template.clone();
                    profile.moods = model.tags.clone();
                    profile.evidence = model.evidence.clone();
                    profile.decisions = model.decisions.clone();
                    processed += 1;
                    with_suggestions += usize::from(!profile.moods.is_empty());
                    suggested_tags += profile.moods.len();
                    track_results.push(json!({
                        "track_id": profile.track_id.get(), "source_signature": profile.source_signature,
                        "tags": profile.moods, "evidence": profile.evidence, "decisions": profile.decisions,
                    }));
                    writes.push(ModelAnalysisWrite { profile });
                }
                ensure_vocabulary_unchanged(&self.assistant, &parameters.vocabulary_fingerprint)
                    .await?;
                updated += self
                    .analysis_repository
                    .store_model_analysis(
                        MODEL_TAG_ANALYZER_ID,
                        &record.id,
                        &parameters.inference_fingerprint,
                        &parameters.vocabulary_fingerprint,
                        self.local_analysis
                            .voice_analyzer()
                            .source_signature
                            .as_deref(),
                        &writes,
                    )
                    .await
                    .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?;
                usage.set_feature_progress(json!({
                    "processed_tracks": processed, "tracks_with_suggestions": with_suggestions,
                    "tracks_without_suggestions": processed - with_suggestions, "suggested_tags": suggested_tags,
                    "updated_profiles": updated, "rejected_tracks": rejected, "track_results": track_results,
                }));
                context
                    .checkpoint(usage.checkpoint())
                    .await
                    .map_err(JobHandlerError::from_execution)?;
            }
            let result = json!({"schema_version":"assistant-model-batch-result/v2", "batch_id":record.id,"state":status.state,
                "updated_profiles":updated,"rejected_tracks":rejected,"unavailable_or_changed_tracks":templates.len().saturating_sub(updated + rejected),
                "processed_tracks":processed,"tracks_with_suggestions":with_suggestions,"tracks_without_suggestions":processed - with_suggestions,
                "suggested_tags":suggested_tags,"track_results":track_results,
                "stale_configuration":!current,"usage":usage.summary()});
            record.document = json!({"result":result,"terminal_state":status.state,"output_file_id":status.output_file_id,"error_file_id":status.error_file_id});
            persist(services, &mut record, "results_saved").await?;
        }
        // Results are durable before deletion, so a cleanup failure is retryable
        // without resubmitting or reapplying provider output.
        for file in [
            record.input_file_id.as_deref(),
            record.document["output_file_id"].as_str(),
            record.document["error_file_id"].as_str(),
        ]
        .into_iter()
        .flatten()
        {
            services
                .transport
                .delete_file(&target, file)
                .await
                .map_err(model_task_failure)?;
        }
        let terminal = record.document["terminal_state"]
            .as_str()
            .ok_or_else(|| JobHandlerError::new("batch_record_invalid"))?
            .to_owned();
        persist(services, &mut record, &terminal).await?;
        Ok(record.document["result"].clone())
    }
}

async fn persist(
    services: &crate::assistant::ModelBatchServices,
    record: &mut ModelBatchRecord,
    next: &str,
) -> Result<(), JobHandlerError> {
    let expected = record.state.clone();
    record.state = next.to_owned();
    if !services
        .repository
        .update_model_batch(&expected, record)
        .await
        .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?
    {
        return Err(JobHandlerError::new("batch_state_changed"));
    }
    Ok(())
}
