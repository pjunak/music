use super::*;

impl ModelEvaluationJobHandler {
    pub(super) async fn execute_tagging(
        &self,
        context: &JobExecutionContext,
        parameters: &ModelEvaluationJobParameters,
    ) -> Result<Map<String, Value>, JobHandlerError> {
        let suite = tag_quality_suite().map_err(model_task_failure)?;
        let retest = !parameters.case_ids.is_empty();
        let execution_cases = if retest {
            let requested = parameters.case_ids.iter().collect::<BTreeSet<_>>();
            let selected = suite
                .cases
                .iter()
                .filter(|case| requested.contains(&case.id))
                .cloned()
                .collect::<Vec<_>>();
            if selected.len() != requested.len() {
                return Err(JobHandlerError::new("evaluation_retest_baseline_stale"));
            }
            selected
        } else {
            suite.cases.clone()
        };
        let baseline = if retest {
            Some(load_tagging_baseline(context, parameters, &suite).await?)
        } else {
            None
        };
        let safety_count = execution_cases
            .iter()
            .filter(|case| case.gate == TagQualityGate::Safety)
            .count();
        let total_attempts = execution_cases.len().saturating_add(safety_count);
        update_progress(
            context,
            0,
            total_attempts,
            "Preparing evaluation",
            format!(
                "Loading {} {} tagging scenarios; {} safety reruns make {} scored attempts",
                execution_cases.len(),
                if retest { "failed" } else { "fixed" },
                safety_count,
                total_attempts,
            ),
        )
        .await?;
        let execution = self.prepare(parameters).await?;
        let vocabulary = default_vocabulary_snapshot().map_err(model_task_failure)?;
        let mut planned_requests = 0;
        for safety_only in [false, true] {
            let inputs = execution_cases
                .iter()
                .filter(|case| !safety_only || case.gate == TagQualityGate::Safety)
                .map(|case| case.track.clone())
                .collect::<Vec<_>>();
            planned_requests +=
                crate::assistant::plan_model_tagger_batches(&inputs, &vocabulary, |request| {
                    self.transport
                        .validate_request(&execution.role.execution, request)
                })
                .map_err(model_task_failure)?
                .len();
        }
        let max_attempts = tagging_attempt_budget(planned_requests);
        let mut usage =
            start_evaluation_run(context, &execution.role, parameters, max_attempts).await?;
        let mut retry_budget = MODEL_TAGGER_INVALID_RESPONSE_RETRY_LIMIT;
        let mut completed = 0_usize;
        let mut deterministic_execution_failure = None;
        let results = self
            .evaluate_tagging_cases(
                context,
                &execution.role,
                &execution_cases,
                &vocabulary,
                &mut usage,
                &mut retry_budget,
                &mut completed,
                total_attempts,
                execution_cases.len(),
                &mut deterministic_execution_failure,
            )
            .await?;
        let safety_cases = execution_cases
            .iter()
            .filter(|case| case.gate == TagQualityGate::Safety)
            .cloned()
            .collect::<Vec<_>>();
        let repeats = self
            .evaluate_tagging_cases(
                context,
                &execution.role,
                &safety_cases,
                &vocabulary,
                &mut usage,
                &mut retry_budget,
                &mut completed,
                total_attempts,
                execution_cases.len(),
                &mut deterministic_execution_failure,
            )
            .await?;
        let evaluated = merge_safety_repeats(results, repeats).map_err(model_task_failure)?;
        let merged = match baseline {
            Some(baseline) => merge_tagging_retest(baseline, evaluated)?,
            None => evaluated,
        };
        let result =
            TagQualityEvaluationResult::summarize(&suite, merged).map_err(model_task_failure)?;
        context
            .check_cancelled()
            .await
            .map_err(JobHandlerError::from_execution)?;
        if !retest {
            self.quality
                .record_evaluation(
                    &execution,
                    context.job_id(),
                    MODEL_TAG_ANALYZER_ID,
                    result.passed,
                    result.passed_cases,
                    result.total_cases,
                )
                .await
                .map_err(|error| JobHandlerError::new(error.code()))?;
        }
        quality_result(
            parameters,
            if retest {
                "diagnostic_retest"
            } else {
                "full_suite"
            },
            &result,
            &usage,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn evaluate_tagging_cases(
        &self,
        context: &JobExecutionContext,
        role: &ResolvedRoleExecution,
        cases: &[TagQualityCase],
        vocabulary: &crate::assistant::TagVocabularySnapshot,
        usage: &mut ProviderUsageAccumulator,
        retry_budget: &mut u8,
        completed: &mut usize,
        total_attempts: usize,
        scenario_count: usize,
        deterministic_execution_failure: &mut Option<ModelTaskError>,
    ) -> Result<Vec<TagQualityCaseResult>, JobHandlerError> {
        let mut results = Vec::with_capacity(cases.len());
        let inputs = cases
            .iter()
            .map(|case| case.track.clone())
            .collect::<Vec<_>>();
        let batches = crate::assistant::plan_model_tagger_batches(&inputs, vocabulary, |request| {
            self.transport.validate_request(&role.execution, request)
        })
        .map_err(model_task_failure)?;
        for planned in batches {
            let chunk = &cases[planned.input_range];
            let batch = planned.task;
            let profiles = if let Some(error) = deterministic_execution_failure.clone() {
                Err(error)
            } else {
                let mut correction = false;
                loop {
                    let model_result = self
                        .execute_model(context, role, &batch.request(correction), usage)
                        .await?;
                    match batch.finish(model_result) {
                        Ok(profiles) => break Ok(profiles),
                        Err(error) if retryable_tagger_error(&error) && *retry_budget > 0 => {
                            *retry_budget = retry_budget.saturating_sub(1);
                            correction = true;
                        }
                        Err(error) => break Err(error),
                    }
                }
            };
            if let Err(error) = &profiles
                && deterministic_tagger_execution_failure(error)
            {
                *deterministic_execution_failure = Some(error.clone());
            }
            for case in chunk {
                let result = match &profiles {
                    Ok(profiles) => {
                        let track_id = case
                            .track
                            .get("track_id")
                            .and_then(Value::as_i64)
                            .ok_or_else(|| JobHandlerError::new("invalid tagging suite track"))?;
                        match profiles.get(&track_id) {
                            Some(profile) => case.assess(Ok(profile), vocabulary),
                            None => {
                                let error = ModelTaskError::new("model_output_track_set_mismatch");
                                case.assess(Err(&error), vocabulary)
                            }
                        }
                    }
                    Err(error) => case.assess(Err(error), vocabulary),
                };
                results.push(result);
                *completed = completed.saturating_add(1);
                update_progress(
                    context,
                    *completed,
                    total_attempts,
                    "Evaluating tagging model",
                    format!(
                        "Completed {} of {} scored attempts across {} scenarios",
                        *completed, total_attempts, scenario_count,
                    ),
                )
                .await?;
            }
        }
        Ok(results)
    }
}

impl ModelFeatureJobHandler {
    pub(super) async fn execute_tagging(
        &self,
        context: &JobExecutionContext,
        parameters: ModelTaggingJobParameters,
    ) -> Result<Value, JobHandlerError> {
        validate_feature_header(
            &parameters.role_id,
            "music_tagger",
            &parameters.quality_evaluation_id,
            TAGGING_QUALITY_EVALUATION_ID,
            &parameters.disclosure_version,
            "assistant-model-music-tagging-disclosure/v11",
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
        let vocabulary = self
            .assistant
            .vocabulary()
            .await
            .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?;
        if vocabulary.fingerprint != parameters.vocabulary_fingerprint {
            return Err(JobHandlerError::new("tag_vocabulary_changed"));
        }
        let tracks = self
            .assistant
            .tracks()
            .await
            .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?;
        let library_tracks = tracks.len();
        let scope = parameters.scope.application_scope()?;
        let scoped = tracks
            .iter()
            .filter(|track| scope.contains(&track.track))
            .collect::<Vec<_>>();
        let indexed = scoped
            .iter()
            .map(|track| track.track.clone())
            .collect::<Vec<_>>();
        let contexts = self
            .local_analysis
            .current_contexts(&indexed)
            .await
            .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?;
        let planned = scoped
            .iter()
            .copied()
            .filter(|track| {
                parameters.context_policy == ModelTaggingContextPolicy::Include
                    || contexts
                        .get(&track.track.id)
                        .is_some_and(|context| context.completeness == "full")
            })
            .collect::<Vec<_>>();
        let skipped_context_tracks = scoped.len().saturating_sub(planned.len());
        let signatures = planned
            .iter()
            .map(|track| {
                model_tag_source_signature(
                    &track.track,
                    &parameters.role_fingerprint,
                    &parameters.vocabulary_fingerprint,
                    contexts.get(&track.track.id),
                )
                .map(|signature| (track.track.id, signature))
                .map_err(|_| JobHandlerError::new("model_tag_source_invalid"))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let work = planned
            .iter()
            .copied()
            .filter(|track| {
                parameters.force
                    || !track.analyses.iter().any(|analysis| {
                        analysis.analyzer_id == MODEL_TAG_ANALYZER_ID
                            && signatures
                                .get(&track.track.id)
                                .is_some_and(|signature| analysis.source_signature == *signature)
                    })
            })
            .collect::<Vec<_>>();
        let total = work.len();
        let inputs = work
            .iter()
            .map(|track| model_tag_track_input(&track.track, contexts.get(&track.track.id)))
            .collect::<Vec<_>>();
        let batches =
            crate::assistant::plan_model_tagger_batches(&inputs, &vocabulary, |request| {
                self.transport.validate_request(&role.execution, request)
            })
            .map_err(model_task_failure)?;
        update_progress(
            context,
            0,
            total,
            "Preparing metadata batches",
            if total == 0 {
                "All model tag suggestions are current".to_owned()
            } else {
                format!(
                    "{total} of {} tracks need model tag suggestions",
                    planned.len()
                )
            },
        )
        .await?;
        let mut updated = 0_usize;
        let mut skipped_changed = 0_usize;
        let mut provider_usage = start_model_run(
            context,
            &role,
            &parameters.quality_evaluation_id,
            Some(&parameters.disclosure_version),
            &parameters,
            &signatures
                .iter()
                .map(|(id, signature)| (id.get(), signature))
                .collect::<Vec<_>>(),
            tagging_attempt_budget(batches.len()),
            ModelReviewDestination::TrackTagReview,
        )
        .await?;
        let mut retry_budget = MODEL_TAGGER_INVALID_RESPONSE_RETRY_LIMIT;
        for planned in batches {
            let start = planned.input_range.start;
            let batch = &work[planned.input_range];
            let task = planned.task;
            ensure_feature_role_unchanged(
                &self.quality,
                &parameters.role_id,
                &parameters.quality_evaluation_id,
                &parameters.role_fingerprint,
            )
            .await?;
            ensure_vocabulary_unchanged(&self.assistant, &parameters.vocabulary_fingerprint)
                .await?;
            update_progress(
                context,
                start,
                total,
                "Waiting for mood-tagging model",
                format!(
                    "Classifying tracks {}-{} of {total}",
                    start + 1,
                    start + batch.len()
                ),
            )
            .await?;
            let mut correction = false;
            let profiles = loop {
                let result = execute_provider_request(
                    context,
                    self.transport.as_ref(),
                    &role,
                    &task.request(correction),
                    &mut provider_usage,
                )
                .await?;
                match task.finish(result) {
                    Ok(profiles) => break profiles,
                    Err(error) if retryable_tagger_error(&error) && retry_budget > 0 => {
                        retry_budget = retry_budget.saturating_sub(1);
                        correction = true;
                    }
                    Err(error) => return Err(model_task_failure(error)),
                }
            };
            ensure_feature_role_unchanged(
                &self.quality,
                &parameters.role_id,
                &parameters.quality_evaluation_id,
                &parameters.role_fingerprint,
            )
            .await?;
            ensure_vocabulary_unchanged(&self.assistant, &parameters.vocabulary_fingerprint)
                .await?;
            let writes = batch
                .iter()
                .map(|track| {
                    let model = profiles
                        .get(&track.track.id.get())
                        .ok_or_else(|| JobHandlerError::new("model_output_track_set_mismatch"))?;
                    let (energy, brightness, tension) =
                        local_context_axes(contexts.get(&track.track.id));
                    let context_status = contexts
                        .get(&track.track.id)
                        .map(|context| context.completeness.as_str())
                        .unwrap_or("missing");
                    let confidence = match model.confidence {
                        TagConfidence::High => Confidence::High,
                        TagConfidence::Medium => Confidence::Medium,
                        TagConfidence::Low => Confidence::Low,
                    };
                    Ok(ModelAnalysisWrite {
                        profile: AnalysisWrite {
                            track_id: track.track.id,
                            source_signature: signatures
                                .get(&track.track.id)
                                .cloned()
                                .ok_or_else(|| JobHandlerError::new("model_tag_source_invalid"))?,
                            energy,
                            brightness,
                            tension,
                            moods: model.tags.clone(),
                            evidence: model.evidence.clone(),
                            metrics: json!({
                                "contract": "assistant-music-tagger-output/v3",
                                "input_contract": MODEL_TAGGER_INPUT_CONTRACT,
                                "context_status": context_status,
                                "role_fingerprint": parameters.role_fingerprint,
                                "vocabulary_fingerprint": parameters.vocabulary_fingerprint,
                            })
                            .as_object()
                            .cloned()
                            .ok_or_else(|| JobHandlerError::new("model_tag_profile_invalid"))?,
                            confidence,
                        },
                    })
                })
                .collect::<Result<Vec<_>, JobHandlerError>>()?;
            let stored = self
                .analysis_repository
                .store_model_analysis(
                    MODEL_TAG_ANALYZER_ID,
                    context.job_id(),
                    &parameters.role_fingerprint,
                    &parameters.vocabulary_fingerprint,
                    self.local_analysis
                        .voice_analyzer()
                        .source_signature
                        .as_deref(),
                    &writes,
                )
                .await
                .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?;
            updated = updated.saturating_add(stored);
            skipped_changed = skipped_changed.saturating_add(batch.len().saturating_sub(stored));
            update_progress(
                context,
                (start + batch.len()).min(total),
                total,
                "Saving reviewable suggestions",
                format!("Processed {} of {total} tracks", start + batch.len()),
            )
            .await?;
        }
        Ok(json!({
            "schema_version": "assistant-model-music-tagging-job-result/v6",
            "disclosure_version": parameters.disclosure_version,
            "role_id": parameters.role_id,
            "role_fingerprint": parameters.role_fingerprint,
            "analyzer_id": MODEL_TAG_ANALYZER_ID,
            "vocabulary_fingerprint": parameters.vocabulary_fingerprint,
            "library_tracks": library_tracks,
            "scope": parameters.scope,
            "scope_tracks": scoped.len(),
            "context_policy": parameters.context_policy,
            "skipped_context_tracks": skipped_context_tracks,
            "updated_profiles": updated,
            "unchanged_profiles": planned.len().saturating_sub(work.len()),
            "skipped_changed_tracks": skipped_changed,
            "usage": provider_usage.summary(),
        }))
    }
}
