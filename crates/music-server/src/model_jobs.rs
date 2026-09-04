use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use music_application::assistant::{
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
use music_application::jobs::{
    JobCheckpointPolicy, JobDefinition, JobExecutionContext, JobHandler, JobHandlerError,
    JobHandlerFuture, JobLane, JobProgress, JobStatus,
};
use music_domain::LibraryPath;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::provider_transport::ProviderNetworkBoundary;

pub(crate) const MODEL_PLAYLIST_SUGGESTION_JOB_KIND: &str = "assistant.model-playlist-suggestion";
pub(crate) const MODEL_EQ_DRAFT_JOB_KIND: &str = "assistant.model-eq-draft";
pub(crate) const MODEL_TAG_CLEANUP_JOB_KIND: &str = "assistant.model-tag-cleanup";
pub(crate) const MODEL_TAGGING_JOB_KIND: &str = "assistant.model-music-tagging";

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
    network: Arc<ProviderNetworkBoundary>,
}

impl ModelEvaluationJobHandler {
    fn new(
        kind: EvaluationKind,
        quality: Arc<ModelQualityService>,
        network: Arc<ProviderNetworkBoundary>,
    ) -> Self {
        Self {
            kind,
            quality,
            network,
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
        context
            .check_cancelled()
            .await
            .map_err(JobHandlerError::from_execution)?;
        let result = self
            .network
            .execute_structured_model_request(&role.execution, request)
            .await;
        usage.record(&result);
        context
            .checkpoint(usage.checkpoint())
            .await
            .map_err(JobHandlerError::from_execution)?;
        Ok(result)
    }

    async fn execute_playlist(
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
        let mut usage = ProviderUsageAccumulator::default();
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

    async fn execute_playlist_task(
        &self,
        context: &JobExecutionContext,
        role: &ResolvedRoleExecution,
        task: &ModelPlaylistTask,
        usage: &mut ProviderUsageAccumulator,
    ) -> Result<
        Result<music_application::assistant::PlaylistSuggestion, ModelTaskError>,
        JobHandlerError,
    > {
        if let Some(result) = task.immediate_result() {
            return Ok(Ok(result));
        }
        let request = task
            .request()
            .ok_or_else(|| JobHandlerError::new("playlist model task is incomplete"))?;
        let result = self.execute_model(context, role, &request, usage).await?;
        Ok(task.finish(result))
    }

    async fn execute_eq(
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

    async fn execute_tag_cleanup(
        &self,
        context: &JobExecutionContext,
        parameters: &ModelEvaluationJobParameters,
    ) -> Result<Map<String, Value>, JobHandlerError> {
        let suite = tag_cleanup_quality_suite().map_err(model_task_failure)?;
        update_progress(
            context,
            0,
            suite.cases.len(),
            "Preparing evaluation",
            "Loading fixed synthetic tag-cleanup cases",
        )
        .await?;
        let execution = self.prepare(parameters).await?;
        let vocabulary = default_vocabulary_snapshot().map_err(model_task_failure)?;
        let mut usage = ProviderUsageAccumulator::default();
        let mut results = Vec::with_capacity(suite.cases.len());
        for (index, case) in suite.cases.iter().enumerate() {
            let mut task = ModelTagCleanupTask::new(&case.usage(), vocabulary.clone())
                .map_err(model_task_failure)?;
            let mut failure = None;
            while let Some(request) = task.next_request() {
                let model_result = self
                    .execute_model(context, &execution.role, &request, &mut usage)
                    .await?;
                if let Err(error) = task.accept(model_result) {
                    failure = Some(error);
                    break;
                }
            }
            let result = match failure {
                Some(error) => Err(error),
                None => task
                    .finish()
                    .ok_or_else(|| ModelTaskError::new("model_cleanup_incomplete")),
            };
            results.push(case.assess(result));
            update_progress(
                context,
                index + 1,
                suite.cases.len(),
                "Evaluating tag cleanup model",
                format!(
                    "Completed {} of {} synthetic cases",
                    index + 1,
                    suite.cases.len()
                ),
            )
            .await?;
        }
        let result = TagCleanupQualityEvaluationResult::from_cases(&suite, results);
        context
            .check_cancelled()
            .await
            .map_err(JobHandlerError::from_execution)?;
        self.quality
            .record_evaluation(
                &execution,
                context.job_id(),
                MODEL_TAG_CLEANUP_ENGINE_ID,
                result.passed,
                result.passed_cases,
                result.total_cases,
            )
            .await
            .map_err(|error| JobHandlerError::new(error.code()))?;
        quality_result(parameters, "full_suite", &result, &usage)
    }

    async fn execute_tagging(
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
        let total_attempts = execution_cases.len().saturating_add(safety_count);
        update_progress(
            context,
            0,
            total_attempts,
            "Preparing evaluation",
            format!(
                "Loading {} {} tagging scenarios; {} safety reruns make {} scored attempts",
                execution_cases.len(),
                if retest { "failed" } else { "fixed" },
                safety_count,
                total_attempts,
            ),
        )
        .await?;
        let execution = self.prepare(parameters).await?;
        let vocabulary = default_vocabulary_snapshot().map_err(model_task_failure)?;
        let mut usage = ProviderUsageAccumulator::default();
        let mut retry_budget = MODEL_TAGGER_INVALID_RESPONSE_RETRY_LIMIT;
        let mut completed = 0_usize;
        let mut deterministic_execution_failure = None;
        let results = self
            .evaluate_tagging_cases(
                context,
                &execution.role,
                &execution_cases,
                &vocabulary,
                &mut usage,
                &mut retry_budget,
                &mut completed,
                total_attempts,
                execution_cases.len(),
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
                &vocabulary,
                &mut usage,
                &mut retry_budget,
                &mut completed,
                total_attempts,
                execution_cases.len(),
                &mut deterministic_execution_failure,
            )
            .await?;
        let evaluated = merge_safety_repeats(results, repeats).map_err(model_task_failure)?;
        let merged = match baseline {
            Some(baseline) => merge_tagging_retest(baseline, evaluated)?,
            None => evaluated,
        };
        let result =
            TagQualityEvaluationResult::summarize(&suite, merged).map_err(model_task_failure)?;
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
        vocabulary: &music_application::assistant::TagVocabularySnapshot,
        usage: &mut ProviderUsageAccumulator,
        retry_budget: &mut u8,
        completed: &mut usize,
        total_attempts: usize,
        scenario_count: usize,
        deterministic_execution_failure: &mut Option<ModelTaskError>,
    ) -> Result<Vec<TagQualityCaseResult>, JobHandlerError> {
        let mut results = Vec::with_capacity(cases.len());
        let inputs = cases
            .iter()
            .map(|case| case.track.clone())
            .collect::<Vec<_>>();
        let batches = music_application::assistant::plan_model_tagger_batches(
            &inputs,
            vocabulary,
            |request| {
                crate::provider_handlers::validate_structured_request(&role.execution, request)
            },
        )
        .map_err(model_task_failure)?;
        for planned in batches {
            let chunk = &cases[planned.input_range];
            let batch = planned.task;
            let profiles = if let Some(error) = deterministic_execution_failure.clone() {
                Err(error)
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
                && deterministic_tagger_execution_failure(error)
            {
                *deterministic_execution_failure = Some(error.clone());
            }
            for case in chunk {
                let result = match &profiles {
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
                results.push(result);
                *completed = completed.saturating_add(1);
                update_progress(
                    context,
                    *completed,
                    total_attempts,
                    "Evaluating tagging model",
                    format!(
                        "Completed {} of {} scored attempts across {} scenarios",
                        *completed, total_attempts, scenario_count,
                    ),
                )
                .await?;
            }
        }
        Ok(results)
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

#[derive(Debug, Deserialize)]
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
        .map(|case| (case.id.as_str(), case.gate))
        .collect::<Vec<_>>();
    let actual = cases
        .iter()
        .map(|case| (case.id.as_str(), case.gate))
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

pub(crate) fn model_evaluation_job_handlers(
    quality: Arc<ModelQualityService>,
    network: Arc<ProviderNetworkBoundary>,
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
            Arc::clone(&network),
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
    network: Arc<ProviderNetworkBoundary>,
    assistant: Arc<AssistantService>,
    local_analysis: Arc<LocalAnalysisService>,
    analysis_repository: Arc<dyn LocalAnalysisRepository>,
}

impl ModelFeatureJobHandler {
    async fn execute_playlist(
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
        let mut usage = ProviderUsageAccumulator::default();
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
            let result =
                execute_provider_request(context, &self.network, &role, &request, &mut usage)
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
            "suggestion": music_application::assistant::playlist_suggestion_payload(&suggestion),
            "usage": usage.summary(),
        }))
    }

    async fn execute_eq(
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
        let result =
            execute_provider_request(context, &self.network, &role, &task.request(), &mut usage)
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

    async fn execute_tag_cleanup(
        &self,
        context: &JobExecutionContext,
        parameters: ModelTagCleanupJobParameters,
    ) -> Result<Value, JobHandlerError> {
        validate_feature_header(
            &parameters.role_id,
            "tag_cleanup",
            &parameters.quality_evaluation_id,
            TAG_CLEANUP_QUALITY_EVALUATION_ID,
            &parameters.disclosure_version,
            "assistant-model-tag-cleanup-disclosure/v3",
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
        if role.fingerprint != parameters.role_fingerprint {
            return Err(JobHandlerError::new("role_changed"));
        }
        let usage_snapshot = self
            .assistant
            .tag_usage()
            .await
            .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?;
        let vocabulary = self
            .assistant
            .vocabulary()
            .await
            .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?;
        let current_catalog_signature = catalog_signature(&usage_snapshot)
            .map_err(|_| JobHandlerError::new("tag_catalog_invalid"))?;
        if current_catalog_signature != parameters.catalog_signature {
            return Err(JobHandlerError::new("tag_catalog_changed"));
        }
        if vocabulary.fingerprint != parameters.vocabulary_fingerprint {
            return Err(JobHandlerError::new("tag_vocabulary_changed"));
        }
        if usage_snapshot.is_empty() {
            return Err(JobHandlerError::new("tag_catalog_empty"));
        }
        if usage_snapshot.len() > MAX_MODEL_CLEANUP_TAGS {
            return Err(JobHandlerError::new("tag_catalog_too_large"));
        }
        let mut task = ModelTagCleanupTask::new(&usage_snapshot, vocabulary.clone())
            .map_err(model_task_failure)?;
        let total_batches = task.total_model_batches();
        let progress_total = total_batches.max(1);
        update_progress(
            context,
            0,
            progress_total,
            if total_batches == 0 {
                "Applying deterministic cleanup rules"
            } else {
                "Waiting for tag cleanup model"
            },
            if total_batches == 0 {
                "All cleanup candidates were resolved locally".to_owned()
            } else {
                format!("Reviewing unresolved tag names in {total_batches} bounded batches")
            },
        )
        .await?;
        let mut provider_usage = ProviderUsageAccumulator::default();
        while let Some(request) = task.next_request() {
            ensure_feature_role_unchanged(
                &self.quality,
                &parameters.role_id,
                &parameters.quality_evaluation_id,
                &parameters.role_fingerprint,
            )
            .await?;
            ensure_vocabulary_unchanged(&self.assistant, &parameters.vocabulary_fingerprint)
                .await?;
            let result = execute_provider_request(
                context,
                &self.network,
                &role,
                &request,
                &mut provider_usage,
            )
            .await?;
            task.accept(result).map_err(model_task_failure)?;
            update_progress(
                context,
                task.completed_model_batches(),
                progress_total,
                "Waiting for tag cleanup model",
                format!(
                    "Completed {} of {} provider batches",
                    task.completed_model_batches(),
                    total_batches
                ),
            )
            .await?;
        }
        let suggestions = task
            .finish()
            .ok_or_else(|| JobHandlerError::new("model_cleanup_incomplete"))?;
        ensure_feature_role_unchanged(
            &self.quality,
            &parameters.role_id,
            &parameters.quality_evaluation_id,
            &parameters.role_fingerprint,
        )
        .await?;
        ensure_vocabulary_unchanged(&self.assistant, &parameters.vocabulary_fingerprint).await?;
        let final_usage = self
            .assistant
            .tag_usage()
            .await
            .map_err(|_| JobHandlerError::new("assistant_storage_failed"))?;
        if catalog_signature(&final_usage)
            .map_err(|_| JobHandlerError::new("tag_catalog_invalid"))?
            != parameters.catalog_signature
        {
            return Err(JobHandlerError::new("tag_catalog_changed"));
        }
        let counts = usage_snapshot
            .iter()
            .map(|item| (item.tag.as_str(), item.track_count))
            .collect::<BTreeMap<_, _>>();
        let local_pairs = build_cleanup_preview(&usage_snapshot, &vocabulary)
            .map_err(|_| JobHandlerError::new("tag_cleanup_preview_failed"))?
            .suggestions
            .into_iter()
            .map(|item| (item.source, item.target))
            .collect::<BTreeSet<_>>();
        let output = suggestions
            .iter()
            .map(|suggestion| {
                json!({
                    "id": model_tag_cleanup_suggestion_id(
                        &parameters.role_fingerprint,
                        &parameters.catalog_signature,
                        &parameters.vocabulary_fingerprint,
                        suggestion,
                    ),
                    "source": suggestion.source,
                    "target": suggestion.target,
                    "origin": if local_pairs.contains(&(suggestion.source.clone(), suggestion.target.clone())) {
                        "local-rule"
                    } else {
                        "model"
                    },
                    "confidence": suggestion.confidence.as_str(),
                    "reason": suggestion.reason,
                    "source_track_count": counts.get(suggestion.source.as_str()).copied().unwrap_or(0),
                    "target_track_count": counts.get(suggestion.target.as_str()).copied().unwrap_or(0),
                    "merged": counts.contains_key(suggestion.target.as_str()),
                })
            })
            .collect::<Vec<_>>();
        update_progress(
            context,
            progress_total,
            progress_total,
            "Saving cleanup proposal",
            format!("Saved {} review-only suggestions", output.len()),
        )
        .await?;
        Ok(json!({
            "schema_version": "assistant-model-tag-cleanup-job-result/v3",
            "disclosure_version": parameters.disclosure_version,
            "role_id": parameters.role_id,
            "role_fingerprint": parameters.role_fingerprint,
            "engine_id": MODEL_TAG_CLEANUP_ENGINE_ID,
            "catalog_signature": parameters.catalog_signature,
            "vocabulary_fingerprint": parameters.vocabulary_fingerprint,
            "catalog_tags": usage_snapshot.len(),
            "suggestions": output,
            "usage": provider_usage.summary(),
        }))
    }

    async fn execute_tagging(
        &self,
        context: &JobExecutionContext,
        parameters: ModelTaggingJobParameters,
    ) -> Result<Value, JobHandlerError> {
        validate_feature_header(
            &parameters.role_id,
            "music_tagger",
            &parameters.quality_evaluation_id,
            TAGGING_QUALITY_EVALUATION_ID,
            &parameters.disclosure_version,
            "assistant-model-music-tagging-disclosure/v11",
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
        if role.fingerprint != parameters.role_fingerprint {
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
                    &parameters.role_fingerprint,
                    &parameters.vocabulary_fingerprint,
                    contexts.get(&track.track.id),
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
                        analysis.analyzer_id == MODEL_TAG_ANALYZER_ID
                            && signatures
                                .get(&track.track.id)
                                .is_some_and(|signature| analysis.source_signature == *signature)
                    })
            })
            .collect::<Vec<_>>();
        let total = work.len();
        let inputs = work
            .iter()
            .map(|track| model_tag_track_input(&track.track, contexts.get(&track.track.id)))
            .collect::<Vec<_>>();
        let batches = music_application::assistant::plan_model_tagger_batches(
            &inputs,
            &vocabulary,
            |request| {
                crate::provider_handlers::validate_structured_request(&role.execution, request)
            },
        )
        .map_err(model_task_failure)?;
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
        let mut provider_usage = ProviderUsageAccumulator::default();
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
            let profiles = loop {
                let result = execute_provider_request(
                    context,
                    &self.network,
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
                    let (energy, brightness, tension) =
                        local_context_axes(contexts.get(&track.track.id));
                    let context_status = contexts
                        .get(&track.track.id)
                        .map(|context| context.completeness.as_str())
                        .unwrap_or("missing");
                    let confidence = match model.confidence {
                        TagConfidence::High => Confidence::High,
                        TagConfidence::Medium => Confidence::Medium,
                        TagConfidence::Low => Confidence::Low,
                    };
                    Ok(ModelAnalysisWrite {
                        profile: AnalysisWrite {
                            track_id: track.track.id,
                            source_signature: signatures
                                .get(&track.track.id)
                                .cloned()
                                .ok_or_else(|| JobHandlerError::new("model_tag_source_invalid"))?,
                            energy,
                            brightness,
                            tension,
                            moods: model.tags.clone(),
                            evidence: model.evidence.clone(),
                            metrics: json!({
                                "contract": "assistant-music-tagger-output/v3",
                                "input_contract": MODEL_TAGGER_INPUT_CONTRACT,
                                "context_status": context_status,
                                "role_fingerprint": parameters.role_fingerprint,
                                "vocabulary_fingerprint": parameters.vocabulary_fingerprint,
                            })
                            .as_object()
                            .cloned()
                            .ok_or_else(|| JobHandlerError::new("model_tag_profile_invalid"))?,
                            confidence,
                        },
                    })
                })
                .collect::<Result<Vec<_>, JobHandlerError>>()?;
            let stored = self
                .analysis_repository
                .store_model_analysis(
                    MODEL_TAG_ANALYZER_ID,
                    context.job_id(),
                    &parameters.role_fingerprint,
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
            update_progress(
                context,
                (start + batch.len()).min(total),
                total,
                "Saving reviewable suggestions",
                format!("Processed {} of {total} tracks", start + batch.len()),
            )
            .await?;
        }
        Ok(json!({
            "schema_version": "assistant-model-music-tagging-job-result/v6",
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
            "updated_profiles": updated,
            "unchanged_profiles": planned.len().saturating_sub(work.len()),
            "skipped_changed_tracks": skipped_changed,
            "usage": provider_usage.summary(),
        }))
    }
}

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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelPlaylistJobParameters {
    role_id: String,
    quality_evaluation_id: String,
    disclosure_version: String,
    consent: bool,
    role_fingerprint: String,
    request: PlaylistRequestParameters,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelEqJobParameters {
    role_id: String,
    quality_evaluation_id: String,
    disclosure_version: String,
    consent: bool,
    role_fingerprint: String,
    request: EqRequestParameters,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EqRequestParameters {
    name: String,
    goal: String,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
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

pub(crate) fn model_feature_job_handlers(
    quality: Arc<ModelQualityService>,
    network: Arc<ProviderNetworkBoundary>,
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
            network: Arc::clone(&network),
            assistant: Arc::clone(&assistant),
            local_analysis: Arc::clone(&local_analysis),
            analysis_repository: Arc::clone(&analysis_repository),
        }) as Arc<dyn JobHandler>
    })
    .collect()
}

async fn execute_provider_request(
    context: &JobExecutionContext,
    network: &ProviderNetworkBoundary,
    role: &ResolvedRoleExecution,
    request: &StructuredModelRequest,
    usage: &mut ProviderUsageAccumulator,
) -> Result<StructuredModelResult, JobHandlerError> {
    context
        .check_cancelled()
        .await
        .map_err(JobHandlerError::from_execution)?;
    let result = network
        .execute_structured_model_request(&role.execution, request)
        .await;
    usage.record(&result);
    context
        .checkpoint(usage.checkpoint())
        .await
        .map_err(JobHandlerError::from_execution)?;
    Ok(result)
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
    use music_application::assistant::ModelTaskError;
    use music_application::jobs::JobLane;

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
