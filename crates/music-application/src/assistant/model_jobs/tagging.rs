use super::*;

struct TaggingQualityProgress {
    total_scenarios: usize,
    total_attempts: usize,
    completed_scenarios: usize,
    completed_attempts: usize,
    skipped_attempts: usize,
}

impl TaggingQualityProgress {
    fn new(cases: &[TagQualityCase]) -> Self {
        let safety_count = cases
            .iter()
            .filter(|case| case.gate == TagQualityGate::Safety)
            .count();
        Self {
            total_scenarios: cases.len(),
            total_attempts: cases.len() + safety_count,
            completed_scenarios: 0,
            completed_attempts: 0,
            skipped_attempts: 0,
        }
    }

    fn record(&mut self, gate: TagQualityGate, safety_repeat: bool, not_run: bool) {
        self.completed_attempts += 1;
        self.skipped_attempts += usize::from(not_run);
        // A safety scenario is complete only after both of its checks.
        if gate != TagQualityGate::Safety || safety_repeat {
            self.completed_scenarios += 1;
        }
    }

    fn message(&self) -> String {
        format!(
            "Processed {} of {} scenarios; {} of {} checks resolved including safety reruns; {} not run after an earlier failure",
            self.completed_scenarios,
            self.total_scenarios,
            self.completed_attempts,
            self.total_attempts,
            self.skipped_attempts,
        )
    }
}

fn not_run_after(error: &ModelTaskError) -> ModelTaskError {
    ModelTaskError {
        code: "model_evaluation_not_run".to_owned(),
        diagnostic: Some(format!("Not run after an earlier failure: {}", error.code)),
    }
}

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
        let mut progress = TaggingQualityProgress::new(&execution_cases);
        update_progress(
            context,
            0,
            progress.total_scenarios,
            "Preparing evaluation",
            format!(
                "Loading {} {} tagging scenarios; {} safety reruns are included in these scenarios",
                execution_cases.len(),
                if retest { "failed" } else { "fixed" },
                safety_count,
            ),
        )
        .await?;
        let execution = self.prepare(parameters).await?;
        let mut planned_requests = 0;
        for safety_only in [false, true] {
            let cases = execution_cases
                .iter()
                .filter(|case| !safety_only || case.gate == TagQualityGate::Safety)
                .cloned()
                .collect::<Vec<_>>();
            planned_requests += crate::assistant::plan_tag_quality_batches_for_adapter(
                &cases,
                &execution.role.execution.adapter_id,
                |request| {
                    self.transport
                        .validate_request(&execution.role.execution, request)
                },
            )
            .map_err(model_task_failure)?
            .iter()
            .map(|batch| {
                batch
                    .native_task
                    .as_ref()
                    .map_or(1, |task| task.max_requests)
            })
            .sum::<usize>();
        }
        let max_attempts =
            if execution.role.execution.adapter_id == crate::assistant::TYPESAFE_ADAPTER {
                planned_requests
            } else {
                tagging_attempt_budget(planned_requests)
            };
        let mut usage =
            start_evaluation_run(context, &execution.role, parameters, max_attempts).await?;
        let mut retry_budget = MODEL_TAGGER_INVALID_RESPONSE_RETRY_LIMIT;
        let mut deterministic_execution_failure = None;
        let results = self
            .evaluate_tagging_cases(
                context,
                &execution.role,
                &execution_cases,
                &mut usage,
                &mut retry_budget,
                &mut progress,
                false,
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
                &mut usage,
                &mut retry_budget,
                &mut progress,
                true,
                &mut deterministic_execution_failure,
            )
            .await?;
        let evaluated = merge_safety_repeats(results, repeats).map_err(model_task_failure)?;
        let merged = match baseline {
            Some(baseline) => merge_tagging_retest(baseline, evaluated)?,
            None => evaluated,
        };
        let mut result =
            TagQualityEvaluationResult::summarize(&suite, merged).map_err(model_task_failure)?;
        if execution.role.execution.adapter_id == crate::assistant::TYPESAFE_ADAPTER {
            result.engine_id = crate::assistant::JEV_TAGGER_CONTRACT;
        }
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
    async fn evaluate_tagging_cases(
        &self,
        context: &JobExecutionContext,
        role: &ResolvedRoleExecution,
        cases: &[TagQualityCase],
        usage: &mut ProviderUsageAccumulator,
        retry_budget: &mut u8,
        progress: &mut TaggingQualityProgress,
        safety_repeat: bool,
        deterministic_execution_failure: &mut Option<ModelTaskError>,
    ) -> Result<Vec<TagQualityCaseResult>, JobHandlerError> {
        let mut results = Vec::with_capacity(cases.len());
        let batches = crate::assistant::plan_tag_quality_batches_for_adapter(
            cases,
            &role.execution.adapter_id,
            |request| self.transport.validate_request(&role.execution, request),
        )
        .map_err(model_task_failure)?;
        for planned in batches {
            let chunk = &cases[planned.case_range];
            let vocabulary = &planned.vocabulary;
            let batch = planned.task;
            let mut diagnostics = planned.native_task.as_ref().map(|task| task.diagnostics());
            let not_run = deterministic_execution_failure.is_some();
            let profiles = if let Some(error) = deterministic_execution_failure.as_ref() {
                Err(not_run_after(error))
            } else if let Some(native) = planned.native_task {
                let transport = self
                    .transport
                    .typed_decisions()
                    .ok_or_else(|| JobHandlerError::new("unsupported_provider_feature"))?;
                native
                    .execute(context, role, transport, usage, diagnostics.as_mut())
                    .await?
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
                && !not_run
                && (role.execution.adapter_id == crate::assistant::TYPESAFE_ADAPTER
                    || deterministic_tagger_execution_failure(error))
            {
                *deterministic_execution_failure = Some(error.clone());
            }
            for case in chunk {
                let mut result = match &profiles {
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
                result.diagnostics = diagnostics.clone();
                results.push(result);
                progress.record(case.gate, safety_repeat, not_run);
                update_progress(
                    context,
                    progress.completed_scenarios,
                    progress.total_scenarios,
                    "Evaluating tagging model",
                    progress.message(),
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
        parameters.limits.validate().map_err(model_task_failure)?;
        validate_feature_header(
            &parameters.role_id,
            "music_tagger",
            &parameters.quality_evaluation_id,
            TAGGING_QUALITY_EVALUATION_ID,
            &parameters.disclosure_version,
            "assistant-model-music-tagging-disclosure/v16",
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
        if role.fingerprint != parameters.role_fingerprint
            || role.inference_fingerprint != parameters.inference_fingerprint
        {
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
                    &parameters.inference_fingerprint,
                    &parameters.vocabulary_fingerprint,
                    contexts.get(&track.track.id),
                    track.catalog_evidence.as_ref(),
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
                        signatures.get(&track.track.id).is_some_and(|signature| {
                            crate::assistant::model_tag_profile_is_current(analysis, signature)
                        })
                    })
            })
            .collect::<Vec<_>>();
        let deferred_tracks = work.len().saturating_sub(parameters.limits.max_tracks);
        let unchanged_profiles = planned.len().saturating_sub(work.len());
        let work = &work[..work.len().min(parameters.limits.max_tracks)];
        let total = work.len();
        let inputs = work
            .iter()
            .map(|track| {
                model_tag_track_input(
                    &track.track,
                    contexts.get(&track.track.id),
                    track.catalog_evidence.as_ref(),
                )
            })
            .collect::<Vec<_>>();
        let batches = crate::assistant::plan_tagger_engine_batches(
            &inputs,
            &vocabulary,
            &role.execution.adapter_id,
            |request| self.transport.validate_request(&role.execution, request),
        )
        .map_err(model_task_failure)?;
        let native = role.execution.adapter_id == crate::assistant::TYPESAFE_ADAPTER;
        let max_attempts = if native {
            batches
                .iter()
                .map(|batch| {
                    batch
                        .native_task
                        .as_ref()
                        .map_or(0, |task| task.max_requests)
                })
                .sum::<usize>()
        } else {
            tagging_attempt_budget(batches.len())
        };
        if native
            && (max_attempts > parameters.limits.max_requests
                || batches
                    .iter()
                    .map(|batch| {
                        batch
                            .native_task
                            .as_ref()
                            .map_or(0, |task| task.token_reservation)
                    })
                    .sum::<u64>()
                    > parameters.limits.max_token_reservation)
        {
            return Err(JobHandlerError::new("tagging_budget_too_small"));
        }
        if native && parameters.execution_mode == crate::assistant::ModelTaggingExecutionMode::Batch
        {
            return Err(JobHandlerError::new("batch_unsupported_adapter"));
        }
        if parameters.execution_mode == crate::assistant::ModelTaggingExecutionMode::Batch
            && !work.is_empty()
        {
            let templates = work.iter().map(|track| {
                AnalysisWrite {
                    track_id: track.track.id, source_signature: signatures[&track.track.id].clone(),
                    moods: Vec::new(), evidence: Vec::new(), decisions: Vec::new(),
                    metrics: json!({"contract":"assistant-music-tagger-output/v5", "input_contract":MODEL_TAGGER_INPUT_CONTRACT,
                        "context_status":contexts.get(&track.track.id).map_or("missing", |context| context.completeness.as_str()),
                        "role_fingerprint":parameters.role_fingerprint,"vocabulary_fingerprint":parameters.vocabulary_fingerprint,
                        "input_snapshot":super::super::model_tagger::model_tag_input_snapshot(&model_tag_track_input(&track.track, contexts.get(&track.track.id), track.catalog_evidence.as_ref()))}).as_object().cloned().unwrap_or_default(),
                }
            }).collect();
            return self
                .submit_tagging_batch(context, &parameters, &role, &inputs, &batches, templates)
                .await;
        }
        if let Some(services) = &self.batch
            && services
                .repository
                .pending_model_batch()
                .await
                .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?
                .is_some()
        {
            return Err(JobHandlerError::new("model_batch_pending"));
        }
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
        let mut processed_tracks = 0_usize;
        let mut tracks_with_suggestions = 0_usize;
        let mut suggested_tags = 0_usize;
        let mut stopped_empty_batch = false;
        let mut track_results = Vec::new();
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
            max_attempts.min(parameters.limits.max_requests),
            ModelReviewDestination::TrackTagReview,
        )
        .await?;
        provider_usage.limit_token_reservation(parameters.limits.max_token_reservation);
        context
            .checkpoint(provider_usage.checkpoint())
            .await
            .map_err(JobHandlerError::from_execution)?;
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
            let profiles = if let Some(native) = planned.native_task {
                let transport = self
                    .transport
                    .typed_decisions()
                    .ok_or_else(|| JobHandlerError::new("unsupported_provider_feature"))?;
                native
                    .execute(context, &role, transport, &mut provider_usage, None)
                    .await?
                    .map_err(model_task_failure)?
            } else {
                loop {
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
                    let context_status = contexts
                        .get(&track.track.id)
                        .map(|context| context.completeness.as_str())
                        .unwrap_or("missing");
                    Ok(ModelAnalysisWrite {
                        profile: AnalysisWrite {
                            track_id: track.track.id,
                            source_signature: signatures
                                .get(&track.track.id)
                                .cloned()
                                .ok_or_else(|| JobHandlerError::new("model_tag_source_invalid"))?,
                            moods: model.tags.clone(),
                            evidence: model.evidence.clone(),
                            metrics: json!({
                                "contract": "assistant-music-tagger-output/v5",
                                "input_contract": MODEL_TAGGER_INPUT_CONTRACT,
                                "context_status": context_status,
                                "role_fingerprint": parameters.role_fingerprint,
                                "vocabulary_fingerprint": parameters.vocabulary_fingerprint,
                                "input_snapshot": super::super::model_tagger::model_tag_input_snapshot(&model_tag_track_input(&track.track, contexts.get(&track.track.id), track.catalog_evidence.as_ref())),
                            })
                            .as_object()
                            .cloned()
                            .ok_or_else(|| JobHandlerError::new("model_tag_profile_invalid"))?,
                            decisions: model.decisions.clone(),
                        },
                    })
                })
                .collect::<Result<Vec<_>, JobHandlerError>>()?;
            let stored = self
                .analysis_repository
                .store_model_analysis(
                    MODEL_TAG_ANALYZER_ID,
                    context.job_id(),
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
            updated = updated.saturating_add(stored);
            skipped_changed = skipped_changed.saturating_add(batch.len().saturating_sub(stored));
            let batch_tags = profiles
                .values()
                .map(|profile| profile.tags.len())
                .sum::<usize>();
            suggested_tags += batch_tags;
            tracks_with_suggestions += profiles
                .values()
                .filter(|profile| !profile.tags.is_empty())
                .count();
            processed_tracks += batch.len();
            track_results.extend(writes.iter().map(|write| {
                json!({
                    "track_id": write.profile.track_id.get(),
                    "source_signature": write.profile.source_signature,
                    "tags": write.profile.moods,
                    "evidence": write.profile.evidence,
                    "decisions": write.profile.decisions,
                })
            }));
            provider_usage.set_feature_progress(json!({
                "processed_tracks": processed_tracks,
                "tracks_with_suggestions": tracks_with_suggestions,
                "tracks_without_suggestions": processed_tracks - tracks_with_suggestions,
                "suggested_tags": suggested_tags,
                "updated_profiles": updated,
                "skipped_changed_tracks": skipped_changed,
                "track_results": track_results,
            }));
            context
                .checkpoint(provider_usage.checkpoint())
                .await
                .map_err(JobHandlerError::from_execution)?;
            update_progress(
                context,
                (start + batch.len()).min(total),
                total,
                "Saving reviewable suggestions",
                format!("Processed {} of {total} tracks", start + batch.len()),
            )
            .await?;
            if parameters.limits.stop_on_empty_batch && batch_tags == 0 && processed_tracks < total
            {
                stopped_empty_batch = true;
                break;
            }
        }
        Ok(json!({
            "schema_version": "assistant-model-music-tagging-job-result/v7",
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
            "deferred_tracks": deferred_tracks,
            "updated_profiles": updated,
            "unchanged_profiles": unchanged_profiles,
            "processed_tracks": processed_tracks,
            "tracks_with_suggestions": tracks_with_suggestions,
            "tracks_without_suggestions": processed_tracks - tracks_with_suggestions,
            "suggested_tags": suggested_tags,
            "stopped_empty_batch": stopped_empty_batch,
            "remaining_tracks": total.saturating_sub(processed_tracks),
            "track_results": track_results,
            "skipped_changed_tracks": skipped_changed,
            "usage": provider_usage.summary(),
        }))
    }
}

#[cfg(test)]
mod quality_progress_tests {
    use super::*;

    #[test]
    fn aborted_quality_cases_remain_failed_and_distinct_from_the_rejected_response()
    -> Result<(), Box<dyn std::error::Error>> {
        let suite = tag_quality_suite()?;
        let error = ModelTaskError::new("model_execution_typed_answer_set_mismatch");
        let skipped = not_run_after(&error);
        let mut results = Vec::new();
        let mut repeats = Vec::new();
        let mut progress = TaggingQualityProgress::new(&suite.cases);
        for (index, case) in suite.cases.iter().enumerate() {
            let vocabulary = case.vocabulary.snapshot()?;
            let result = case.assess(Err(if index == 0 { &error } else { &skipped }), &vocabulary);
            assert!(!result.passed);
            assert_eq!(
                result
                    .failures
                    .iter()
                    .any(|v| v.contains("model_evaluation_not_run")),
                index > 0
            );
            assert!(result.failures.iter().any(|v| v.contains(&error.code)));
            results.push(result);
            progress.record(case.gate, false, index > 0);
            if case.gate == TagQualityGate::Safety {
                repeats.push(case.assess(Err(&skipped), &vocabulary));
                progress.record(case.gate, true, true);
            }
        }
        let report =
            TagQualityEvaluationResult::summarize(&suite, merge_safety_repeats(results, repeats)?)?;
        assert!(!report.passed);
        assert_eq!(report.passed_cases, 0);
        assert_eq!(report.total_cases as usize, suite.cases.len());
        assert_eq!(report.minimum_quality_pass_rate, 0.90);
        assert_eq!(progress.skipped_attempts, progress.total_attempts - 1);
        assert_eq!(progress.completed_scenarios, suite.cases.len());
        assert!(
            progress
                .message()
                .contains("not run after an earlier failure")
        );
        Ok(())
    }

    #[test]
    fn full_suite_and_retest_count_scenarios_once_after_safety_repeats()
    -> Result<(), Box<dyn std::error::Error>> {
        let suite = tag_quality_suite()?;
        let retest = suite
            .cases
            .iter()
            .filter(|case| {
                ["curious-puzzle", "metadata-prompt-injection"].contains(&case.id.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        for cases in [&suite.cases, &retest] {
            let mut progress = TaggingQualityProgress::new(cases);
            let safety = cases
                .iter()
                .filter(|case| case.gate == TagQualityGate::Safety)
                .collect::<Vec<_>>();
            let expected_total = cases.len();
            for case in cases {
                progress.record(case.gate, false, false);
                assert_eq!(progress.total_scenarios, expected_total);
                assert!(progress.completed_scenarios < expected_total);
            }
            assert_eq!(progress.completed_scenarios, expected_total - safety.len());
            assert_eq!(progress.completed_attempts, expected_total);
            for case in safety {
                progress.record(case.gate, true, false);
                assert_eq!(progress.total_scenarios, expected_total);
            }
            assert_eq!(progress.completed_scenarios, expected_total);
            assert_eq!(progress.completed_attempts, progress.total_attempts);
        }
        Ok(())
    }
}
