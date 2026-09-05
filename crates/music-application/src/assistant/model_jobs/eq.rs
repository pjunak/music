use super::*;

impl ModelEvaluationJobHandler {
    pub(super) async fn execute_eq(
        &self,
        context: &JobExecutionContext,
        parameters: &ModelEvaluationJobParameters,
    ) -> Result<Map<String, Value>, JobHandlerError> {
        let suite = eq_quality_suite().map_err(model_task_failure)?;
        update_progress(
            context,
            0,
            suite.cases.len(),
            "Preparing evaluation",
            "Loading fixed synthetic EQ goals",
        )
        .await?;
        let execution = self.prepare(parameters).await?;
        let mut usage = ProviderUsageAccumulator::default();
        let mut results = Vec::with_capacity(suite.cases.len());
        for (index, case) in suite.cases.iter().enumerate() {
            let task = EqDraftTask::new(&case.id, &case.goal).map_err(model_task_failure)?;
            let model_result = self
                .execute_model(context, &execution.role, &task.request(), &mut usage)
                .await?;
            results.push(case.assess(task.finish(model_result)));
            update_progress(
                context,
                index + 1,
                suite.cases.len(),
                "Evaluating EQ model",
                format!(
                    "Completed {} of {} synthetic goals",
                    index + 1,
                    suite.cases.len()
                ),
            )
            .await?;
        }
        let result = EqQualityEvaluationResult::from_cases(&suite, results);
        context
            .check_cancelled()
            .await
            .map_err(JobHandlerError::from_execution)?;
        self.quality
            .record_evaluation(
                &execution,
                context.job_id(),
                EQ_DRAFT_ENGINE_ID,
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
    pub(super) async fn execute_eq(
        &self,
        context: &JobExecutionContext,
        parameters: ModelEqJobParameters,
    ) -> Result<Value, JobHandlerError> {
        validate_feature_header(
            &parameters.role_id,
            "eq_assistant",
            &parameters.quality_evaluation_id,
            EQ_QUALITY_EVALUATION_ID,
            &parameters.disclosure_version,
            "assistant-eq-draft-disclosure/v2",
            parameters.consent,
            &parameters.role_fingerprint,
        )?;
        update_progress(
            context,
            0,
            2,
            "Preparing EQ request",
            "Validating the fixed graphic-EQ contract",
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
        let task = EqDraftTask::new(&parameters.request.name, &parameters.request.goal)
            .map_err(model_task_failure)?;
        update_progress(
            context,
            1,
            2,
            "Waiting for EQ model",
            "Sending only the disclosed sound goal and fixed EQ limits",
        )
        .await?;
        let mut usage = ProviderUsageAccumulator::default();
        let result = execute_provider_request(
            context,
            self.transport.as_ref(),
            &role,
            &task.request(),
            &mut usage,
        )
        .await?;
        let draft = task.finish(result).map_err(model_task_failure)?;
        ensure_feature_role_unchanged(
            &self.quality,
            &parameters.role_id,
            &parameters.quality_evaluation_id,
            &parameters.role_fingerprint,
        )
        .await?;
        update_progress(
            context,
            2,
            2,
            "Draft ready",
            "The EQ draft is ready for Authoring review",
        )
        .await?;
        Ok(json!({
            "schema_version": "assistant-eq-draft-job-result/v1",
            "disclosure_version": parameters.disclosure_version,
            "role_id": parameters.role_id,
            "role_fingerprint": parameters.role_fingerprint,
            "engine_id": EQ_DRAFT_ENGINE_ID,
            "draft": draft,
            "usage": usage.summary(),
        }))
    }
}
