mod eq;
mod playlist;
mod tag_cleanup;
mod tagging;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::assistant::{
    AnalysisWrite, AssistantService, Confidence, ContextScope, EQ_DRAFT_ENGINE_ID,
    EQ_QUALITY_EVALUATION_ID, EnergyCurve, EqDraftTask, EqQualityEvaluationResult,
    LocalAnalysisRepository, LocalAnalysisService, MAX_MODEL_CLEANUP_TAGS,
    MODEL_PLAYLIST_ENGINE_ID, MODEL_TAG_ANALYZER_ID, MODEL_TAG_CLEANUP_ENGINE_ID,
    MODEL_TAGGER_INPUT_CONTRACT, MODEL_TAGGER_INVALID_RESPONSE_RETRY_LIMIT, ModelAnalysisWrite,
    ModelEvaluationExecution, ModelPlaylistTask, ModelQualityService, ModelTagCleanupTask,
    ModelTaskError, PLAYLIST_QUALITY_EVALUATION_ID, PlaylistQualityEvaluationResult,
    PlaylistSuggestionRequest, ProviderUsageAccumulator, ResolvedRoleExecution,
    StructuredModelRequest, StructuredModelResult, TAG_CLEANUP_QUALITY_EVALUATION_ID,
    TAGGING_QUALITY_EVALUATION_ID, TagCleanupQualityEvaluationResult, TagConfidence,
    TagQualityCase, TagQualityCaseResult, TagQualityEvaluationResult, TagQualityGate,
    TagQualitySuite, build_cleanup_preview, catalog_signature, default_vocabulary_snapshot,
    eq_quality_suite, local_context_axes, merge_safety_repeats, model_tag_cleanup_suggestion_id,
    model_tag_source_signature, model_tag_track_input, playlist_quality_suite,
    retryable_tagger_error, tag_cleanup_quality_suite, tag_quality_suite,
};
use crate::jobs::{
    JobCheckpointPolicy, JobDefinition, JobExecutionContext, JobHandler, JobHandlerError,
    JobHandlerFuture, JobLane, JobProgress, JobStatus,
};
use music_domain::LibraryPath;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{
    ModelReviewDestination, ModelRunManifest, StructuredModelTransport,
    execute_recorded_provider_request as execute_provider_request,
};

#[allow(clippy::too_many_arguments)]
async fn start_model_run(
    context: &JobExecutionContext,
    role: &ResolvedRoleExecution,
    evaluation_id: &str,
    disclosure_version: Option<&str>,
    scope: &impl Serialize,
    evidence: &impl Serialize,
    max_attempts: usize,
    destination: ModelReviewDestination,
) -> Result<ProviderUsageAccumulator, JobHandlerError> {
    let usage = ProviderUsageAccumulator::for_run(ModelRunManifest::new(
        context,
        role,
        evaluation_id,
        disclosure_version,
        scope,
        evidence,
        max_attempts,
        destination,
    )?);
    context
        .checkpoint(usage.checkpoint())
        .await
        .map_err(JobHandlerError::from_execution)?;
    Ok(usage)
}

async fn start_evaluation_run(
    context: &JobExecutionContext,
    role: &ResolvedRoleExecution,
    parameters: &ModelEvaluationJobParameters,
    max_attempts: usize,
) -> Result<ProviderUsageAccumulator, JobHandlerError> {
    // The role fingerprint covers the exact synthetic suite and task sources.
    start_model_run(
        context,
        role,
        &parameters.evaluation_id,
        None,
        parameters,
        &(&parameters.evaluation_id, &role.fingerprint),
        max_attempts,
        ModelReviewDestination::QualityEvaluation,
    )
    .await
}

fn tagging_attempt_budget(planned_requests: usize) -> usize {
    if planned_requests == 0 {
        0
    } else {
        planned_requests + usize::from(MODEL_TAGGER_INVALID_RESPONSE_RETRY_LIMIT)
    }
}

pub const MODEL_PLAYLIST_SUGGESTION_JOB_KIND: &str = "assistant.model-playlist-suggestion";
pub const MODEL_EQ_DRAFT_JOB_KIND: &str = "assistant.model-eq-draft";
pub const MODEL_TAG_CLEANUP_JOB_KIND: &str = "assistant.model-tag-cleanup";
pub const MODEL_TAGGING_JOB_KIND: &str = "assistant.model-music-tagging";

#[derive(Debug, Clone, Copy)]
enum EvaluationKind {
    Playlist,
    Tagging,
    TagCleanup,
    Eq,
}

impl EvaluationKind {
    const fn role_id(self) -> &'static str {
        match self {
            Self::Playlist => "playlist_planner",
            Self::Tagging => "music_tagger",
            Self::TagCleanup => "tag_cleanup",
            Self::Eq => "eq_assistant",
        }
    }

    const fn evaluation_id(self) -> &'static str {
        match self {
            Self::Playlist => PLAYLIST_QUALITY_EVALUATION_ID,
            Self::Tagging => TAGGING_QUALITY_EVALUATION_ID,
            Self::TagCleanup => TAG_CLEANUP_QUALITY_EVALUATION_ID,
            Self::Eq => EQ_QUALITY_EVALUATION_ID,
        }
    }

    const fn job_kind(self) -> &'static str {
        match self {
            Self::Playlist => "assistant.model-evaluation.playlist-quality-v1",
            Self::Tagging => "assistant.model-evaluation.music-tagging-quality-v1",
            Self::TagCleanup => "assistant.model-evaluation.tag-cleanup-quality-v1",
            Self::Eq => "assistant.model-evaluation.eq-quality-v1",
        }
    }
}

#[derive(Debug)]
pub(crate) struct ModelEvaluationJobHandler {
    kind: EvaluationKind,
    quality: Arc<ModelQualityService>,
    transport: Arc<dyn StructuredModelTransport>,
}

impl ModelEvaluationJobHandler {
    fn new(
        kind: EvaluationKind,
        quality: Arc<ModelQualityService>,
        transport: Arc<dyn StructuredModelTransport>,
    ) -> Self {
        Self {
            kind,
            quality,
            transport,
        }
    }

    async fn prepare(
        &self,
        parameters: &ModelEvaluationJobParameters,
    ) -> Result<ModelEvaluationExecution, JobHandlerError> {
        let retest = !parameters.case_ids.is_empty();
        let unique_cases = parameters.case_ids.iter().collect::<BTreeSet<_>>();
        if parameters.role_id != self.kind.role_id()
            || parameters.evaluation_id != self.kind.evaluation_id()
            || parameters.role_fingerprint.len() != 64
            || !parameters
                .role_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || retest != parameters.baseline_job_id.is_some()
            || parameters.case_ids.len() > 100
            || unique_cases.len() != parameters.case_ids.len()
            || parameters.case_ids.iter().any(|case_id| case_id.is_empty())
            || (retest && !matches!(self.kind, EvaluationKind::Tagging))
        {
            return Err(JobHandlerError::new("invalid model evaluation parameters"));
        }
        let execution = self
            .quality
            .prepare_evaluation_execution(&parameters.role_id, &parameters.evaluation_id)
            .await
            .map_err(|error| JobHandlerError::new(error.code()))?;
        if execution.definition.job_kind != self.kind.job_kind()
            || execution.role.fingerprint != parameters.role_fingerprint
        {
            return Err(JobHandlerError::new("role_changed"));
        }
        Ok(execution)
    }

    async fn execute_model(
        &self,
        context: &JobExecutionContext,
        role: &ResolvedRoleExecution,
        request: &StructuredModelRequest,
        usage: &mut ProviderUsageAccumulator,
    ) -> Result<StructuredModelResult, JobHandlerError> {
        execute_provider_request(context, self.transport.as_ref(), role, request, usage).await
    }
}

fn deterministic_tagger_execution_failure(error: &ModelTaskError) -> bool {
    matches!(
        error.code.as_str(),
        "model_execution_completion_endpoint_not_found"
            | "model_execution_destination_blocked"
            | "model_execution_failed_precondition"
            | "model_execution_forbidden"
            | "model_execution_invalid_request"
            | "model_execution_invalid_request_headers"
            | "model_execution_model_not_found"
            | "model_execution_output_schema_required"
            | "model_execution_parameter_unknown"
            | "model_execution_redirect_blocked"
            | "model_execution_request_too_large"
            | "model_execution_unauthorized"
            | "model_execution_unsupported_adapter"
            | "model_execution_unsupported_provider_feature"
    )
}

impl JobHandler for ModelEvaluationJobHandler {
    fn definition(&self) -> JobDefinition {
        evaluation_job_definition(self.kind)
    }

    fn execute<'a>(
        &'a self,
        context: &'a JobExecutionContext,
        parameters: Map<String, Value>,
    ) -> JobHandlerFuture<'a> {
        Box::pin(async move {
            let parameters =
                serde_json::from_value::<ModelEvaluationJobParameters>(Value::Object(parameters))
                    .map_err(|_| JobHandlerError::new("invalid model evaluation parameters"))?;
            let result = match self.kind {
                EvaluationKind::Playlist => self.execute_playlist(context, &parameters).await,
                EvaluationKind::Tagging => self.execute_tagging(context, &parameters).await,
                EvaluationKind::TagCleanup => self.execute_tag_cleanup(context, &parameters).await,
                EvaluationKind::Eq => self.execute_eq(context, &parameters).await,
            }?;
            Ok(Value::Object(result))
        })
    }
}

fn evaluation_job_definition(kind: EvaluationKind) -> JobDefinition {
    JobDefinition {
        kind: kind.job_kind(),
        schema_version: 1,
        lane: JobLane::Provider,
        restartable: false,
        checkpoint_policy: JobCheckpointPolicy::Replace,
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ModelEvaluationJobParameters {
    role_id: String,
    evaluation_id: String,
    role_fingerprint: String,
    #[serde(default)]
    case_ids: Vec<String>,
    #[serde(default)]
    baseline_job_id: Option<String>,
}

async fn load_tagging_baseline(
    context: &JobExecutionContext,
    parameters: &ModelEvaluationJobParameters,
    suite: &TagQualitySuite,
) -> Result<Vec<TagQualityCaseResult>, JobHandlerError> {
    let baseline_id = parameters
        .baseline_job_id
        .as_deref()
        .ok_or_else(|| JobHandlerError::new("evaluation_retest_baseline_unavailable"))?;
    let baseline = context
        .related_job(baseline_id)
        .await
        .map_err(JobHandlerError::from_execution)?
        .filter(|job| {
            job.status == JobStatus::Succeeded
                && job.kind == "assistant.model-evaluation.music-tagging-quality-v1"
        })
        .ok_or_else(|| JobHandlerError::new("evaluation_retest_baseline_unavailable"))?;
    let baseline_parameters_valid = baseline.parameters.get("role_id").and_then(Value::as_str)
        == Some(parameters.role_id.as_str())
        && baseline
            .parameters
            .get("evaluation_id")
            .and_then(Value::as_str)
            == Some(parameters.evaluation_id.as_str())
        && baseline
            .parameters
            .get("role_fingerprint")
            .and_then(Value::as_str)
            == Some(parameters.role_fingerprint.as_str())
        && baseline
            .parameters
            .get("case_ids")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
        && baseline
            .parameters
            .get("baseline_job_id")
            .is_none_or(Value::is_null);
    let result = baseline
        .result
        .ok_or_else(|| JobHandlerError::new("evaluation_retest_baseline_unavailable"))?;
    let identity_valid = result.get("schema_version").and_then(Value::as_str)
        == Some("assistant-model-quality-result/v1")
        && result.get("execution_scope").and_then(Value::as_str) == Some("full_suite")
        && result.get("role_id").and_then(Value::as_str) == Some(parameters.role_id.as_str())
        && result.get("evaluation_id").and_then(Value::as_str)
            == Some(parameters.evaluation_id.as_str())
        && result.get("role_fingerprint").and_then(Value::as_str)
            == Some(parameters.role_fingerprint.as_str());
    if !baseline_parameters_valid || !identity_valid {
        return Err(JobHandlerError::new("evaluation_retest_baseline_stale"));
    }
    let evaluation = result
        .get("evaluation")
        .and_then(Value::as_object)
        .ok_or_else(|| JobHandlerError::new("evaluation_retest_baseline_unavailable"))?;
    if evaluation.get("suite_id").and_then(Value::as_str) != Some(suite.id.as_str()) {
        return Err(JobHandlerError::new("evaluation_retest_baseline_stale"));
    }
    let cases = evaluation
        .get("cases")
        .and_then(Value::as_array)
        .ok_or_else(|| JobHandlerError::new("evaluation_retest_baseline_unavailable"))?
        .iter()
        .cloned()
        .map(|value| {
            serde_json::from_value::<TagQualityCaseResult>(value)
                .map_err(|_| JobHandlerError::new("evaluation_retest_baseline_unavailable"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expected = suite
        .cases
        .iter()
        .map(|case| (case.id.as_str(), case.gate, case.vocabulary))
        .collect::<Vec<_>>();
    let actual = cases
        .iter()
        .map(|case| (case.id.as_str(), case.gate, case.vocabulary))
        .collect::<Vec<_>>();
    let failed = cases
        .iter()
        .filter(|case| !case.passed)
        .map(|case| case.id.as_str())
        .collect::<BTreeSet<_>>();
    let requested = parameters
        .case_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if actual != expected || failed != requested {
        return Err(JobHandlerError::new("evaluation_retest_baseline_stale"));
    }
    Ok(cases)
}

fn merge_tagging_retest(
    baseline: Vec<TagQualityCaseResult>,
    retested: Vec<TagQualityCaseResult>,
) -> Result<Vec<TagQualityCaseResult>, JobHandlerError> {
    let mut replacements = retested
        .into_iter()
        .map(|result| (result.id.clone(), result))
        .collect::<BTreeMap<_, _>>();
    let merged = baseline
        .into_iter()
        .map(|result| replacements.remove(&result.id).unwrap_or(result))
        .collect::<Vec<_>>();
    if !replacements.is_empty() {
        return Err(JobHandlerError::new("evaluation_retest_baseline_stale"));
    }
    Ok(merged)
}

pub fn model_evaluation_job_handlers(
    quality: Arc<ModelQualityService>,
    transport: Arc<dyn StructuredModelTransport>,
) -> Vec<Arc<dyn JobHandler>> {
    [
        EvaluationKind::Playlist,
        EvaluationKind::Tagging,
        EvaluationKind::TagCleanup,
        EvaluationKind::Eq,
    ]
    .into_iter()
    .map(|kind| {
        Arc::new(ModelEvaluationJobHandler::new(
            kind,
            Arc::clone(&quality),
            Arc::clone(&transport),
        )) as Arc<dyn JobHandler>
    })
    .collect()
}

#[derive(Debug, Clone, Copy)]
enum FeatureKind {
    Playlist,
    Eq,
    TagCleanup,
    Tagging,
}

#[derive(Debug)]
struct ModelFeatureJobHandler {
    kind: FeatureKind,
    quality: Arc<ModelQualityService>,
    transport: Arc<dyn StructuredModelTransport>,
    assistant: Arc<AssistantService>,
    local_analysis: Arc<LocalAnalysisService>,
    analysis_repository: Arc<dyn LocalAnalysisRepository>,
}

impl ModelFeatureJobHandler {}

impl JobHandler for ModelFeatureJobHandler {
    fn definition(&self) -> JobDefinition {
        feature_job_definition(self.kind)
    }

    fn execute<'a>(
        &'a self,
        context: &'a JobExecutionContext,
        parameters: Map<String, Value>,
    ) -> JobHandlerFuture<'a> {
        Box::pin(async move {
            match self.kind {
                FeatureKind::Playlist => {
                    let parameters = serde_json::from_value(Value::Object(parameters))
                        .map_err(|_| JobHandlerError::new("invalid playlist model parameters"))?;
                    self.execute_playlist(context, parameters).await
                }
                FeatureKind::Eq => {
                    let parameters = serde_json::from_value(Value::Object(parameters))
                        .map_err(|_| JobHandlerError::new("invalid EQ model parameters"))?;
                    self.execute_eq(context, parameters).await
                }
                FeatureKind::TagCleanup => {
                    let parameters =
                        serde_json::from_value(Value::Object(parameters)).map_err(|_| {
                            JobHandlerError::new("invalid tag cleanup model parameters")
                        })?;
                    self.execute_tag_cleanup(context, parameters).await
                }
                FeatureKind::Tagging => {
                    let parameters = serde_json::from_value(Value::Object(parameters))
                        .map_err(|_| JobHandlerError::new("invalid music tagging parameters"))?;
                    self.execute_tagging(context, parameters).await
                }
            }
        })
    }
}

fn feature_job_definition(kind: FeatureKind) -> JobDefinition {
    JobDefinition {
        kind: match kind {
            FeatureKind::Playlist => MODEL_PLAYLIST_SUGGESTION_JOB_KIND,
            FeatureKind::Eq => MODEL_EQ_DRAFT_JOB_KIND,
            FeatureKind::TagCleanup => MODEL_TAG_CLEANUP_JOB_KIND,
            FeatureKind::Tagging => MODEL_TAGGING_JOB_KIND,
        },
        schema_version: 1,
        lane: JobLane::Provider,
        restartable: false,
        checkpoint_policy: JobCheckpointPolicy::Replace,
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ModelPlaylistJobParameters {
    role_id: String,
    quality_evaluation_id: String,
    disclosure_version: String,
    consent: bool,
    role_fingerprint: String,
    request: PlaylistRequestParameters,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PlaylistRequestParameters {
    prompt: String,
    target_minutes: u16,
    candidate_limit: u16,
    min_bpm: Option<u32>,
    max_bpm: Option<u32>,
    include_unknown_bpm: bool,
    exclude_track_ids: Vec<i64>,
    energy_curve: EnergyCurve,
}

impl PlaylistRequestParameters {
    fn application_request(&self) -> Result<PlaylistSuggestionRequest, JobHandlerError> {
        let request = PlaylistSuggestionRequest {
            prompt: self.prompt.trim().to_owned(),
            target_minutes: self.target_minutes,
            candidate_limit: self.candidate_limit,
            min_bpm: self.min_bpm,
            max_bpm: self.max_bpm,
            include_unknown_bpm: self.include_unknown_bpm,
            exclude_track_ids: self
                .exclude_track_ids
                .iter()
                .map(|track_id| music_domain::TrackId::new(*track_id))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| JobHandlerError::new("invalid playlist model parameters"))?,
            energy_curve: self.energy_curve,
        };
        request
            .validate()
            .map_err(|_| JobHandlerError::new("invalid playlist model parameters"))?;
        Ok(request)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ModelEqJobParameters {
    role_id: String,
    quality_evaluation_id: String,
    disclosure_version: String,
    consent: bool,
    role_fingerprint: String,
    request: EqRequestParameters,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EqRequestParameters {
    name: String,
    goal: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ModelTagCleanupJobParameters {
    role_id: String,
    quality_evaluation_id: String,
    disclosure_version: String,
    consent: bool,
    role_fingerprint: String,
    catalog_signature: String,
    vocabulary_fingerprint: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ModelTaggingJobParameters {
    role_id: String,
    quality_evaluation_id: String,
    disclosure_version: String,
    consent: bool,
    role_fingerprint: String,
    vocabulary_fingerprint: String,
    scope: ModelTaggingScopeParameters,
    context_policy: ModelTaggingContextPolicy,
    force: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ModelTaggingScopeParameters {
    #[serde(rename = "type")]
    kind: ModelTaggingScopeKind,
    path: String,
    recursive: bool,
    track_ids: Vec<i64>,
}

impl ModelTaggingScopeParameters {
    fn application_scope(&self) -> Result<ContextScope, JobHandlerError> {
        if self.path.len() > 1_024 || self.track_ids.len() > 5_000 {
            return Err(JobHandlerError::new("invalid music tagging scope"));
        }
        let mut seen = BTreeSet::new();
        if self
            .track_ids
            .iter()
            .any(|id| *id <= 0 || !seen.insert(*id))
        {
            return Err(JobHandlerError::new("invalid music tagging scope"));
        }
        match self.kind {
            ModelTaggingScopeKind::All if self.path.is_empty() && self.track_ids.is_empty() => {
                Ok(ContextScope::All)
            }
            ModelTaggingScopeKind::Folder if self.track_ids.is_empty() => {
                let path = if self.path.is_empty() {
                    None
                } else {
                    Some(
                        LibraryPath::parse(&self.path)
                            .map_err(|_| JobHandlerError::new("invalid music tagging scope"))?,
                    )
                };
                Ok(ContextScope::Folder {
                    path,
                    recursive: self.recursive,
                })
            }
            ModelTaggingScopeKind::Tracks if self.path.is_empty() && !self.track_ids.is_empty() => {
                Ok(ContextScope::Tracks(
                    self.track_ids
                        .iter()
                        .map(|id| music_domain::TrackId::new(*id))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| JobHandlerError::new("invalid music tagging scope"))?,
                ))
            }
            _ => Err(JobHandlerError::new("invalid music tagging scope")),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum ModelTaggingScopeKind {
    All,
    Folder,
    Tracks,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum ModelTaggingContextPolicy {
    Include,
    Skip,
}

pub fn model_feature_job_handlers(
    quality: Arc<ModelQualityService>,
    transport: Arc<dyn StructuredModelTransport>,
    assistant: Arc<AssistantService>,
    local_analysis: Arc<LocalAnalysisService>,
    analysis_repository: Arc<dyn LocalAnalysisRepository>,
) -> Vec<Arc<dyn JobHandler>> {
    [
        FeatureKind::Playlist,
        FeatureKind::Eq,
        FeatureKind::TagCleanup,
        FeatureKind::Tagging,
    ]
    .into_iter()
    .map(|kind| {
        Arc::new(ModelFeatureJobHandler {
            kind,
            quality: Arc::clone(&quality),
            transport: Arc::clone(&transport),
            assistant: Arc::clone(&assistant),
            local_analysis: Arc::clone(&local_analysis),
            analysis_repository: Arc::clone(&analysis_repository),
        }) as Arc<dyn JobHandler>
    })
    .collect()
}

#[allow(clippy::too_many_arguments)]
fn validate_feature_header(
    role_id: &str,
    expected_role_id: &str,
    evaluation_id: &str,
    expected_evaluation_id: &str,
    disclosure_version: &str,
    expected_disclosure_version: &str,
    consent: bool,
    fingerprint: &str,
) -> Result<(), JobHandlerError> {
    if role_id != expected_role_id
        || evaluation_id != expected_evaluation_id
        || disclosure_version != expected_disclosure_version
        || !consent
        || fingerprint.len() != 64
        || !fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(JobHandlerError::new("invalid model feature parameters"));
    }
    Ok(())
}

async fn ensure_feature_role_unchanged(
    quality: &ModelQualityService,
    role_id: &str,
    evaluation_id: &str,
    fingerprint: &str,
) -> Result<(), JobHandlerError> {
    let current = quality
        .prepare_quality_gated_role_execution(role_id, evaluation_id)
        .await
        .map_err(|error| JobHandlerError::new(error.code()))?;
    if current.fingerprint != fingerprint {
        return Err(JobHandlerError::new("role_changed"));
    }
    Ok(())
}

async fn ensure_vocabulary_unchanged(
    assistant: &AssistantService,
    fingerprint: &str,
) -> Result<(), JobHandlerError> {
    let current = assistant
        .vocabulary()
        .await
        .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?;
    if current.fingerprint != fingerprint {
        return Err(JobHandlerError::new("tag_vocabulary_changed"));
    }
    Ok(())
}

async fn update_progress(
    context: &JobExecutionContext,
    current: usize,
    total: usize,
    phase: &str,
    message: impl Into<String>,
) -> Result<(), JobHandlerError> {
    let current =
        u64::try_from(current).map_err(|_| JobHandlerError::new("job progress overflow"))?;
    let total = u64::try_from(total).map_err(|_| JobHandlerError::new("job progress overflow"))?;
    let progress = JobProgress::new(current, Some(total), phase, message)
        .map_err(|_| JobHandlerError::new("invalid job progress"))?;
    context
        .update_progress(progress)
        .await
        .map_err(JobHandlerError::from_execution)
}

fn quality_result<T: serde::Serialize>(
    parameters: &ModelEvaluationJobParameters,
    execution_scope: &str,
    evaluation: &T,
    usage: &ProviderUsageAccumulator,
) -> Result<Map<String, Value>, JobHandlerError> {
    json!({
        "schema_version": "assistant-model-quality-result/v1",
        "execution_scope": execution_scope,
        "role_id": parameters.role_id,
        "evaluation_id": parameters.evaluation_id,
        "role_fingerprint": parameters.role_fingerprint,
        "evaluation": evaluation,
        "usage": usage.summary(),
    })
    .as_object()
    .cloned()
    .ok_or_else(|| JobHandlerError::new("model evaluation result encoding failed"))
}

fn model_task_failure(error: ModelTaskError) -> JobHandlerError {
    JobHandlerError::new(error.code)
}

#[cfg(test)]
mod tests {
    use super::{
        EvaluationKind, FeatureKind, deterministic_tagger_execution_failure,
        evaluation_job_definition, feature_job_definition,
    };
    use crate::assistant::ModelTaskError;
    use crate::jobs::JobLane;

    #[test]
    fn provider_quality_jobs_are_non_restartable_and_serialized_on_provider_lane() {
        for kind in [
            EvaluationKind::Playlist,
            EvaluationKind::Tagging,
            EvaluationKind::TagCleanup,
            EvaluationKind::Eq,
        ] {
            let definition = evaluation_job_definition(kind);
            assert_eq!(definition.lane, JobLane::Provider);
            assert!(!definition.restartable);
        }
    }

    #[test]
    fn tagging_quality_stops_repeating_deterministic_provider_request_failures() {
        assert!(deterministic_tagger_execution_failure(
            &ModelTaskError::new("model_execution_invalid_request")
        ));
        assert!(deterministic_tagger_execution_failure(
            &ModelTaskError::new("model_execution_parameter_unknown")
        ));
        assert!(!deterministic_tagger_execution_failure(
            &ModelTaskError::new("model_execution_timeout")
        ));
        assert!(!deterministic_tagger_execution_failure(
            &ModelTaskError::new("model_output_schema_invalid")
        ));
    }

    #[test]
    fn provider_feature_jobs_are_non_restartable_and_serialized_on_provider_lane() {
        for kind in [
            FeatureKind::Playlist,
            FeatureKind::Eq,
            FeatureKind::TagCleanup,
            FeatureKind::Tagging,
        ] {
            let definition = feature_job_definition(kind);
            assert_eq!(definition.lane, JobLane::Provider);
            assert!(!definition.restartable);
        }
    }
}
