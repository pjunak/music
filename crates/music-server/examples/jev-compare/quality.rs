//! Reuses native execution, write-ahead attempts, fixtures and scoring. No app acceptance is written.
use super::*;
use music_application::{
    assistant::{
        ModelReviewDestination, ModelRunManifest, ModelTaskError, ModelTransportFuture,
        ProviderAttemptOutcome, ProviderExecutionTarget, ProviderSecret, ProviderUsageAccumulator,
        ResolvedRoleExecution, StructuredModelResult, TYPESAFE_ADAPTER, TagQualityEvaluationResult,
        TagQualityGate, ThinkingMode, TypedDecisionTransport, execute_recorded_typed_request,
        merge_safety_repeats, typed_conformance_request, validate_typed_conformance,
    },
    jobs::{
        JobCheckpointPolicy, JobDefinition, JobExecutionContext, JobHandler, JobHandlerError,
        JobHandlerFuture, JobLane, JobProgress, JobRepository, JobStatus, NewJob,
        start_job_coordinator,
    },
};
use music_storage::{SqliteStorage, SqliteStorageOptions};
use serde_json::Map;
use std::sync::Arc;

const MAX_QUALITY_REQUESTS: usize = 818;
const MAX_QUALITY_INPUT_UNITS: u64 = 21_000_000;

#[derive(Debug)]
pub(super) struct Transport(reqwest::Client);

impl TypedDecisionTransport for Transport {
    fn validate_typed_request(
        &self,
        target: &ProviderExecutionTarget,
        request: &TypedDecisionRequest,
    ) -> std::result::Result<(), ModelTaskError> {
        if target.model_id != MODEL || target.adapter_id != TYPESAFE_ADAPTER {
            return Err(ModelTaskError::new("invalid_request"));
        }
        request.validate()
    }

    fn execute_typed_request<'a>(
        &'a self,
        target: &'a ProviderExecutionTarget,
        request: &'a TypedDecisionRequest,
    ) -> ModelTransportFuture<'a> {
        Box::pin(async move {
            match call(&self.0, target.api_key.expose_secret(), request).await {
                Ok(value) => StructuredModelResult {
                    succeeded: true,
                    error_code: None,
                    outcome: ProviderAttemptOutcome::ResponseReceived,
                    payload: Some(value["answers"].clone()),
                    provider_model_id: Some(MODEL.to_owned()),
                    finish_reason: None,
                    input_tokens: value["input_tokens"].as_u64(),
                    output_tokens: value["output_tokens"].as_u64(),
                    token_details: Default::default(),
                },
                Err(_) => StructuredModelResult {
                    succeeded: false,
                    error_code: Some("comparison_request_failed".to_owned()),
                    outcome: ProviderAttemptOutcome::Uncertain,
                    payload: None,
                    provider_model_id: None,
                    finish_reason: None,
                    input_tokens: None,
                    output_tokens: None,
                    token_details: Default::default(),
                },
            }
        })
    }
}

#[derive(Debug)]
struct Handler {
    role: ResolvedRoleExecution,
    transport: Transport,
    max_requests: usize,
    max_units: u64,
}

fn job_error(error: impl std::fmt::Display) -> JobHandlerError {
    JobHandlerError::new(error.to_string())
}

impl JobHandler for Handler {
    fn definition(&self) -> JobDefinition {
        JobDefinition {
            kind: "dev.jev-quality",
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
            let suite = tag_quality_suite().map_err(job_error)?;
            let mut usage = ProviderUsageAccumulator::for_run(ModelRunManifest::new(
                context,
                &self.role,
                "music-tagging-quality-v1",
                None,
                &json!(suite.id),
                &json!(JEV_TAGGER_CONTRACT),
                self.max_requests,
                ModelReviewDestination::QualityEvaluation,
            )?);
            usage.limit_token_reservation(self.max_units);
            let conformance = execute_recorded_typed_request(
                context,
                &self.transport,
                &self.role,
                &typed_conformance_request("comparison"),
                &mut usage,
            )
            .await?;
            validate_typed_conformance("comparison", &conformance).map_err(job_error)?;
            let mut primary = Vec::new();
            let mut repeats = Vec::new();
            let mut failure: Option<ModelTaskError> = None;
            let mut completed = 0;
            let total = suite.cases.len()
                + suite
                    .cases
                    .iter()
                    .filter(|case| case.gate == TagQualityGate::Safety)
                    .count();
            for repeat in [false, true] {
                for case in suite
                    .cases
                    .iter()
                    .filter(|case| !repeat || case.gate == TagQualityGate::Safety)
                {
                    let vocabulary = case.vocabulary.snapshot().map_err(job_error)?;
                    let tasks = plan_jev_tagging(std::slice::from_ref(&case.track), &vocabulary)
                        .map_err(job_error)?;
                    let task = tasks
                        .first()
                        .ok_or_else(|| job_error("missing synthetic task"))?;
                    let mut diagnostics = task.diagnostics();
                    let profiles = if let Some(error) = &failure {
                        Err(ModelTaskError {
                            code: "model_evaluation_not_run".to_owned(),
                            diagnostic: Some(error.code.clone()),
                        })
                    } else {
                        task.execute(
                            context,
                            &self.role,
                            &self.transport,
                            &mut usage,
                            Some(&mut diagnostics),
                        )
                        .await?
                    };
                    let mut result = match profiles {
                        Ok(profiles) => {
                            let id = case.track["track_id"]
                                .as_i64()
                                .ok_or_else(|| job_error("missing track"))?;
                            let profile = profiles
                                .get(&id)
                                .ok_or_else(|| job_error("missing profile"))?;
                            case.assess(Ok(profile), &vocabulary)
                        }
                        Err(error) => {
                            let result = case.assess(Err(&error), &vocabulary);
                            if failure.is_none() {
                                failure = Some(error);
                            }
                            result
                        }
                    };
                    result.diagnostics = Some(diagnostics);
                    if repeat {
                        repeats.push(result);
                    } else {
                        primary.push(result);
                    }
                    completed += 1;
                    usage.set_feature_progress(
                        json!({"certifies_model":false,"conformance_passed":true,
                        "primary":primary,"repeats":repeats}),
                    );
                    context
                        .checkpoint(usage.checkpoint())
                        .await
                        .map_err(JobHandlerError::from_execution)?;
                    context
                        .update_progress(
                            JobProgress::new(
                                completed,
                                Some(total as u64),
                                "Synthetic checks",
                                &case.id,
                            )
                            .map_err(job_error)?,
                        )
                        .await
                        .map_err(JobHandlerError::from_execution)?;
                }
            }
            let merged = merge_safety_repeats(primary, repeats).map_err(job_error)?;
            let mut evaluation =
                TagQualityEvaluationResult::summarize(&suite, merged).map_err(job_error)?;
            evaluation.engine_id = JEV_TAGGER_CONTRACT;
            let mut result = usage.checkpoint();
            result.remove("feature_progress");
            result.insert(
                "evaluation".to_owned(),
                serde_json::to_value(evaluation).map_err(job_error)?,
            );
            result.insert("certifies_model".to_owned(), json!(false));
            result.insert("conformance_passed".to_owned(), json!(true));
            Ok(Value::Object(result))
        })
    }
}

fn authorize(plan: &Value, expected: &str, calls: usize, units: u64) -> Result<()> {
    let quality_calls = plan["max_quality_requests"]
        .as_u64()
        .ok_or("missing request bound")?;
    let quality_units = plan["max_input_units"]
        .as_u64()
        .ok_or("missing input bound")?;
    let conformance_units = model_request_reservation(
        &typed_conformance_request("comparison").accounting_request(),
        0,
    );
    if fingerprint(plan)? != expected
        || calls > MAX_QUALITY_REQUESTS
        || units > MAX_QUALITY_INPUT_UNITS
        || quality_calls + 1 != calls as u64
        || quality_units + conformance_units != units
    {
        return Err("full suite exceeds authorization or its reviewed plan changed".into());
    }
    Ok(())
}

pub(super) async fn run(
    key_file: &Path,
    output: &Path,
    expected: &str,
    calls: usize,
    units: u64,
) -> Result<()> {
    let plan = quality_plan()?;
    authorize(&plan, expected, calls, units)?;
    std::fs::create_dir(output)?;
    serde_json::to_writer_pretty(new_file(&output.join("plan.json"))?, &plan)?;
    let (role, transport) = execution(key_file, expected)?;
    let handler = Arc::new(Handler {
        role,
        transport,
        max_requests: calls,
        max_units: units,
    });
    let job = run_job(handler, output, "jev-quality", 82).await?;
    if let Some(evaluation) = job.as_ref().and_then(|result| result.get("evaluation")) {
        println!(
            "Quality: {}/{} scenarios; gate passed: {}. This run does not update app acceptance.",
            evaluation["passed_cases"], evaluation["total_cases"], evaluation["passed"]
        );
    }
    Ok(())
}

pub(super) fn execution(
    key_file: &Path,
    expected: &str,
) -> Result<(ResolvedRoleExecution, Transport)> {
    let mut bytes = Zeroizing::new(Vec::new());
    File::open(key_file)?.take(4097).read_to_end(&mut bytes)?;
    let secret = std::str::from_utf8(&bytes)?.trim();
    if secret.is_empty() || secret.len() > 4096 || secret.chars().any(char::is_control) {
        return Err("key file must contain one token".into());
    }
    let role = ResolvedRoleExecution {
        connection_id: "isolated-comparison".to_owned(),
        role_id: "music_tagger".to_owned(),
        connection_name: "Temporary Jev comparison".to_owned(),
        fingerprint: expected.to_owned(),
        inference_fingerprint: expected.to_owned(),
        role_configuration_fingerprint: expected.to_owned(),
        connection_fingerprint: expected.to_owned(),
        execution: ProviderExecutionTarget {
            adapter_id: TYPESAFE_ADAPTER.to_owned(),
            base_url: "https://api.typesafe.ai/v1".to_owned(),
            api_key: ProviderSecret::new(secret),
            allow_private_network: false,
            model_id: MODEL.to_owned(),
            thinking_mode: ThinkingMode::ProviderDefault,
            timeout_seconds: 60,
            max_output_tokens: 0,
        },
    };
    let client = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()?;
    Ok((role, Transport(client)))
}

pub(super) async fn run_job(
    handler: Arc<dyn JobHandler>,
    output: &Path,
    id: &str,
    total: usize,
) -> Result<Option<Value>> {
    let storage =
        Arc::new(SqliteStorage::open(SqliteStorageOptions::new(output.join("jobs.sqlite"))).await?);
    storage
        .create(&NewJob {
            id: id.to_owned(),
            definition: handler.definition(),
            parameters: Map::new(),
            retry_of_id: None,
        })
        .await?;
    let coordinator = start_job_coordinator(storage.clone(), vec![handler]).await?;
    let mut previous = 0;
    let job = loop {
        let job = storage.get(id).await?.ok_or("job disappeared")?;
        if job.progress_current != previous {
            println!(
                "{}/{} {}",
                job.progress_current, total, job.progress_message
            );
            previous = job.progress_current;
        }
        if matches!(
            job.status,
            JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled
        ) {
            break job;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    serde_json::to_writer_pretty(
        new_file(&output.join("result.json"))?,
        &json!({"status":job.status,"error":job.error,"result":job.result}),
    )?;
    if job.status != JobStatus::Succeeded {
        return Err("isolated job stopped; inspect the saved result; no automatic retry".into());
    }
    Ok(job.result.map(Value::Object))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_suite_bounds_include_safety_and_conformance() -> Result<()> {
        let plan = quality_plan()?;
        let expected = fingerprint(&plan)?;
        assert_eq!(
            plan["max_total_requests"].as_u64(),
            plan["max_quality_requests"].as_u64().map(|count| count + 1)
        );
        assert!(plan["max_total_input_units"].as_u64() > plan["max_input_units"].as_u64());
        let calls = plan["max_total_requests"].as_u64().ok_or("request bound")? as usize;
        let units = plan["max_total_input_units"]
            .as_u64()
            .ok_or("input bound")?;
        authorize(&plan, &expected, calls, units)?;
        assert!(authorize(&plan, &expected, calls - 1, units).is_err());
        assert!(authorize(&plan, &expected, calls, units - 1).is_err());
        assert!(authorize(&plan, &expected, calls + 1, units).is_err());
        assert!(authorize(&plan, &expected, calls, units + 1).is_err());
        assert!(authorize(&plan, &expected, MAX_QUALITY_REQUESTS + 1, units).is_err());
        assert!(authorize(&plan, &expected, calls, MAX_QUALITY_INPUT_UNITS + 1).is_err());
        assert!(authorize(&plan, "unreviewed", calls, units).is_err());
        let mut changed = plan.clone();
        changed["inference_identity"] = json!("changed grounding contract");
        assert!(authorize(&changed, &expected, calls, units).is_err());
        let cases = plan["cases"].as_array().ok_or("cases")?;
        assert_eq!(cases.len(), 66);
        assert_eq!(
            cases
                .iter()
                .filter_map(|case| case["executions"].as_u64())
                .sum::<u64>(),
            82
        );
        Ok(())
    }
}
