use super::*;
use crate::assistant::{
    CleanupModelDecision, LIBRARY_CLEANUP_ENGINE_ID, LIBRARY_CLEANUP_SUITE_ID,
    library_cleanup_quality_cases,
};

impl ModelEvaluationJobHandler {
    pub(super) async fn execute_library_cleanup(
        &self,
        context: &JobExecutionContext,
        parameters: &ModelEvaluationJobParameters,
    ) -> Result<Map<String, Value>, JobHandlerError> {
        let cases = library_cleanup_quality_cases().map_err(model_task_failure)?;
        let execution = self.prepare(parameters).await?;
        let mut usage =
            start_evaluation_run(context, &execution.role, parameters, cases.len()).await?;
        let mut results = Vec::new();
        for (name, task, expected) in cases {
            let result = self
                .execute_model(context, &execution.role, &task.request(), &mut usage)
                .await?;
            let decision = task.finish(result);
            let passed = decision.as_ref().is_ok_and(|out| match &expected {
                Some(id) => {
                    out.decision == CleanupModelDecision::Select
                        && out.candidate_id.as_ref() == Some(id)
                }
                None => out.decision != CleanupModelDecision::Select && out.candidate_id.is_none(),
            });
            results.push(json!({"id": name, "passed": passed, "error_code": decision.as_ref().err().map(|e| &e.code)}));
        }
        let passed = results.iter().filter(|r| r["passed"] == true).count() as u32;
        self.quality
            .record_evaluation(
                &execution,
                context.job_id(),
                LIBRARY_CLEANUP_ENGINE_ID,
                passed == results.len() as u32,
                passed,
                results.len() as u32,
            )
            .await
            .map_err(|e| JobHandlerError::new(e.code()))?;
        quality_result(
            parameters,
            "full_suite",
            &json!({"schema_version": "assistant-library-cleanup-quality-result/v1", "suite_id": LIBRARY_CLEANUP_SUITE_ID,
            "passed": passed == results.len() as u32, "passed_cases": passed, "total_cases": results.len(), "cases": results}),
            &usage,
        )
    }
}
