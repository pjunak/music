//! Durable bounded execution of frozen Score questions, with no adaptive calls or retries.
use super::*;
use music_application::{
    assistant::{
        ModelReviewDestination, ModelRunManifest, ModelTaskError, ModelTransportFuture,
        ProviderAttemptOutcome, ProviderExecutionTarget, ProviderUsageAccumulator,
        ResolvedRoleExecution, StructuredModelResult, TypedDecisionTransport,
        execute_recorded_typed_request,
    },
    jobs::{
        JobCheckpointPolicy, JobDefinition, JobExecutionContext, JobHandler, JobHandlerError,
        JobHandlerFuture, JobLane, JobProgress,
    },
};
use serde_json::Map;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
struct RecordedTransport {
    inner: Arc<dyn TypedDecisionTransport>,
    journal: Arc<Mutex<File>>,
    index: usize,
}
impl RecordedTransport {
    fn record(&self, value: &Value) -> Result<()> {
        checkpoint(
            &mut *self
                .journal
                .lock()
                .map_err(|_| "journal lock unavailable")?,
            value,
        )
    }
}
impl TypedDecisionTransport for RecordedTransport {
    fn validate_typed_request(
        &self,
        target: &ProviderExecutionTarget,
        request: &TypedDecisionRequest,
    ) -> std::result::Result<(), ModelTaskError> {
        self.inner.validate_typed_request(target, request)
    }
    fn execute_typed_request<'a>(
        &'a self,
        target: &'a ProviderExecutionTarget,
        request: &'a TypedDecisionRequest,
    ) -> ModelTransportFuture<'a> {
        Box::pin(async move {
            let failure = || StructuredModelResult {
                succeeded: false,
                error_code: Some("graded_journal_unavailable".into()),
                outcome: ProviderAttemptOutcome::Uncertain,
                payload: None,
                provider_model_id: None,
                finish_reason: None,
                input_tokens: None,
                output_tokens: None,
                token_details: Default::default(),
            };
            if self
                .record(&json!({"event":"attempt_started","index":self.index,"request":request}))
                .is_err()
            {
                return failure();
            }
            let result = self.inner.execute_typed_request(target, request).await;
            if self
                .record(&json!({"event":"response","index":self.index,
                "succeeded":result.succeeded,"model":result.provider_model_id,
                "input_tokens":result.input_tokens,"output_tokens":result.output_tokens,
                "answers":result.payload}))
                .is_err()
            {
                return failure();
            }
            result
        })
    }
}

#[derive(Debug)]
struct Handler {
    plan: Value,
    role: ResolvedRoleExecution,
    transport: Arc<dyn TypedDecisionTransport>,
    journal: Arc<Mutex<File>>,
}
fn error(e: impl std::fmt::Display) -> JobHandlerError {
    JobHandlerError::new(e.to_string())
}
impl JobHandler for Handler {
    fn definition(&self) -> JobDefinition {
        JobDefinition {
            kind: "dev.jev-graded-pilot",
            schema_version: 1,
            lane: JobLane::Provider,
            restartable: false,
            checkpoint_policy: JobCheckpointPolicy::Replace,
        }
    }
    fn execute<'a>(
        &'a self,
        context: &'a JobExecutionContext,
        _: Map<String, Value>,
    ) -> JobHandlerFuture<'a> {
        Box::pin(async move {
            let cases = self.plan["cases"]
                .as_array()
                .ok_or_else(|| error("cases"))?;
            let mut usage = ProviderUsageAccumulator::for_run(ModelRunManifest::new(
                context,
                &self.role,
                "dev.jev-graded-pilot",
                None,
                &json!(fingerprint(&self.plan).map_err(error)?),
                &json!(RUBRIC),
                cases.len(),
                ModelReviewDestination::QualityEvaluation,
            )?);
            usage.limit_token_reservation(
                self.plan["max_input_units"]
                    .as_u64()
                    .ok_or_else(|| error("units"))?,
            );
            let mut rows = Vec::new();
            for (index, case) in cases.iter().enumerate() {
                let transport = RecordedTransport {
                    inner: self.transport.clone(),
                    journal: self.journal.clone(),
                    index,
                };
                let request = request(case).map_err(error)?;
                let result = execute_recorded_typed_request(
                    context, &transport, &self.role, &request, &mut usage,
                )
                .await?;
                if !result.succeeded {
                    return Err(error(
                        "graded experiment stopped after provider failure; no retry",
                    ));
                }
                let values = scores(
                    case,
                    result.payload.ok_or_else(|| error("missing answers"))?,
                )
                .map_err(error)?;
                let probe = case["group"] == "probe";
                let mut probe_passed = true;
                if probe {
                    let fit = values["calm"]["relevance"]
                        .as_f64()
                        .ok_or_else(|| error("probe fit"))?;
                    probe_passed = if case["expected_extreme"] == 4 {
                        fit >= 0.75
                    } else {
                        fit <= 0.25
                    };
                }
                rows.push(
                    json!({"case":index,"track_id":case["track_id"],"arm":case["arm"],
                    "group":case["group"],"repeat_control":case["repeat_control"],"scores":values,
                    "probe_passed":if probe {Some(probe_passed)} else {None}}),
                );
                usage.set_feature_progress(json!({"certifies_model":false,"rows":rows}));
                context
                    .checkpoint(usage.checkpoint())
                    .await
                    .map_err(JobHandlerError::from_execution)?;
                context
                    .update_progress(
                        JobProgress::new(
                            index as u64 + 1,
                            Some(cases.len() as u64),
                            "Graded listening experiment",
                            format!("{} / {} / {}", case["track_id"], case["arm"], case["group"]),
                        )
                        .map_err(error)?,
                    )
                    .await
                    .map_err(JobHandlerError::from_execution)?;
                if !probe_passed {
                    return Err(error(
                        "native Score semantic probe failed; no song requests sent",
                    ));
                }
            }
            let mut result = usage.checkpoint();
            result.remove("feature_progress");
            result.insert("certifies_model".into(), json!(false));
            result.insert("rows".into(), json!(rows));
            Ok(Value::Object(result))
        })
    }
}

pub(crate) async fn run(
    plan_path: &Path,
    key_file: &Path,
    output: &Path,
    expected: &str,
    calls: usize,
    units: u64,
) -> Result<()> {
    let plan = pilot::read_json(plan_path)?;
    authorize(&plan, expected, calls, units)?;
    for recording in plan["recordings"].as_array().ok_or("recordings")? {
        let path = Path::new(recording["source_path"].as_str().ok_or("source path")?);
        if pilot::audio_hash(path)? != recording["file_sha256"] {
            return Err("original changed since analysis".into());
        }
    }
    std::fs::create_dir(output)?;
    serde_json::to_writer_pretty(new_file(&output.join("plan.json"))?, &plan)?;
    let journal = Arc::new(Mutex::new(new_file(&output.join("requests.jsonl"))?));
    let (role, transport) = quality::execution(key_file, expected)?;
    let total = plan["max_requests"].as_u64().ok_or("requests")? as usize;
    quality::run_job(
        Arc::new(Handler {
            plan,
            role,
            transport: Arc::new(transport),
            journal,
        }),
        output,
        "jev-graded-pilot",
        total,
    )
    .await?;
    println!("Graded pilot complete; all scores need listening review. No library tags changed.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn invalid_approval_and_changed_original_fail_before_key_or_output_access() -> Result<()>
    {
        let directory = tempfile::tempdir()?;
        let original = directory.path().join("original.audio");
        std::fs::write(&original, b"original bytes")?;
        let mut recordings = super::super::tests::recordings()?;
        recordings[0]["source_path"] = json!(original);
        let hash = pilot::audio_hash(&original)?;
        recordings[0]["file_sha256"] = json!(hash);
        recordings[0]["learned"]["file_sha256"] = json!(hash);
        let plan = build(&recordings)?;
        let plan_path = directory.path().join("plan.json");
        serde_json::to_writer(new_file(&plan_path)?, &plan)?;
        let output = directory.path().join("must-not-exist");
        let missing_key = directory.path().join("must-not-read");
        let calls = plan["max_requests"].as_u64().ok_or("calls")? as usize;
        let units = plan["max_input_units"].as_u64().ok_or("units")?;
        let denied = run(
            &plan_path,
            &missing_key,
            &output,
            &fingerprint(&plan)?,
            calls + 1,
            units,
        )
        .await
        .err()
        .ok_or("budget should fail")?;
        assert!(denied.to_string().contains("reviewed plan or budget"));
        assert!(!output.exists());
        std::fs::write(&original, b"changed bytes")?;
        let changed = run(
            &plan_path,
            &missing_key,
            &output,
            &fingerprint(&plan)?,
            calls,
            units,
        )
        .await
        .err()
        .ok_or("changed original should fail")?;
        assert!(changed.to_string().contains("original changed"));
        assert!(!output.exists());
        Ok(())
    }

    #[derive(Debug)]
    struct Fake {
        calls: AtomicUsize,
        fail_at: Option<usize>,
    }
    impl TypedDecisionTransport for Fake {
        fn validate_typed_request(
            &self,
            _: &ProviderExecutionTarget,
            r: &TypedDecisionRequest,
        ) -> std::result::Result<(), ModelTaskError> {
            r.validate()
        }
        fn execute_typed_request<'a>(
            &'a self,
            _: &'a ProviderExecutionTarget,
            r: &'a TypedDecisionRequest,
        ) -> ModelTransportFuture<'a> {
            Box::pin(async move {
                let index = self.calls.fetch_add(1, Ordering::SeqCst);
                let failed = self.fail_at == Some(index);
                let level = if index == 0 { 0 } else { 4 };
                let answers = r.questions.keys().map(|id| (id.clone(),json!({"type":"score",
                    "score":level,"confidence":1.0,"probabilities":(0..5).map(|i|
                        (i.to_string(),json!(if i==level {1.0} else {0.0}))).collect::<Map<_,_>>()})))
                    .collect::<Map<_,_>>();
                StructuredModelResult {
                    succeeded: !failed,
                    error_code: failed.then(|| "fixture_failure".into()),
                    outcome: if failed {
                        ProviderAttemptOutcome::Uncertain
                    } else {
                        ProviderAttemptOutcome::ResponseReceived
                    },
                    payload: (!failed).then(|| json!(answers)),
                    provider_model_id: Some(MODEL.into()),
                    finish_reason: None,
                    input_tokens: Some(17),
                    output_tokens: Some(3),
                    token_details: Default::default(),
                }
            })
        }
    }
    #[tokio::test]
    async fn records_complete_dense_scores_and_stops_without_retry_on_uncertain_attempt()
    -> Result<()> {
        for fail_at in [None, Some(2)] {
            let directory = tempfile::tempdir()?;
            let plan = build(&super::super::tests::recordings()?)?;
            let key = directory.path().join("fixture-token");
            std::fs::write(&key, "fixture-not-a-provider-key")?;
            let (role, _) = quality::execution(&key, &fingerprint(&plan)?)?;
            let fake = Arc::new(Fake {
                calls: AtomicUsize::new(0),
                fail_at,
            });
            let journal_path = directory.path().join("requests.jsonl");
            let journal = Arc::new(Mutex::new(new_file(&journal_path)?));
            let total = plan["max_requests"].as_u64().ok_or("total")? as usize;
            let result = quality::run_job(
                Arc::new(Handler {
                    plan,
                    role,
                    transport: fake.clone(),
                    journal,
                }),
                directory.path(),
                "graded-fixture",
                total,
            )
            .await;
            assert_eq!(result.is_ok(), fail_at.is_none());
            let attempted = fail_at.map_or(total, |n| n + 1);
            assert_eq!(fake.calls.load(Ordering::SeqCst), attempted);
            let lines = std::fs::read_to_string(journal_path)?
                .lines()
                .map(serde_json::from_str::<Value>)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            assert_eq!(lines.len(), attempted * 2);
            for pair in lines.chunks(2) {
                assert_eq!(pair[0]["event"], "attempt_started");
                assert_eq!(pair[1]["event"], "response");
                assert_eq!(pair[0]["index"], pair[1]["index"]);
            }
        }
        Ok(())
    }
}
