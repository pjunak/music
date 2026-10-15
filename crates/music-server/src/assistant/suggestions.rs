//! Playlist and EQ suggestions, local and model-assisted.

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use music_application::assistant::{PlaylistSuggestion, PlaylistSuggestionRequest};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use crate::auth::Operator;
use crate::error::{ApiError, HttpValidationErrorBody, openapi_integer};
use crate::http::HttpState;
use crate::jobs::BackgroundJobResponse;
use crate::model_jobs::{MODEL_EQ_DRAFT_JOB_KIND, MODEL_PLAYLIST_SUGGESTION_JOB_KIND};
use crate::provider_api::map_model_quality_error;

use super::review::track_ids;
use super::schema::{
    candidate_limit_schema, confidence_schema, energy_curve_default_schema, energy_curve_schema,
    eq_disclosure_version_schema, eq_quality_id_schema, eq_role_id_schema,
    excluded_track_ids_schema, model_playlist_maximum_candidates_schema,
    nonnegative_integer_schema, nonnegative_number_schema, nullable_bpm_schema,
    nullable_integer_schema, nullable_positive_integer_schema, nullable_string_schema,
    playlist_disclosure_version_schema, playlist_quality_id_schema, playlist_role_id_schema,
    target_minutes_schema, true_schema, unit_number_schema,
};
use super::shared::{
    ConfidenceWire, EnergyCurveWire, default_candidate_limit, default_target_minutes, default_true,
    enqueue_model_feature, map_assistant_error, model_role_availability, service,
};

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = PlaylistSuggestionRequest)]
pub(super) struct PlaylistSuggestionRequestWire {
    #[schema(min_length = 2, max_length = 500)]
    pub(super) prompt: String,
    #[serde(default = "default_target_minutes")]
    #[schema(required = false, schema_with = target_minutes_schema)]
    pub(super) target_minutes: u16,
    #[serde(default = "default_candidate_limit")]
    #[schema(required = false, schema_with = candidate_limit_schema)]
    pub(super) candidate_limit: u16,
    #[serde(default)]
    #[schema(required = false, schema_with = nullable_bpm_schema)]
    pub(super) min_bpm: Option<u32>,
    #[serde(default)]
    #[schema(required = false, schema_with = nullable_bpm_schema)]
    pub(super) max_bpm: Option<u32>,
    #[serde(default = "default_true")]
    #[schema(required = false, default = true)]
    pub(super) include_unknown_bpm: bool,
    #[serde(default)]
    #[schema(required = false, schema_with = excluded_track_ids_schema)]
    pub(super) exclude_track_ids: Vec<i64>,
    #[serde(default)]
    #[schema(required = false, schema_with = energy_curve_default_schema)]
    pub(super) energy_curve: EnergyCurveWire,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelPlaylistDisclosure)]
pub(super) struct ModelPlaylistDisclosureResponse {
    #[schema(schema_with = playlist_disclosure_version_schema)]
    pub(super) version: &'static str,
    pub(super) shared_with_provider: Vec<&'static str>,
    pub(super) never_shared: Vec<&'static str>,
    #[schema(schema_with = model_playlist_maximum_candidates_schema)]
    pub(super) maximum_candidates: u16,
    pub(super) may_incur_cost: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelPlaylistAvailability)]
pub(super) struct ModelPlaylistAvailabilityResponse {
    pub(super) available: bool,
    #[schema(required = true, schema_with = nullable_string_schema)]
    pub(super) reason_code: Option<String>,
    #[schema(schema_with = playlist_role_id_schema)]
    pub(super) role_id: &'static str,
    #[schema(required = true, schema_with = nullable_string_schema)]
    pub(super) connection_name: Option<String>,
    #[schema(required = true, schema_with = nullable_string_schema)]
    pub(super) model_id: Option<String>,
    #[schema(schema_with = playlist_quality_id_schema)]
    pub(super) quality_evaluation_id: &'static str,
    pub(super) job_kind: &'static str,
    pub(super) disclosure: ModelPlaylistDisclosureResponse,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelPlaylistSuggestionStartRequest)]
pub(super) struct ModelPlaylistSuggestionStartRequest {
    pub(super) request: PlaylistSuggestionRequestWire,
    #[schema(schema_with = playlist_disclosure_version_schema)]
    pub(super) disclosure_version: String,
    #[schema(schema_with = true_schema)]
    pub(super) consent: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = EqDraftRequest)]
pub(super) struct EqDraftRequestWire {
    #[schema(min_length = 1, max_length = 128)]
    pub(super) name: String,
    #[schema(min_length = 2, max_length = 1000)]
    pub(super) goal: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelEqDisclosure)]
pub(super) struct ModelEqDisclosureResponse {
    #[schema(schema_with = eq_disclosure_version_schema)]
    pub(super) version: &'static str,
    pub(super) shared_with_provider: Vec<&'static str>,
    pub(super) never_shared: Vec<&'static str>,
    pub(super) may_incur_cost: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelEqAvailability)]
pub(super) struct ModelEqAvailabilityResponse {
    pub(super) available: bool,
    #[schema(required = true, schema_with = nullable_string_schema)]
    pub(super) reason_code: Option<String>,
    #[schema(schema_with = eq_role_id_schema)]
    pub(super) role_id: &'static str,
    #[schema(required = true, schema_with = nullable_string_schema)]
    pub(super) connection_name: Option<String>,
    #[schema(required = true, schema_with = nullable_string_schema)]
    pub(super) model_id: Option<String>,
    #[schema(schema_with = eq_quality_id_schema)]
    pub(super) quality_evaluation_id: &'static str,
    pub(super) job_kind: &'static str,
    pub(super) disclosure: ModelEqDisclosureResponse,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelEqDraftStartRequest)]
pub(super) struct ModelEqDraftStartRequest {
    pub(super) request: EqDraftRequestWire,
    #[schema(schema_with = eq_disclosure_version_schema)]
    pub(super) disclosure_version: String,
    #[schema(schema_with = true_schema)]
    pub(super) consent: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = PlaylistIntent)]
pub(super) struct PlaylistIntentResponse {
    pub(super) matched_moods: Vec<String>,
    pub(super) search_terms: Vec<String>,
    #[schema(schema_with = unit_number_schema)]
    pub(super) energy: f64,
    #[schema(schema_with = unit_number_schema)]
    pub(super) brightness: f64,
    #[schema(schema_with = unit_number_schema)]
    pub(super) tension: f64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = PlaylistPlan)]
pub(super) struct PlaylistPlanResponse {
    #[schema(schema_with = energy_curve_schema)]
    pub(super) energy_curve: EnergyCurveWire,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) selected_tracks: usize,
    #[schema(schema_with = nonnegative_number_schema)]
    pub(super) selected_duration_s: f64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = PlaylistCandidate)]
pub(super) struct PlaylistCandidateResponse {
    #[schema(schema_with = openapi_integer)]
    pub(super) track_id: i64,
    pub(super) path: String,
    pub(super) title: String,
    pub(super) display_title: String,
    pub(super) artist: String,
    pub(super) album: String,
    pub(super) origin: String,
    pub(super) genre: String,
    pub(super) manual_tags: Vec<String>,
    #[schema(schema_with = nonnegative_number_schema)]
    pub(super) length_s: f64,
    #[schema(required = true, schema_with = nullable_integer_schema)]
    pub(super) bpm: Option<u32>,
    #[schema(schema_with = unit_number_schema)]
    pub(super) match_score: f64,
    #[schema(schema_with = confidence_schema)]
    pub(super) confidence: ConfidenceWire,
    pub(super) reasons: Vec<String>,
    pub(super) default_selected: bool,
    #[schema(schema_with = nullable_positive_integer_schema)]
    pub(super) sequence_position: Option<usize>,
    #[schema(schema_with = unit_number_schema)]
    pub(super) planning_energy: f64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = PlaylistSuggestionResponse)]
pub(super) struct PlaylistSuggestionResponse {
    #[schema(min_length = 1, max_length = 128)]
    pub(super) engine: String,
    #[schema(schema_with = openapi_integer)]
    pub(super) library_tracks: usize,
    #[schema(schema_with = openapi_integer)]
    pub(super) eligible_tracks: usize,
    pub(super) intent: PlaylistIntentResponse,
    pub(super) plan: PlaylistPlanResponse,
    pub(super) candidates: Vec<PlaylistCandidateResponse>,
}

#[utoipa::path(
    post,
    path = "/assistant/playlists/suggest",
    request_body = PlaylistSuggestionRequestWire,
    responses(
        (status = 200, description = "Successful Response", body = PlaylistSuggestionResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
pub(super) async fn suggest_playlist(
    State(state): State<HttpState>,
    _operator: Operator,
    payload: Result<Json<PlaylistSuggestionRequestWire>, JsonRejection>,
) -> Result<Json<PlaylistSuggestionResponse>, ApiError> {
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let request = playlist_request(payload)?;
    let suggestion = service(&state)?
        .suggest_playlist(&request)
        .await
        .map_err(map_assistant_error)?;
    Ok(Json(playlist_response(suggestion)))
}

#[utoipa::path(
    get,
    path = "/assistant/playlists/model-status",
    responses((status = 200, description = "Successful Response", body = ModelPlaylistAvailabilityResponse)),
    tag = "assistant"
)]
pub(super) async fn model_playlist_status(
    State(state): State<HttpState>,
    _operator: Operator,
) -> Result<Json<ModelPlaylistAvailabilityResponse>, ApiError> {
    let role = model_role_availability(&state, "playlist_planner", "playlist-quality-v1").await?;
    Ok(Json(ModelPlaylistAvailabilityResponse {
        available: role.reason_code.is_none(),
        reason_code: role.reason_code,
        role_id: "playlist_planner",
        connection_name: role.connection_name,
        model_id: role.model_id,
        quality_evaluation_id: "playlist-quality-v1",
        job_kind: MODEL_PLAYLIST_SUGGESTION_JOB_KIND,
        disclosure: playlist_disclosure(),
    }))
}

#[utoipa::path(
    post,
    path = "/assistant/playlists/model-suggestions/jobs",
    request_body = ModelPlaylistSuggestionStartRequest,
    responses(
        (status = 202, description = "Successful Response", body = BackgroundJobResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
pub(super) async fn start_model_playlist_suggestion(
    State(state): State<HttpState>,
    _operator: Operator,
    payload: Result<Json<ModelPlaylistSuggestionStartRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<BackgroundJobResponse>), ApiError> {
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if payload.disclosure_version != "assistant-playlist-model-disclosure/v4" || !payload.consent {
        return Err(ApiError::validation());
    }
    let request_value =
        serde_json::to_value(&payload.request).map_err(|_| ApiError::validation())?;
    let request = playlist_request(payload.request)?;
    request.validate().map_err(|_| ApiError::validation())?;
    let providers = state
        .providers
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)?;
    let role = providers
        .quality_service()
        .prepare_quality_gated_role_execution("playlist_planner", "playlist-quality-v1")
        .await
        .map_err(map_model_quality_error)?;
    let parameters = json!({
        "role_id": "playlist_planner",
        "quality_evaluation_id": "playlist-quality-v1",
        "disclosure_version": payload.disclosure_version,
        "consent": true,
        "role_fingerprint": role.fingerprint,
        "request": request_value,
    });
    enqueue_model_feature(
        &state,
        MODEL_PLAYLIST_SUGGESTION_JOB_KIND,
        parameters,
        "model_suggestion_in_progress",
        "Another model playlist suggestion is already running. Wait for it to finish or cancel it first.",
    )
    .await
}

#[utoipa::path(
    get,
    path = "/assistant/eq/model-status",
    responses((status = 200, description = "Successful Response", body = ModelEqAvailabilityResponse)),
    tag = "assistant"
)]
pub(super) async fn model_eq_status(
    State(state): State<HttpState>,
    _operator: Operator,
) -> Result<Json<ModelEqAvailabilityResponse>, ApiError> {
    let role = model_role_availability(&state, "eq_assistant", "eq-quality-v1").await?;
    Ok(Json(ModelEqAvailabilityResponse {
        available: role.reason_code.is_none(),
        reason_code: role.reason_code,
        role_id: "eq_assistant",
        connection_name: role.connection_name,
        model_id: role.model_id,
        quality_evaluation_id: "eq-quality-v1",
        job_kind: MODEL_EQ_DRAFT_JOB_KIND,
        disclosure: eq_disclosure(),
    }))
}

#[utoipa::path(
    post,
    path = "/assistant/eq/drafts/jobs",
    request_body = ModelEqDraftStartRequest,
    responses(
        (status = 202, description = "Successful Response", body = BackgroundJobResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
pub(super) async fn start_model_eq_draft(
    State(state): State<HttpState>,
    _operator: Operator,
    payload: Result<Json<ModelEqDraftStartRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<BackgroundJobResponse>), ApiError> {
    let Json(mut payload) = payload.map_err(|_| ApiError::validation())?;
    payload.request.name = payload.request.name.trim().to_owned();
    payload.request.goal = payload.request.goal.trim().to_owned();
    if payload.disclosure_version != "assistant-eq-draft-disclosure/v2"
        || !payload.consent
        || payload.request.name.is_empty()
        || payload.request.name.chars().count() > 128
        || !(2..=1_000).contains(&payload.request.goal.chars().count())
    {
        return Err(ApiError::validation());
    }
    let providers = state
        .providers
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)?;
    let role = providers
        .quality_service()
        .prepare_quality_gated_role_execution("eq_assistant", "eq-quality-v1")
        .await
        .map_err(map_model_quality_error)?;
    let parameters = json!({
        "role_id": "eq_assistant",
        "quality_evaluation_id": "eq-quality-v1",
        "disclosure_version": payload.disclosure_version,
        "consent": true,
        "role_fingerprint": role.fingerprint,
        "request": {"name": payload.request.name, "goal": payload.request.goal},
    });
    enqueue_model_feature(
        &state,
        MODEL_EQ_DRAFT_JOB_KIND,
        parameters,
        "eq_draft_in_progress",
        "Another EQ draft is already running. Wait for it to finish or cancel it first.",
    )
    .await
}

pub(super) fn playlist_disclosure() -> ModelPlaylistDisclosureResponse {
    ModelPlaylistDisclosureResponse {
        version: "assistant-playlist-model-disclosure/v4",
        shared_with_provider: vec![
            "Your mood prompt, duration, tempo filters, and requested energy flow",
            "Up to 100 locally prefiltered candidate IDs and descriptive metadata",
            "Candidate titles, artists, albums, origins, genres, durations, and BPM values",
            "Your database mood tags and local search ranking hints (not audio measurements)",
            "Vocabulary names and definitions linking matched prompt aliases or context cues to candidate database mood tags",
            "The deterministic local ranking, default selection, sequence, and duration plan",
        ],
        never_shared: vec![
            "Audio files or cover artwork",
            "Filesystem or library-relative paths",
            "Songs removed by local eligibility, exclusion, and candidate-limit checks",
            "Provider credentials",
        ],
        maximum_candidates: 100,
        may_incur_cost: true,
    }
}

pub(super) fn eq_disclosure() -> ModelEqDisclosureResponse {
    ModelEqDisclosureResponse {
        version: "assistant-eq-draft-disclosure/v2",
        shared_with_provider: vec![
            "The preset goal you type",
            "The fixed ten EQ band frequencies and supported gain limits",
            "A deterministic local baseline and per-band safety envelope derived from the goal",
        ],
        never_shared: vec![
            "Songs, audio, waveforms, or library metadata",
            "Filesystem paths, playlists, existing presets, or provider credentials",
        ],
        may_incur_cost: true,
    }
}

pub(super) fn playlist_request(
    payload: PlaylistSuggestionRequestWire,
) -> Result<PlaylistSuggestionRequest, ApiError> {
    Ok(PlaylistSuggestionRequest {
        prompt: payload.prompt.trim().to_owned(),
        target_minutes: payload.target_minutes,
        candidate_limit: payload.candidate_limit,
        min_bpm: payload.min_bpm,
        max_bpm: payload.max_bpm,
        include_unknown_bpm: payload.include_unknown_bpm,
        exclude_track_ids: track_ids(&payload.exclude_track_ids)?,
        energy_curve: payload.energy_curve.into(),
    })
}

pub(super) fn playlist_response(value: PlaylistSuggestion) -> PlaylistSuggestionResponse {
    PlaylistSuggestionResponse {
        engine: value.engine,
        library_tracks: value.library_tracks,
        eligible_tracks: value.eligible_tracks,
        intent: PlaylistIntentResponse {
            matched_moods: value.intent.matched_moods,
            search_terms: value.intent.search_terms,
            energy: value.intent.energy,
            brightness: value.intent.brightness,
            tension: value.intent.tension,
        },
        plan: PlaylistPlanResponse {
            energy_curve: value.plan.energy_curve.into(),
            selected_tracks: value.plan.selected_tracks,
            selected_duration_s: value.plan.selected_duration_s,
        },
        candidates: value
            .candidates
            .into_iter()
            .map(|item| PlaylistCandidateResponse {
                track_id: item.track_id.get(),
                path: item.path,
                title: item.title,
                display_title: item.display_title,
                artist: item.artist,
                album: item.album,
                origin: item.origin,
                genre: item.genre,
                manual_tags: item.manual_tags,
                length_s: item.length_s,
                bpm: item.bpm,
                match_score: item.match_score,
                confidence: item.confidence.into(),
                reasons: item.reasons,
                default_selected: item.default_selected,
                sequence_position: item.sequence_position,
                planning_energy: item.planning_energy,
            })
            .collect(),
    }
}
