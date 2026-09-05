use super::*;

impl ModelEvaluationJobHandler {
    pub(super) async fn execute_playlist(
        &self,
        context: &JobExecutionContext,
        parameters: &ModelEvaluationJobParameters,
    ) -> Result<Map<String, Value>, JobHandlerError> {
        let suite = playlist_quality_suite().map_err(model_task_failure)?;
        update_progress(
            context,
            0,
            suite.cases.len(),
            "Preparing evaluation",
            "Loading fixed synthetic playlist scenarios",
        )
        .await?;
        let execution = self.prepare(parameters).await?;
        let max_attempts = suite
            .cases
            .iter()
            .map(|case| {
                case.task().map(|task| {
                    usize::from(task.request().is_some())
                        * (1 + usize::from(case.requires_repeat()))
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(model_task_failure)?
            .into_iter()
            .sum();
        let mut usage =
            start_evaluation_run(context, &execution.role, parameters, max_attempts).await?;
        let mut results = Vec::with_capacity(suite.cases.len());
        for (index, case) in suite.cases.iter().enumerate() {
            let task = case.task().map_err(model_task_failure)?;
            let first = self
                .execute_playlist_task(context, &execution.role, &task, &mut usage)
                .await?;
            let repeated = if case.requires_repeat() {
                Some(
                    self.execute_playlist_task(context, &execution.role, &task, &mut usage)
                        .await?,
                )
            } else {
                None
            };
            results.push(case.assess(first, repeated));
            update_progress(
                context,
                index + 1,
                suite.cases.len(),
                "Evaluating playlist model",
                format!(
                    "Completed {} of {} synthetic scenarios",
                    index + 1,
                    suite.cases.len()
                ),
            )
            .await?;
        }
        let result =
            PlaylistQualityEvaluationResult::from_cases(&suite, MODEL_PLAYLIST_ENGINE_ID, results)
                .map_err(model_task_failure)?;
        context
            .check_cancelled()
            .await
            .map_err(JobHandlerError::from_execution)?;
        self.quality
            .record_evaluation(
                &execution,
                context.job_id(),
                MODEL_PLAYLIST_ENGINE_ID,
                result.passed,
                result.summary.passed_cases,
                result.summary.cases,
            )
            .await
            .map_err(|error| JobHandlerError::new(error.code()))?;
        quality_result(parameters, "full_suite", &result, &usage)
    }

    pub(super) async fn execute_playlist_task(
        &self,
        context: &JobExecutionContext,
        role: &ResolvedRoleExecution,
        task: &ModelPlaylistTask,
        usage: &mut ProviderUsageAccumulator,
    ) -> Result<Result<crate::assistant::PlaylistSuggestion, ModelTaskError>, JobHandlerError> {
        if let Some(result) = task.immediate_result() {
            return Ok(Ok(result));
        }
        let request = task
            .request()
            .ok_or_else(|| JobHandlerError::new("playlist model task is incomplete"))?;
        let result = self.execute_model(context, role, &request, usage).await?;
        Ok(task.finish(result))
    }
}

impl ModelFeatureJobHandler {
    pub(super) async fn execute_playlist(
        &self,
        context: &JobExecutionContext,
        parameters: ModelPlaylistJobParameters,
    ) -> Result<Value, JobHandlerError> {
        validate_feature_header(
            &parameters.role_id,
            "playlist_planner",
            &parameters.quality_evaluation_id,
            PLAYLIST_QUALITY_EVALUATION_ID,
            &parameters.disclosure_version,
            "assistant-playlist-model-disclosure/v2",
            parameters.consent,
            &parameters.role_fingerprint,
        )?;
        update_progress(
            context,
            0,
            3,
            "Loading library evidence",
            "Reading the current local library snapshot",
        )
        .await?;
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
        let tracks = self
            .assistant
            .tracks()
            .await
            .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?;
        let request = parameters.request.application_request()?;
        update_progress(
            context,
            1,
            3,
            "Filtering locally",
            format!(
                "Preparing a bounded candidate pool from {} library tracks",
                tracks.len()
            ),
        )
        .await?;
        let task = ModelPlaylistTask::new(&tracks, &request).map_err(model_task_failure)?;
        let model_request = task.request();
        let mut usage = start_model_run(
            context,
            &role,
            &parameters.quality_evaluation_id,
            Some(&parameters.disclosure_version),
            &parameters,
            &model_request,
            usize::from(model_request.is_some()),
            ModelReviewDestination::PlaylistPreview,
        )
        .await?;
        let suggestion = if let Some(suggestion) = task.immediate_result() {
            suggestion
        } else {
            update_progress(
                context,
                2,
                3,
                "Waiting for playlist model",
                "Sending the disclosed, path-free candidate pool",
            )
            .await?;
            let request = task
                .request()
                .ok_or_else(|| JobHandlerError::new("model_playlist_task_incomplete"))?;
            let result = execute_provider_request(
                context,
                self.transport.as_ref(),
                &role,
                &request,
                &mut usage,
            )
            .await?;
            task.finish(result).map_err(model_task_failure)?
        };
        ensure_feature_role_unchanged(
            &self.quality,
            &parameters.role_id,
            &parameters.quality_evaluation_id,
            &parameters.role_fingerprint,
        )
        .await?;
        update_progress(
            context,
            3,
            3,
            "Draft ready",
            "The model-ranked draft is ready for your review",
        )
        .await?;
        Ok(json!({
            "schema_version": "assistant-playlist-suggestion-job-result/v1",
            "disclosure_version": parameters.disclosure_version,
            "role_id": parameters.role_id,
            "role_fingerprint": parameters.role_fingerprint,
            "suggestion": crate::assistant::playlist_suggestion_payload(&suggestion),
            "usage": usage.summary(),
        }))
    }
}
