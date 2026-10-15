//! Model tagging: availability, planning, starting runs and managing provider batches.

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use music_application::assistant::{
    MODEL_TAG_ANALYZER_ID, MODEL_TAG_BATCH_SIZE, MODEL_TAGGER_INVALID_RESPONSE_RETRY_LIMIT,
    ManualTagQuery, TagVocabularySnapshot, model_tag_source_signature,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;

use crate::analysis::ContextScopeParameters;
use crate::auth::Operator;
use crate::error::{ApiError, HttpValidationErrorBody};
use crate::http::HttpState;
use crate::jobs::BackgroundJobResponse;
use crate::model_jobs::MODEL_TAGGING_JOB_KIND;
use crate::provider_api::map_model_quality_error;

use super::review::{LibraryTagPageResponse, track_response};
use super::schema::{
    model_tagging_context_policy_schema, model_tagging_disclosure_version_schema,
    model_tagging_quality_schema, model_tagging_retry_limit_schema, model_tagging_role_schema,
    model_tagging_scope_ref, model_tagging_tracks_per_request_schema, nonnegative_integer_schema,
    nullable_string_schema, review_status_default_pending_schema, true_schema,
};
use super::shared::{
    ModelTaggingScopeWire, ReviewStatusWire, analysis_service, default_page_limit,
    enqueue_model_feature, map_assistant_error, map_local_analysis_error, model_role_availability,
    service,
};

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
#[schema(as = ModelTaggingReviewQuery)]
pub(super) struct ModelTaggingReviewQueryRequest {
    #[schema(required = false, schema_with = model_tagging_scope_ref)]
    pub(super) scope: ModelTaggingScopeWire,
    #[schema(required = false, schema_with = review_status_default_pending_schema)]
    pub(super) review: ReviewStatusWire,
    #[schema(required = false, minimum = 0, default = 0)]
    pub(super) offset: usize,
    #[serde(default = "default_page_limit")]
    #[schema(required = false, minimum = 1, maximum = 100, default = 50)]
    pub(super) limit: usize,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum ModelTaggingContextPolicyWire {
    #[default]
    Include,
    Skip,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = ModelTaggingExecutionMode)]
pub(super) enum ModelTaggingExecutionModeWire {
    #[default]
    Standard,
    Batch,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
#[derive(Default)]
pub(super) struct ModelBatchActionRequest {
    pub(super) abandon_uncertain: bool,
    pub(super) cancel: bool,
    pub(super) remote_batch_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(super) struct ModelBatchStatusResponse {
    pub(super) id: String,
    pub(super) state: String,
    pub(super) input_file_id: Option<String>,
    pub(super) remote_batch_id: Option<String>,
    pub(super) result: Option<Value>,
}

#[utoipa::path(get, path = "/assistant/library-tags/model-batches/{batch_id}", params(("batch_id" = String, Path)), responses((status = 200, description = "Successful Response", body = ModelBatchStatusResponse)), tag = "assistant-tags")]
pub(super) async fn model_batch_status(
    State(state): State<HttpState>,
    _operator: Operator,
    Path(batch_id): Path<String>,
) -> Result<Json<ModelBatchStatusResponse>, ApiError> {
    let batch = state
        .providers
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?
        .batches
        .model_batch_summary(&batch_id)
        .await
        .map_err(|error| ApiError::internal_from("model batch summary failed", error.as_ref()))?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(ModelBatchStatusResponse {
        id: batch.id,
        state: batch.state.into(),
        input_file_id: batch.input_file_id,
        remote_batch_id: batch.remote_batch_id,
        result: batch.document.get("result").cloned(),
    }))
}

#[utoipa::path(post, path = "/assistant/library-tags/model-batches/{batch_id}/actions", params(("batch_id" = String, Path)), request_body = ModelBatchActionRequest, responses((status = 202, description = "Successful Response", body = BackgroundJobResponse)), tag = "assistant-tags")]
pub(super) async fn model_batch_action(
    State(state): State<HttpState>,
    _operator: Operator,
    Path(batch_id): Path<String>,
    payload: Result<Json<ModelBatchActionRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<BackgroundJobResponse>), ApiError> {
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if batch_id.len() != 32
        || !batch_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        || payload
            .remote_batch_id
            .as_ref()
            .is_some_and(|id| id.len() > 128)
    {
        return Err(ApiError::validation());
    }
    enqueue_model_feature(&state, music_application::assistant::MODEL_TAGGING_BATCH_COLLECT_JOB_KIND,
        json!({"batch_id":batch_id,"role_id":"music_tagger","cancel":payload.cancel,"remote_batch_id":payload.remote_batch_id,"abandon_uncertain":payload.abandon_uncertain}),
        "model_batch_action_in_progress", "A batch update is already running.").await
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
#[schema(as = ModelTaggingPlanRequest)]
pub(super) struct ModelTaggingPlanRequest {
    pub(super) execution_mode: ModelTaggingExecutionModeWire,
    pub(super) limits: ModelTaggingLimitsWire,
    #[schema(required = false, default = false)]
    pub(super) force: bool,
    #[schema(required = false, schema_with = model_tagging_scope_ref)]
    pub(super) scope: ModelTaggingScopeWire,
    #[schema(required = false, schema_with = model_tagging_context_policy_schema)]
    pub(super) context_policy: ModelTaggingContextPolicyWire,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelTaggingStartRequest)]
pub(super) struct ModelTaggingStartRequest {
    #[serde(default)]
    pub(super) execution_mode: ModelTaggingExecutionModeWire,
    #[serde(default)]
    pub(super) limits: ModelTaggingLimitsWire,
    #[serde(default)]
    #[schema(required = false, default = false)]
    pub(super) force: bool,
    #[serde(default)]
    #[schema(required = false, schema_with = model_tagging_scope_ref)]
    pub(super) scope: ModelTaggingScopeWire,
    #[serde(default)]
    #[schema(required = false, schema_with = model_tagging_context_policy_schema)]
    pub(super) context_policy: ModelTaggingContextPolicyWire,
    #[schema(schema_with = model_tagging_disclosure_version_schema)]
    pub(super) disclosure_version: String,
    #[schema(schema_with = true_schema)]
    pub(super) consent: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
#[schema(as = ModelTaggingLimits)]
pub(super) struct ModelTaggingLimitsWire {
    #[schema(required = true, minimum = 1, maximum = 10000, default = 20)]
    pub(super) max_tracks: usize,
    #[schema(required = true, minimum = 1, maximum = 1000, default = 10)]
    pub(super) max_requests: usize,
    #[schema(
        required = true,
        minimum = 1000,
        maximum = 100000000,
        default = 1000000
    )]
    pub(super) max_token_reservation: u64,
    #[schema(required = true, default = true)]
    pub(super) stop_on_empty_batch: bool,
}

impl Default for ModelTaggingLimitsWire {
    fn default() -> Self {
        let limits = music_application::assistant::ModelTaggingLimits::default();
        Self {
            max_tracks: limits.max_tracks,
            max_requests: limits.max_requests,
            max_token_reservation: limits.max_token_reservation,
            stop_on_empty_batch: limits.stop_on_empty_batch,
        }
    }
}

impl ModelTaggingLimitsWire {
    fn validate(self) -> Result<(), ApiError> {
        music_application::assistant::ModelTaggingLimits {
            max_tracks: self.max_tracks,
            max_requests: self.max_requests,
            max_token_reservation: self.max_token_reservation,
            stop_on_empty_batch: self.stop_on_empty_batch,
        }
        .validate()
        .map_err(|_| ApiError::validation())
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelTaggingDisclosure)]
pub(super) struct ModelTaggingDisclosureResponse {
    #[schema(schema_with = model_tagging_disclosure_version_schema)]
    pub(super) version: &'static str,
    pub(super) shared_with_provider: Vec<&'static str>,
    pub(super) never_shared: Vec<&'static str>,
    pub(super) allowed_tags: Vec<String>,
    #[schema(schema_with = model_tagging_tracks_per_request_schema)]
    pub(super) tracks_per_request: usize,
    #[schema(schema_with = model_tagging_retry_limit_schema)]
    pub(super) invalid_response_retry_limit: u8,
    pub(super) may_incur_cost: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelTaggingAvailability)]
pub(super) struct ModelTaggingAvailabilityResponse {
    pub(super) execution_mode: ModelTaggingExecutionModeWire,
    pub(super) batch_available: bool,
    pub(super) pending_batch_id: Option<String>,
    pub(super) limits: ModelTaggingLimitsWire,
    pub(super) run_tracks: usize,
    pub(super) deferred_tracks: usize,
    pub(super) token_reservation: u64,
    pub(super) available: bool,
    #[schema(required = true, schema_with = nullable_string_schema)]
    pub(super) reason_code: Option<String>,
    #[schema(schema_with = model_tagging_role_schema)]
    pub(super) role_id: &'static str,
    #[schema(required = true, schema_with = nullable_string_schema)]
    pub(super) connection_name: Option<String>,
    #[schema(required = true, schema_with = nullable_string_schema)]
    pub(super) model_id: Option<String>,
    #[schema(schema_with = model_tagging_quality_schema)]
    pub(super) quality_evaluation_id: &'static str,
    pub(super) job_kind: &'static str,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) library_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) scope_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) planned_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) tracks_with_full_context: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) tracks_with_partial_context: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) tracks_missing_context: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) current_profiles: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) tracks_needing_tags: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) estimated_provider_requests: usize,
    pub(super) disclosure: ModelTaggingDisclosureResponse,
}

#[utoipa::path(
    get,
    path = "/assistant/library-tags/model-status",
    operation_id = "model_music_tagging_status_api_assistant_library_tags_model_status_get",
    responses((status = 200, description = "Successful Response", body = ModelTaggingAvailabilityResponse)),
    tag = "assistant-tags"
)]
pub(super) async fn model_tagging_status(
    State(state): State<HttpState>,
    _operator: Operator,
) -> Result<Json<ModelTaggingAvailabilityResponse>, ApiError> {
    Ok(Json(
        model_tagging_availability(
            &state,
            ContextScopeParameters::default(),
            ModelTaggingContextPolicyWire::Include,
            false,
            ModelTaggingLimitsWire::default(),
            ModelTaggingExecutionModeWire::Standard,
        )
        .await?,
    ))
}

#[utoipa::path(
    post,
    path = "/assistant/library-tags/model-plan",
    operation_id = "plan_model_music_tagging_api_assistant_library_tags_model_plan_post",
    request_body = ModelTaggingPlanRequest,
    responses(
        (status = 200, description = "Successful Response", body = ModelTaggingAvailabilityResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant-tags"
)]
pub(super) async fn plan_model_tagging(
    State(state): State<HttpState>,
    _operator: Operator,
    payload: Result<Json<ModelTaggingPlanRequest>, JsonRejection>,
) -> Result<Json<ModelTaggingAvailabilityResponse>, ApiError> {
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let scope = payload.scope.into_parameters()?;
    Ok(Json(
        model_tagging_availability(
            &state,
            scope,
            payload.context_policy,
            payload.force,
            payload.limits,
            payload.execution_mode,
        )
        .await?,
    ))
}

#[utoipa::path(
    post,
    path = "/assistant/library-tags/model-jobs",
    operation_id = "start_model_music_tagging_api_assistant_library_tags_model_jobs_post",
    request_body = ModelTaggingStartRequest,
    responses(
        (status = 202, description = "Successful Response", body = BackgroundJobResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant-tags"
)]
pub(super) async fn start_model_tagging(
    State(state): State<HttpState>,
    _operator: Operator,
    payload: Result<Json<ModelTaggingStartRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<BackgroundJobResponse>), ApiError> {
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if payload.disclosure_version != "assistant-model-music-tagging-disclosure/v16"
        || !payload.consent
    {
        return Err(ApiError::validation());
    }
    let scope = payload.scope.into_parameters()?;
    scope.to_scope().map_err(|_| ApiError::validation())?;
    let plan = model_tagging_availability(
        &state,
        scope.clone(),
        payload.context_policy,
        payload.force,
        payload.limits,
        payload.execution_mode,
    )
    .await?;
    if let Some(code) = plan.reason_code {
        return Err(if code == "request_too_large" {
            ApiError::coded_conflict(
                "request_too_large",
                "The vocabulary and track metadata exceed the provider request limit. Shorten descriptions, aliases, or context cues before starting.",
            )
        } else {
            ApiError::coded_conflict(
                "model_tagging_unavailable",
                "Model tagging is unavailable; refresh the plan before starting.",
            )
        });
    }
    let providers = state
        .providers
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)?;
    let role = providers
        .quality_service()
        .prepare_quality_gated_role_execution("music_tagger", "music-tagging-quality-v1")
        .await
        .map_err(map_model_quality_error)?;
    let vocabulary = service(&state)?
        .vocabulary()
        .await
        .map_err(map_assistant_error)?;
    let parameters = json!({
        "role_id": "music_tagger",
        "quality_evaluation_id": "music-tagging-quality-v1",
        "inference_fingerprint": role.inference_fingerprint,
        "disclosure_version": payload.disclosure_version,
        "consent": true,
        "role_fingerprint": role.fingerprint,
        "vocabulary_fingerprint": vocabulary.fingerprint,
        "scope": scope,
        "context_policy": payload.context_policy,
        "force": payload.force,
        "limits": payload.limits,
        "execution_mode": payload.execution_mode,
    });
    enqueue_model_feature(
        &state,
        MODEL_TAGGING_JOB_KIND,
        parameters,
        "model_tagging_in_progress",
        "Another model music-tagging job is already running. Wait for it to finish or cancel it first.",
    )
    .await
}

pub(super) async fn model_tagging_availability(
    state: &HttpState,
    scope: ContextScopeParameters,
    context_policy: ModelTaggingContextPolicyWire,
    force: bool,
    limits: ModelTaggingLimitsWire,
    execution_mode: ModelTaggingExecutionModeWire,
) -> Result<ModelTaggingAvailabilityResponse, ApiError> {
    limits.validate()?;
    let scope_filter = scope.to_scope().map_err(|_| ApiError::validation())?;
    let mut role =
        model_role_availability(state, "music_tagger", "music-tagging-quality-v1").await?;
    let assistant = service(state)?;
    let tracks = assistant.tracks().await.map_err(map_assistant_error)?;
    let library_tracks = tracks.len();
    let scoped = tracks
        .iter()
        .filter(|track| scope_filter.contains(&track.track))
        .collect::<Vec<_>>();
    let indexed = scoped
        .iter()
        .map(|track| track.track.clone())
        .collect::<Vec<_>>();
    let contexts = analysis_service(state)?
        .current_contexts(&indexed)
        .await
        .map_err(map_local_analysis_error)?;
    let tracks_with_full_context = contexts
        .values()
        .filter(|context| context.completeness == "full")
        .count();
    let tracks_with_partial_context = contexts
        .values()
        .filter(|context| context.completeness == "partial")
        .count();
    let planned = scoped
        .iter()
        .copied()
        .filter(|track| {
            context_policy == ModelTaggingContextPolicyWire::Include
                || contexts
                    .get(&track.track.id)
                    .is_some_and(|context| context.completeness == "full")
        })
        .collect::<Vec<_>>();
    let vocabulary = assistant.vocabulary().await.map_err(map_assistant_error)?;
    let mut inputs = Vec::new();
    let current_profiles = if let Some(fingerprint) = role.inference_fingerprint.as_deref() {
        let mut current = 0_usize;
        for track in &planned {
            let signature = model_tag_source_signature(
                &track.track,
                fingerprint,
                &vocabulary.fingerprint,
                contexts.get(&track.track.id),
                track.catalog_evidence.as_ref(),
            )
            .map_err(|reason| {
                tracing::error!(%reason, "model tagging plan failed");
                ApiError::internal()
            })?;
            let is_current = track.analyses.iter().any(|analysis| {
                music_application::assistant::model_tag_profile_is_current(analysis, &signature)
            });
            if is_current {
                current = current.saturating_add(1);
            }
            if force || !is_current {
                inputs.push(music_application::assistant::model_tag_track_input(
                    &track.track,
                    contexts.get(&track.track.id),
                    track.catalog_evidence.as_ref(),
                ));
            }
        }
        current
    } else {
        0
    };
    let tracks_needing_tags = planned.len().saturating_sub(current_profiles);
    let batch_available = role.execution.as_ref().is_some_and(|role| {
        role.execution.adapter_id == music_application::assistant::OPENAI_RESPONSES_ADAPTER
    });
    let pending_batch_id = match state.providers.as_ref() {
        Some(providers) => providers
            .batches
            .pending_model_batch()
            .await
            .map_err(|error| {
                ApiError::internal_from("pending model batch lookup failed", error.as_ref())
            })?
            .map(|batch| batch.id),
        None => None,
    };
    if pending_batch_id.is_some() {
        role.reason_code = Some("model_batch_pending".to_owned());
    }
    if execution_mode == ModelTaggingExecutionModeWire::Batch && !batch_available {
        role.reason_code = Some("batch_unsupported_adapter".to_owned());
    }
    let deferred_tracks = inputs.len().saturating_sub(limits.max_tracks);
    inputs.truncate(limits.max_tracks);
    let run_tracks = inputs.len();
    let mut token_reservation = 0;
    let estimated_provider_requests = if let Some(execution) = role.execution.as_ref() {
        match music_application::assistant::plan_tagger_engine_batches(
            &inputs,
            &vocabulary,
            &execution.execution.adapter_id,
            |request| {
                crate::provider_handlers::validate_structured_request(&execution.execution, request)
            },
        ) {
            Ok(batches) => {
                let native = execution.execution.adapter_id
                    == music_application::assistant::TYPESAFE_ADAPTER;
                let request_count = batches
                    .iter()
                    .map(|batch| {
                        batch
                            .native_task
                            .as_ref()
                            .map_or(1, |task| task.max_requests)
                    })
                    .sum::<usize>();
                for task in batches
                    .iter()
                    .filter_map(|batch| batch.native_task.as_ref())
                {
                    for request in task.assessment_requests() {
                        if let Err(error) = crate::provider_transport::validate_typesafe_request(
                            &execution.execution,
                            request,
                        ) {
                            role.reason_code = Some(error.code);
                        }
                    }
                }
                let reservations = batches
                    .iter()
                    .map(|batch| {
                        if let Some(task) = &batch.native_task {
                            return task.token_reservation;
                        }
                        music_application::assistant::model_request_reservation(
                            &batch.task.request(true),
                            execution.execution.max_output_tokens,
                        )
                    })
                    .collect::<Vec<_>>();
                let retries = usize::from(
                    if native || execution_mode == ModelTaggingExecutionModeWire::Batch {
                        0
                    } else {
                        MODEL_TAGGER_INVALID_RESPONSE_RETRY_LIMIT
                    },
                )
                .min(limits.max_requests.saturating_sub(request_count));
                token_reservation = reservations.iter().sum::<u64>()
                    + reservations.iter().max().copied().unwrap_or(0) * retries as u64;
                if execution_mode == ModelTaggingExecutionModeWire::Batch
                    && !native
                    && !batches.is_empty()
                {
                    use music_application::assistant::ModelBatchTransport;
                    let requests = batches
                        .iter()
                        .map(|batch| batch.task.request(false))
                        .collect::<Vec<_>>();
                    if let Some(providers) = &state.providers
                        && let Err(error) = providers
                            .network_boundary()
                            .validate(&execution.execution, &requests)
                    {
                        role.reason_code = Some(error.code);
                    }
                }
                if request_count > limits.max_requests
                    || token_reservation > limits.max_token_reservation
                {
                    role.reason_code = Some("tagging_budget_too_small".to_owned());
                }
                if execution_mode == ModelTaggingExecutionModeWire::Batch
                    && limits.stop_on_empty_batch
                    && batches.len() > 1
                {
                    role.reason_code = Some("batch_pilot_required".to_owned());
                }
                request_count
            }
            Err(error) => {
                role.reason_code = Some(error.code);
                0
            }
        }
    } else {
        tracks_needing_tags.div_ceil(MODEL_TAG_BATCH_SIZE)
    };
    Ok(ModelTaggingAvailabilityResponse {
        execution_mode,
        batch_available,
        pending_batch_id,
        limits,
        run_tracks,
        deferred_tracks,
        token_reservation,
        available: role.reason_code.is_none(),
        reason_code: role.reason_code,
        role_id: "music_tagger",
        connection_name: role.connection_name,
        model_id: role.model_id,
        quality_evaluation_id: "music-tagging-quality-v1",
        job_kind: MODEL_TAGGING_JOB_KIND,
        library_tracks,
        scope_tracks: scoped.len(),
        planned_tracks: planned.len(),
        tracks_with_full_context,
        tracks_with_partial_context,
        tracks_missing_context: scoped.len().saturating_sub(contexts.len()),
        current_profiles,
        tracks_needing_tags,
        estimated_provider_requests,
        disclosure: model_tagging_disclosure(
            &vocabulary,
            role.execution.as_ref().is_some_and(|execution| {
                execution.execution.adapter_id == music_application::assistant::TYPESAFE_ADAPTER
            }),
        ),
    })
}

pub(super) fn model_tagging_disclosure(
    vocabulary: &TagVocabularySnapshot,
    native: bool,
) -> ModelTaggingDisclosureResponse {
    ModelTaggingDisclosureResponse {
        version: "assistant-model-music-tagging-disclosure/v16",
        shared_with_provider: vec![
            "Indexed artist, album, origin, and genre metadata",
            "Track durations and BPM values when available",
            "Current bounded local track context when available: relative signal level, loudness, rhythmic drive, brightness, density and spectral-change trajectories; major acoustic sections and transitions; structural repetition; decoded duration/scope and per-measurement reliability; and optional local voice/instrumental classifier score and coverage (or explicit unknown/unavailable status). The coarse local tempo estimate is withheld.",
            "Current enabled catalog observations: MusicBrainz recording genres, composer credits and first-release date; up to twelve original Last.fm community labels/counts, explicitly weak claims; source, recording ID and retrieval time accompany the claims",
            "For text engines, a batch-local numeric slot used only to match the response; Jev receives one recording per state without a track identifier. Database track IDs are not sent",
            "The full operator-managed tag names, groups, definitions, exact aliases and bounded semantic context cues. Text engines return canonical IDs; Jev answers bounded questions for every tag and selects supporting/conflicting observation IDs. Jev review explanations are assembled by the application and its probabilities are uncalibrated.",
            "In Batch mode, this same evidence is uploaded as a provider file: input expires after seven days and output after up to thirty days; the app deletes known files after collecting results. Completion may take twenty-four hours and completed requests remain chargeable after cancellation.",
        ],
        never_shared: vec![
            "Track titles, display titles, file names, folder names, or library-relative paths",
            "Audio files, waveforms, full-resolution timelines, spectrograms, or cover artwork",
            "The absolute media root or filesystem paths outside the indexed library",
            "Your database mood tags, generated-tag review decisions, or accepted/rejected state",
            "Playlists, review decisions, and provider credentials",
        ],
        allowed_tags: vocabulary
            .document
            .groups
            .iter()
            .flat_map(|group| group.tags.iter().map(|tag| tag.name.clone()))
            .collect(),
        tracks_per_request: if native { 1 } else { MODEL_TAG_BATCH_SIZE },
        invalid_response_retry_limit: if native {
            0
        } else {
            MODEL_TAGGER_INVALID_RESPONSE_RETRY_LIMIT
        },
        may_incur_cost: true,
    }
}

#[utoipa::path(
    post,
    path = "/assistant/library-tags/query",
    operation_id = "query_model_library_tags_api_assistant_library_tags_query_post",
    request_body = ModelTaggingReviewQueryRequest,
    responses(
        (status = 200, description = "Successful Response", body = LibraryTagPageResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant-tags"
)]
pub(super) async fn query_model_library_tags(
    State(state): State<HttpState>,
    _operator: Operator,
    payload: Result<Json<ModelTaggingReviewQueryRequest>, JsonRejection>,
) -> Result<Json<LibraryTagPageResponse>, ApiError> {
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if !(1..=100).contains(&payload.limit) {
        return Err(ApiError::validation());
    }
    let scope = payload
        .scope
        .into_parameters()?
        .to_scope()
        .map_err(|()| ApiError::validation())?;
    let page = service(&state)?
        .tag_page(ManualTagQuery {
            search: String::new(),
            tag: None,
            review: Some(payload.review.into()),
            offset: payload.offset,
            limit: payload.limit,
            analyzer_ids: Some(vec![MODEL_TAG_ANALYZER_ID.to_owned()]),
            scope: Some(scope),
            ..ManualTagQuery::default()
        })
        .await
        .map_err(map_assistant_error)?;
    Ok(Json(LibraryTagPageResponse {
        items: page.items.into_iter().map(track_response).collect(),
        total: page.total,
        offset: page.offset,
        limit: page.limit,
        review_summary: page.review_summary.into(),
    }))
}
