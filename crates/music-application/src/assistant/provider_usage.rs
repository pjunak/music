use std::time::Instant;

use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::{
    ProviderAttemptOutcome, ResolvedRoleExecution, StructuredModelRequest, StructuredModelResult,
    StructuredModelTransport, ThinkingMode,
};
use crate::jobs::{JobExecutionContext, JobHandlerError};

const MAX_PROVIDER_MODEL_IDS: usize = 8;
const MAX_ATTEMPT_RECORDS: usize = 128;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelReviewDestination {
    EqAuthoring,
    PlaylistPreview,
    TagCleanupReview,
    TrackTagReview,
    QualityEvaluation,
}

/// Frozen before execution; only fingerprints of scope/evidence are retained.
/// This is provenance, not permission to apply a proposal or replay a paid run.
#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct ModelRunManifest {
    schema_version: &'static str,
    job_id: String,
    role_id: String,
    role_fingerprint: String,
    role_configuration_fingerprint: String,
    connection_fingerprint: String,
    adapter_id: String,
    model_id: String,
    thinking_mode: ThinkingMode,
    timeout_seconds: u16,
    max_output_tokens_per_request: u32,
    max_attempts: u64,
    output_token_ceiling: u64,
    evaluation_id: String,
    disclosure_version: Option<String>,
    scope_fingerprint: String,
    evidence_fingerprint: String,
    review_destination: ModelReviewDestination,
    queue_wait_seconds: Option<u64>,
}

impl ModelRunManifest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: &JobExecutionContext,
        role: &ResolvedRoleExecution,
        evaluation_id: &str,
        disclosure_version: Option<&str>,
        scope: &impl Serialize,
        evidence: &impl Serialize,
        max_attempts: usize,
        review_destination: ModelReviewDestination,
    ) -> Result<Self, JobHandlerError> {
        let max_attempts = u64::try_from(max_attempts)
            .map_err(|_| JobHandlerError::new("model_run_budget_overflow"))?;
        let output_token_ceiling = max_attempts
            .checked_mul(u64::from(role.execution.max_output_tokens))
            .ok_or_else(|| JobHandlerError::new("model_run_budget_overflow"))?;
        Ok(Self {
            schema_version: "assistant-model-run/v1",
            job_id: context.job_id().to_owned(),
            role_id: role.role_id.clone(),
            role_fingerprint: role.fingerprint.clone(),
            role_configuration_fingerprint: role.role_configuration_fingerprint.clone(),
            connection_fingerprint: role.connection_fingerprint.clone(),
            adapter_id: role.execution.adapter_id.clone(),
            model_id: role.execution.model_id.clone(),
            thinking_mode: role.execution.thinking_mode,
            timeout_seconds: role.execution.timeout_seconds,
            max_output_tokens_per_request: role.execution.max_output_tokens,
            max_attempts,
            output_token_ceiling,
            evaluation_id: evaluation_id.to_owned(),
            disclosure_version: disclosure_version.map(str::to_owned),
            scope_fingerprint: model_input_fingerprint(scope)?,
            evidence_fingerprint: model_input_fingerprint(evidence)?,
            review_destination,
            queue_wait_seconds: context.queue_wait_seconds(),
        })
    }
}

pub fn model_input_fingerprint(value: &impl Serialize) -> Result<String, JobHandlerError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| JobHandlerError::new("model_run_manifest_encoding_failed"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct ProviderAttemptRecord {
    sequence: u64,
    request_fingerprint: String,
    max_output_tokens: u32,
    outcome: ProviderAttemptOutcome,
    elapsed_ms: Option<u64>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct ProviderUsageSummary {
    pub schema_version: &'static str,
    pub attempted_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub input_tokens_reported_requests: u64,
    pub output_tokens_reported_requests: u64,
    pub provider_model_ids: Vec<String>,
    pub provider_model_ids_truncated: bool,
    pub preflight_rejected_requests: u64,
    pub not_sent_requests: u64,
    pub response_received_requests: u64,
    pub uncertain_requests: u64,
    pub responses_missing_usage: u64,
    pub completed_attempt_elapsed_ms: u64,
    pub attempts: Vec<ProviderAttemptRecord>,
    pub attempts_truncated: bool,
    pub run_manifest: ModelRunManifest,
}

#[derive(Debug)]
pub struct ProviderUsageAccumulator {
    summary: ProviderUsageSummary,
    pending: bool,
}

impl ProviderUsageAccumulator {
    #[must_use]
    pub fn for_run(run_manifest: ModelRunManifest) -> Self {
        Self {
            summary: ProviderUsageSummary {
                schema_version: "assistant-provider-usage/v2",
                attempted_requests: 0,
                input_tokens: 0,
                output_tokens: 0,
                input_tokens_reported_requests: 0,
                output_tokens_reported_requests: 0,
                provider_model_ids: Vec::new(),
                provider_model_ids_truncated: false,
                preflight_rejected_requests: 0,
                not_sent_requests: 0,
                response_received_requests: 0,
                uncertain_requests: 0,
                responses_missing_usage: 0,
                completed_attempt_elapsed_ms: 0,
                attempts: Vec::new(),
                attempts_truncated: false,
                run_manifest,
            },
            pending: false,
        }
    }

    fn begin(
        &mut self,
        role: &ResolvedRoleExecution,
        request: &StructuredModelRequest,
    ) -> Result<(), JobHandlerError> {
        let manifest = &self.summary.run_manifest;
        if self.pending || self.summary.attempted_requests >= manifest.max_attempts {
            return Err(JobHandlerError::new("model_run_request_budget_exhausted"));
        }
        if role.role_id != manifest.role_id
            || role.fingerprint != manifest.role_fingerprint
            || role.role_configuration_fingerprint != manifest.role_configuration_fingerprint
            || role.connection_fingerprint != manifest.connection_fingerprint
            || role.execution.adapter_id != manifest.adapter_id
            || role.execution.model_id != manifest.model_id
            || role.execution.thinking_mode != manifest.thinking_mode
            || role.execution.timeout_seconds != manifest.timeout_seconds
            || role.execution.max_output_tokens != manifest.max_output_tokens_per_request
        {
            return Err(JobHandlerError::new("role_changed"));
        }
        let request_fingerprint = model_input_fingerprint(request)?;
        let max_output_tokens = request
            .max_output_tokens
            .min(manifest.max_output_tokens_per_request);
        let summary = &mut self.summary;
        summary.attempted_requests += 1;
        summary.uncertain_requests += 1;
        if summary.attempts.len() == MAX_ATTEMPT_RECORDS {
            summary.attempts.remove(0);
            summary.attempts_truncated = true;
        }
        summary.attempts.push(ProviderAttemptRecord {
            sequence: summary.attempted_requests,
            request_fingerprint,
            max_output_tokens,
            outcome: ProviderAttemptOutcome::Uncertain,
            elapsed_ms: None,
        });
        self.pending = true;
        Ok(())
    }

    fn finish(&mut self, result: &StructuredModelResult, elapsed_ms: u64) {
        let summary = &mut self.summary;
        self.pending = false;
        summary.uncertain_requests -= 1;
        match result.outcome {
            ProviderAttemptOutcome::PreflightRejected => summary.preflight_rejected_requests += 1,
            ProviderAttemptOutcome::NotSent => summary.not_sent_requests += 1,
            ProviderAttemptOutcome::ResponseReceived => summary.response_received_requests += 1,
            ProviderAttemptOutcome::Uncertain => summary.uncertain_requests += 1,
        }
        if let Some(attempt) = summary.attempts.last_mut() {
            attempt.outcome = result.outcome;
            attempt.elapsed_ms = Some(elapsed_ms);
        }
        summary.completed_attempt_elapsed_ms = summary
            .completed_attempt_elapsed_ms
            .saturating_add(elapsed_ms);
        if result.outcome == ProviderAttemptOutcome::ResponseReceived
            && (result.input_tokens.is_none() || result.output_tokens.is_none())
        {
            summary.responses_missing_usage += 1;
        }
        if let Some(input_tokens) = result.input_tokens {
            summary.input_tokens = summary.input_tokens.saturating_add(input_tokens);
            summary.input_tokens_reported_requests += 1;
        }
        if let Some(output_tokens) = result.output_tokens {
            summary.output_tokens = summary.output_tokens.saturating_add(output_tokens);
            summary.output_tokens_reported_requests += 1;
        }
        let Some(model_id) = result
            .provider_model_id
            .as_ref()
            .filter(|model_id| !model_id.is_empty() && model_id.chars().count() <= 256)
        else {
            return;
        };
        if summary.provider_model_ids.contains(model_id) {
            return;
        }
        if summary.provider_model_ids.len() < MAX_PROVIDER_MODEL_IDS {
            summary.provider_model_ids.push(model_id.clone());
        } else {
            summary.provider_model_ids_truncated = true;
        }
    }

    #[must_use]
    pub fn summary(&self) -> &ProviderUsageSummary {
        &self.summary
    }

    #[must_use]
    pub fn checkpoint(&self) -> Map<String, Value> {
        Map::from_iter([
            (
                "schema_version".to_owned(),
                json!("assistant-provider-usage-checkpoint/v2"),
            ),
            ("usage".to_owned(), json!(self.summary)),
        ])
    }
}

/// The write-ahead checkpoint must succeed before any provider I/O. A dropped
/// future or failed completion checkpoint deliberately leaves an uncertain attempt.
pub async fn execute_recorded_provider_request(
    context: &JobExecutionContext,
    transport: &dyn StructuredModelTransport,
    role: &ResolvedRoleExecution,
    request: &StructuredModelRequest,
    usage: &mut ProviderUsageAccumulator,
) -> Result<StructuredModelResult, JobHandlerError> {
    context
        .check_cancelled()
        .await
        .map_err(JobHandlerError::from_execution)?;
    usage.begin(role, request)?;
    if let Err(error) = transport.validate_request(&role.execution, request) {
        let result = StructuredModelResult {
            outcome: ProviderAttemptOutcome::PreflightRejected,
            succeeded: false,
            error_code: Some(error.code),
            payload: None,
            provider_model_id: None,
            finish_reason: None,
            input_tokens: None,
            output_tokens: None,
        };
        usage.finish(&result, 0);
        context
            .checkpoint(usage.checkpoint())
            .await
            .map_err(JobHandlerError::from_execution)?;
        return Ok(result);
    }
    context
        .checkpoint(usage.checkpoint())
        .await
        .map_err(JobHandlerError::from_execution)?;
    context
        .check_cancelled()
        .await
        .map_err(JobHandlerError::from_execution)?;
    let started = Instant::now();
    let result = transport
        .execute_structured_model_request(&role.execution, request)
        .await;
    usage.finish(
        &result,
        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    );
    context
        .checkpoint(usage.checkpoint())
        .await
        .map_err(JobHandlerError::from_execution)?;
    Ok(result)
}
