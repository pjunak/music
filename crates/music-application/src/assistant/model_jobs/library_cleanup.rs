use super::*;
use crate::assistant::{
    CleanupModelDecision, CleanupModelOutput, LIBRARY_CLEANUP_ENGINE_ID, LIBRARY_CLEANUP_SUITE_ID,
    LibraryCleanupTask, ModelTaskError, library_cleanup_quality_cases,
    library_edition_quality_cases,
};

impl ModelEvaluationJobHandler {
    pub(super) async fn execute_library_cleanup(
        &self,
        context: &JobExecutionContext,
        parameters: &ModelEvaluationJobParameters,
    ) -> Result<Map<String, Value>, JobHandlerError> {
        let mut cases = library_cleanup_quality_cases()
            .map_err(model_task_failure)?
            .into_iter()
            .map(|(name, task, expected)| (name, LibraryCleanupTask::Recording(task), expected))
            .collect::<Vec<_>>();
        cases.extend(library_edition_quality_cases().map_err(model_task_failure)?);
        let execution = self.prepare(parameters).await?;
        let mut usage =
            start_evaluation_run(context, &execution.role, parameters, cases.len()).await?;
        let mut results = Vec::new();
        for (name, task, expected) in cases {
            let result = self
                .execute_model(context, &execution.role, &task.request(), &mut usage)
                .await?;
            results.push(quality_case_result(
                &name,
                expected.as_deref(),
                task.finish(result),
            ));
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

fn quality_case_result(
    name: &str,
    expected: Option<&str>,
    decision: Result<CleanupModelOutput, ModelTaskError>,
) -> Value {
    let passed = decision.as_ref().is_ok_and(|out| match expected {
        Some(id) => {
            out.decision == CleanupModelDecision::Select && out.candidate_id.as_deref() == Some(id)
        }
        None => out.decision != CleanupModelDecision::Select && out.candidate_id.is_none(),
    });
    let error_code = match (&decision, passed) {
        (Err(error), _) => Some(error.code.as_str()),
        (Ok(_), false) => Some("cleanup_quality_decision_mismatch"),
        (Ok(_), true) => None,
    };
    json!({"id": name, "passed": passed, "error_code": error_code,
        "expected_candidate_id": expected, "output": decision.as_ref().ok()})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_quality_distinguishes_abstention_from_provider_failure() {
        let abstention = CleanupModelOutput {
            schema_version: "assistant-library-cleanup-output/v1".into(),
            decision: CleanupModelDecision::Ambiguous,
            candidate_id: None,
            evidence_ids: vec![],
            reason: "The supplied evidence remains ambiguous.".into(),
        };
        let miss = quality_case_result("version", Some("candidate-0"), Ok(abstention));
        assert_eq!(miss["passed"], false);
        assert_eq!(miss["error_code"], "cleanup_quality_decision_mismatch");
        assert_eq!(miss["expected_candidate_id"], "candidate-0");
        assert_eq!(miss["output"]["decision"], "ambiguous");
        assert_eq!(
            miss["output"]["reason"],
            "The supplied evidence remains ambiguous."
        );
        let failure = quality_case_result(
            "version",
            Some("candidate-0"),
            Err(ModelTaskError::new("model_execution_invalid_request")),
        );
        assert_eq!(failure["passed"], false);
        assert_eq!(failure["error_code"], "model_execution_invalid_request");
        assert!(failure["output"].is_null());
    }

    #[test]
    fn cleanup_quality_requires_the_expected_candidate_and_preserves_all_cases()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::assistant::structured_harness::tests::model_result;
        let cases = library_cleanup_quality_cases()?;
        assert_eq!(cases.len(), 8);
        for (name, task, expected) in cases {
            let evidence = expected
                .as_ref()
                .map(|id| {
                    vec![
                        format!("{id}-title"),
                        format!("{id}-artist"),
                        format!("{id}-duration"),
                    ]
                })
                .unwrap_or_default();
            let payload = json!({"schema_version": "assistant-library-cleanup-output/v1",
                "decision": if expected.is_some() { "select" } else { "ambiguous" },
                "candidate_id": expected, "evidence_ids": evidence, "reason": "Review the supplied catalog evidence."});
            let schema = task.request().output_schema.ok_or("missing schema")?;
            assert!(jsonschema::validator_for(&schema)?.is_valid(&payload));
            let output = task.finish(model_result(payload))?;
            if let Some(id) = output.candidate_id.as_deref() {
                assert_eq!(
                    task.candidate(id).ok_or("missing candidate")?.id,
                    "recording-one"
                );
            }
            let scored = quality_case_result(&name, expected.as_deref(), Ok(output));
            assert_eq!(scored["passed"], true, "{name}");
            assert!(scored["error_code"].is_null());
        }
        Ok(())
    }
}
