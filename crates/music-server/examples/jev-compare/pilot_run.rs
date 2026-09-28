//! Uses the native task and durable attempt ledger; only the prepared input view changes.
use super::*;
use music_application::{
    assistant::{
        ModelReviewDestination, ModelRunManifest, ModelTaskError, ModelTransportFuture,
        ProviderAttemptOutcome, ProviderExecutionTarget, ProviderUsageAccumulator,
        ResolvedRoleExecution, StructuredModelResult, TypedDecisionTransport,
        execute_recorded_typed_request, validate_typed_conformance,
    },
    jobs::{
        JobCheckpointPolicy, JobDefinition, JobExecutionContext, JobHandler, JobHandlerError,
        JobHandlerFuture, JobLane, JobProgress,
    },
};
use serde_json::Map;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Debug)]
struct PilotTransport {
    inner: Arc<dyn TypedDecisionTransport>,
    journal: Arc<Mutex<File>>,
    case: usize,
    originals: Vec<TypedDecisionRequest>,
    actual: Vec<TypedDecisionRequest>,
    next: AtomicUsize,
}

impl PilotTransport {
    fn request(&self, request: &TypedDecisionRequest) -> Result<TypedDecisionRequest> {
        let index = self.next.load(Ordering::SeqCst);
        if let Some(original) = self.originals.get(index) {
            if serde_json::to_value(original)? != serde_json::to_value(request)? {
                return Err("native assessment no longer matches the reviewed pilot".into());
            }
            return Ok(self.actual[index].clone());
        }
        Ok(request.clone())
    }

    fn write(&self, value: Value) -> Result<()> {
        checkpoint(
            &mut *self
                .journal
                .lock()
                .map_err(|_| "pilot journal unavailable")?,
            &value,
        )
    }
}

impl TypedDecisionTransport for PilotTransport {
    fn validate_typed_request(
        &self,
        target: &ProviderExecutionTarget,
        request: &TypedDecisionRequest,
    ) -> std::result::Result<(), ModelTaskError> {
        let actual = self
            .request(request)
            .map_err(|_| ModelTaskError::new("invalid_request"))?;
        self.inner.validate_typed_request(target, &actual)
    }

    fn execute_typed_request<'a>(
        &'a self,
        target: &'a ProviderExecutionTarget,
        request: &'a TypedDecisionRequest,
    ) -> ModelTransportFuture<'a> {
        Box::pin(async move {
            let failure = || StructuredModelResult {
                succeeded: false,
                error_code: Some("pilot_journal_or_plan_invalid".into()),
                outcome: ProviderAttemptOutcome::Uncertain,
                payload: None,
                provider_model_id: None,
                finish_reason: None,
                input_tokens: None,
                output_tokens: None,
                token_details: Default::default(),
            };
            let Ok(actual) = self.request(request) else {
                return failure();
            };
            let index = self.next.fetch_add(1, Ordering::SeqCst);
            let phase = if self.case == usize::MAX {
                "conformance"
            } else if index < self.originals.len() {
                "assessment"
            } else {
                "grounding"
            };
            if self
                .write(
                    json!({"event":"attempt_started","case":self.case,"index":index,
                "phase":phase,"request":actual}),
                )
                .is_err()
            {
                return failure();
            }
            let result = self.inner.execute_typed_request(target, &actual).await;
            if self
                .write(json!({"event":"response","case":self.case,"index":index,
                "phase":phase,"succeeded":result.succeeded,"model":result.provider_model_id,
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

fn error(error: impl std::fmt::Display) -> JobHandlerError {
    JobHandlerError::new(error.to_string())
}

impl Handler {
    fn transport(
        &self,
        case: usize,
        task: Option<&JevTaggerTask>,
        arm: &str,
    ) -> Result<PilotTransport> {
        let originals = task
            .map(|task| task.assessment_requests().to_vec())
            .unwrap_or_default();
        let actual = originals
            .iter()
            .map(|r| assessment_request(r, arm))
            .collect::<Result<Vec<_>>>()?;
        Ok(PilotTransport {
            inner: self.transport.clone(),
            journal: self.journal.clone(),
            case,
            originals,
            actual,
            next: AtomicUsize::new(0),
        })
    }
}

impl JobHandler for Handler {
    fn definition(&self) -> JobDefinition {
        JobDefinition {
            kind: "dev.jev-private-pilot",
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
            let mut usage = ProviderUsageAccumulator::for_run(ModelRunManifest::new(
                context,
                &self.role,
                "dev.jev-private-pilot",
                None,
                &json!(fingerprint(&self.plan).map_err(error)?),
                &json!(JEV_TAGGER_CONTRACT),
                self.plan["max_requests"]
                    .as_u64()
                    .ok_or_else(|| error("requests"))? as usize,
                ModelReviewDestination::QualityEvaluation,
            )?);
            usage.limit_token_reservation(
                self.plan["max_input_units"]
                    .as_u64()
                    .ok_or_else(|| error("units"))?,
            );
            let conformance = execute_recorded_typed_request(
                context,
                &self.transport(usize::MAX, None, "current").map_err(error)?,
                &self.role,
                &typed_conformance_request("private-pilot"),
                &mut usage,
            )
            .await?;
            validate_typed_conformance("private-pilot", &conformance).map_err(error)?;
            let cases = self.plan["cases"]
                .as_array()
                .ok_or_else(|| error("cases"))?;
            let recordings = self.plan["corpus"]["recordings"]
                .as_array()
                .ok_or_else(|| error("recordings"))?;
            let mut rows = Vec::new();
            for (index, case) in cases.iter().enumerate() {
                let recording = recordings
                    .iter()
                    .find(|v| v["input"]["track_id"] == case["track_id"])
                    .ok_or_else(|| error("recording"))?;
                let arm = case["arm"].as_str().ok_or_else(|| error("arm"))?;
                let task = task(recording, arm).map_err(error)?;
                let mut trace = task.diagnostics();
                let result = task
                    .execute(
                        context,
                        &self.role,
                        &self.transport(index, Some(&task), arm).map_err(error)?,
                        &mut usage,
                        Some(&mut trace),
                    )
                    .await?;
                let (profiles, failure) = match result {
                    Ok(profiles) => (Some(profiles), None),
                    Err(problem) => (None, Some(problem.code)),
                };
                rows.push(json!({"case":index,"track_id":case["track_id"],"arm":arm,
                    "repeat_control":case["repeat_control"],"profiles":profiles,"error":failure,
                    "diagnostics":trace}));
                usage.set_feature_progress(json!({"certifies_model":false,
                    "assessment_mode":"assisted_development_diagnostic","rows":rows}));
                context
                    .checkpoint(usage.checkpoint())
                    .await
                    .map_err(JobHandlerError::from_execution)?;
                context
                    .update_progress(
                        JobProgress::new(
                            index as u64 + 1,
                            Some(cases.len() as u64),
                            "Private pilot comparisons",
                            format!("track {} / {arm}", case["track_id"]),
                        )
                        .map_err(error)?,
                    )
                    .await
                    .map_err(JobHandlerError::from_execution)?;
                if failure.is_some() {
                    return Err(error(
                        "pilot stopped after a failed request; no automatic retry",
                    ));
                }
            }
            let mut result = usage.checkpoint();
            result.remove("feature_progress");
            result.insert("certifies_model".into(), json!(false));
            result.insert(
                "assessment_mode".into(),
                json!("assisted_development_diagnostic"),
            );
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
    let plan = read_json(plan_path)?;
    authorize(&plan, expected, calls, units)?;
    for recording in plan["corpus"]["recordings"]
        .as_array()
        .ok_or("recordings")?
    {
        let path = recording["source_path"].as_str().ok_or("source path")?;
        if audio_hash(Path::new(path))? != recording["file_sha256"] {
            return Err("selected audio changed since the reviewed analysis".into());
        }
    }
    // A new isolated directory is required before reading the key. No production DB is opened.
    std::fs::create_dir(output)?;
    serde_json::to_writer_pretty(new_file(&output.join("plan.json"))?, &plan)?;
    let journal = Arc::new(Mutex::new(new_file(&output.join("requests.jsonl"))?));
    let (role, transport) = quality::execution(key_file, expected)?;
    let total = plan["cases"].as_array().ok_or("cases")?.len();
    let handler = Arc::new(Handler {
        plan,
        role,
        transport: Arc::new(transport),
        journal,
    });
    quality::run_job(handler, output, "jev-private-pilot", total).await?;
    println!(
        "Private pilot complete. Suggestions require listening review; app gates and tags are unchanged."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use music_application::assistant::TypedQuestion;

    #[derive(Debug)]
    struct FakeTransport {
        fail_at: Option<usize>,
        calls: AtomicUsize,
    }

    impl TypedDecisionTransport for FakeTransport {
        fn validate_typed_request(
            &self,
            _: &ProviderExecutionTarget,
            request: &TypedDecisionRequest,
        ) -> std::result::Result<(), ModelTaskError> {
            request.validate()
        }

        fn execute_typed_request<'a>(
            &'a self,
            _: &'a ProviderExecutionTarget,
            request: &'a TypedDecisionRequest,
        ) -> ModelTransportFuture<'a> {
            Box::pin(async move {
                let failed = self.fail_at == Some(self.calls.fetch_add(1, Ordering::SeqCst));
                let answers = request.questions.iter().map(|(id, question)| {
                    let answer = match question {
                        TypedQuestion::Noul { .. } => json!({"type":"noul","noul":
                            if id.starts_with("yes_") || id == "fit_0" || id.starts_with("support_") { 0.99 } else { 0.01 }}),
                        TypedQuestion::Choice { criteria, .. } => {
                            let choice = if id.starts_with("choice_") { "solo_singing" } else { "no_supported_period" };
                            let probabilities = criteria.keys().map(|key| (key.clone(),
                                json!(if key == choice { 1.0 } else { 0.0 }))).collect::<Map<_, _>>();
                            json!({"type":"choice","choice":choice,"probabilities":probabilities,"confidence":1.0})
                        }
                    };
                    (id.clone(), answer)
                }).collect::<Map<_, _>>();
                StructuredModelResult {
                    succeeded: !failed,
                    error_code: failed.then(|| "test_failure".into()),
                    outcome: if failed {
                        ProviderAttemptOutcome::Uncertain
                    } else {
                        ProviderAttemptOutcome::ResponseReceived
                    },
                    payload: (!failed).then(|| Value::Object(answers)),
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
    async fn isolated_pilot_records_actual_wire_states_final_profiles_and_uncertain_stop()
    -> Result<()> {
        for fail_at in [None, Some(1)] {
            let directory = tempfile::tempdir()?;
            let key = directory.path().join("fake-test-token");
            std::fs::write(&key, "not-a-real-key")?;
            let plan = build_plan(&super::super::tests::corpus()?, &[1, 2])?;
            let (role, _) = quality::execution(&key, &fingerprint(&plan)?)?;
            let fake = Arc::new(FakeTransport {
                fail_at,
                calls: AtomicUsize::new(0),
            });
            let journal = directory.path().join("requests.jsonl");
            let handler = Arc::new(Handler {
                plan: plan.clone(),
                role,
                transport: fake.clone(),
                journal: Arc::new(Mutex::new(new_file(&journal)?)),
            });
            let result =
                quality::run_job(handler, directory.path(), "test-private-pilot", 18).await;
            let records = std::fs::read_to_string(&journal)?
                .lines()
                .map(serde_json::from_str::<Value>)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            assert_eq!(records.len(), fake.calls.load(Ordering::SeqCst) * 2);
            for pair in records.chunks_exact(2) {
                assert_eq!(pair[0]["event"], "attempt_started");
                assert_eq!(pair[1]["event"], "response");
                assert_eq!(pair[0]["case"], pair[1]["case"]);
                assert!(!pair[0]["request"].to_string().contains("PRIVATE-"));
            }
            if fail_at.is_some() {
                assert!(result.is_err());
                assert_eq!(fake.calls.load(Ordering::SeqCst), 2);
                let saved = read_json(&directory.path().join("result.json"))?;
                assert_eq!(saved["status"], "failed");
            } else {
                let result = result?.ok_or("result")?;
                let rows = result["rows"].as_array().ok_or("rows")?;
                assert_eq!(rows.len(), 18);
                for row in rows {
                    assert!(row["profiles"].get(row["track_id"].to_string()).is_some());
                }
                assert_eq!(result["certifies_model"], false);
                let cases = plan["cases"].as_array().ok_or("cases")?;
                for record in records
                    .iter()
                    .filter(|v| v["event"] == "attempt_started" && v["phase"] == "assessment")
                {
                    let case = record["case"].as_u64().ok_or("case")? as usize;
                    let index = record["index"].as_u64().ok_or("index")? as usize;
                    assert_eq!(record["request"], cases[case]["assessment"][index]);
                }
                assert!(records.iter().any(|v| v["phase"] == "grounding"));
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn rejected_pilot_never_reads_key_or_creates_run_directory() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let plan = build_plan(&super::super::tests::corpus()?, &[1])?;
        let path = directory.path().join("plan.json");
        serde_json::to_writer(new_file(&path)?, &plan)?;
        let output = directory.path().join("should-not-exist");
        let failure = run(
            &path,
            &directory.path().join("missing-key"),
            &output,
            "unapproved",
            1,
            1,
        )
        .await
        .err()
        .ok_or("must reject")?;
        assert!(failure.to_string().contains("reviewed plan"));
        assert!(!output.exists());
        Ok(())
    }
}
