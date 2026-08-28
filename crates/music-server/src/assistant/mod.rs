use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use music_application::assistant::{
    AUDIO_ANALYSIS_JOB_KIND, AnalysisReviewDecision, AnalysisReviewTarget, AssistantService,
    AssistantServiceError, AssistantTrackView, CleanupSelection, Confidence, EnergyCurve,
    LibraryAnalysisSummary, LocalAnalysisError, LocalAnalysisService, METADATA_ANALYSIS_JOB_KIND,
    ManualTagQuery, PlaylistSuggestion, PlaylistSuggestionRequest, TagVocabularyDocument,
    TagVocabularyEntry, TagVocabularyGroup, TagVocabularySnapshot,
};
use music_application::auth::{SessionTouch, UnixSeconds};
use music_domain::TrackId;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{
    AdditionalProperties, AnyOfBuilder, ArrayBuilder, ObjectBuilder, Schema, Type,
};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::{current_session, format_rfc3339};
use crate::error::{
    ApiError, HttpValidationErrorBody, openapi_integer, openapi_nullable_datetime, openapi_number,
};
use crate::http::HttpState;
use crate::jobs::{BackgroundJobResponse, job_response, map_job_error};

const TAG_VOCABULARY_SCHEMA: &str = "assistant-tag-vocabulary/v1";
const TAG_CLEANUP_PREVIEW_SCHEMA: &str = "assistant-tag-cleanup-preview/v2";
const TAG_CLEANUP_APPLY_SCHEMA: &str = "assistant-tag-cleanup-apply/v1";

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum ConfidenceWire {
    High,
    Medium,
    Low,
}

impl From<Confidence> for ConfidenceWire {
    fn from(value: Confidence) -> Self {
        match value {
            Confidence::High => Self::High,
            Confidence::Medium => Self::Medium,
            Confidence::Low => Self::Low,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum ReviewStatusWire {
    Pending,
    Accepted,
    Rejected,
}

impl From<ReviewStatusWire> for AnalysisReviewDecision {
    fn from(value: ReviewStatusWire) -> Self {
        match value {
            ReviewStatusWire::Pending => Self::Pending,
            ReviewStatusWire::Accepted => Self::Accepted,
            ReviewStatusWire::Rejected => Self::Rejected,
        }
    }
}

impl From<AnalysisReviewDecision> for ReviewStatusWire {
    fn from(value: AnalysisReviewDecision) -> Self {
        match value {
            AnalysisReviewDecision::Pending => Self::Pending,
            AnalysisReviewDecision::Accepted => Self::Accepted,
            AnalysisReviewDecision::Rejected => Self::Rejected,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum EnergyCurveWire {
    #[default]
    Steady,
    Rising,
    Falling,
    Arc,
}

impl From<EnergyCurveWire> for EnergyCurve {
    fn from(value: EnergyCurveWire) -> Self {
        match value {
            EnergyCurveWire::Steady => Self::Steady,
            EnergyCurveWire::Rising => Self::Rising,
            EnergyCurveWire::Falling => Self::Falling,
            EnergyCurveWire::Arc => Self::Arc,
        }
    }
}

impl From<EnergyCurve> for EnergyCurveWire {
    fn from(value: EnergyCurve) -> Self {
        match value {
            EnergyCurve::Steady => Self::Steady,
            EnergyCurve::Rising => Self::Rising,
            EnergyCurve::Falling => Self::Falling,
            EnergyCurve::Arc => Self::Arc,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = PlaylistSuggestionRequest)]
struct PlaylistSuggestionRequestWire {
    #[schema(min_length = 2, max_length = 500)]
    prompt: String,
    #[serde(default = "default_target_minutes")]
    #[schema(required = false, schema_with = target_minutes_schema)]
    target_minutes: u16,
    #[serde(default = "default_candidate_limit")]
    #[schema(required = false, schema_with = candidate_limit_schema)]
    candidate_limit: u16,
    #[serde(default)]
    #[schema(required = false, schema_with = nullable_bpm_schema)]
    min_bpm: Option<u32>,
    #[serde(default)]
    #[schema(required = false, schema_with = nullable_bpm_schema)]
    max_bpm: Option<u32>,
    #[serde(default = "default_true")]
    #[schema(required = false, default = true)]
    include_unknown_bpm: bool,
    #[serde(default)]
    #[schema(required = false, schema_with = excluded_track_ids_schema)]
    exclude_track_ids: Vec<i64>,
    #[serde(default)]
    #[schema(required = false, schema_with = energy_curve_default_schema)]
    energy_curve: EnergyCurveWire,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = PlaylistIntent)]
struct PlaylistIntentResponse {
    matched_moods: Vec<String>,
    search_terms: Vec<String>,
    #[schema(schema_with = unit_number_schema)]
    energy: f64,
    #[schema(schema_with = unit_number_schema)]
    brightness: f64,
    #[schema(schema_with = unit_number_schema)]
    tension: f64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = PlaylistAudioSignal)]
pub(crate) struct PlaylistAudioSignalResponse {
    analyzer_id: String,
    #[schema(schema_with = unit_number_schema)]
    energy: f64,
    #[schema(schema_with = unit_number_schema)]
    brightness: f64,
    #[schema(schema_with = unit_number_schema)]
    tension: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(required = false, schema_with = nullable_positive_bpm_number_schema)]
    tempo_bpm: Option<f64>,
    #[schema(schema_with = confidence_schema)]
    confidence: ConfidenceWire,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = PlaylistPlan)]
struct PlaylistPlanResponse {
    #[schema(schema_with = energy_curve_schema)]
    energy_curve: EnergyCurveWire,
    #[schema(schema_with = nonnegative_integer_schema)]
    selected_tracks: usize,
    #[schema(schema_with = nonnegative_number_schema)]
    selected_duration_s: f64,
    #[schema(schema_with = nonnegative_integer_schema)]
    audio_profile_tracks: usize,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = PlaylistCandidate)]
struct PlaylistCandidateResponse {
    #[schema(schema_with = openapi_integer)]
    track_id: i64,
    path: String,
    title: String,
    display_title: String,
    artist: String,
    album: String,
    origin: String,
    genre: String,
    manual_tags: Vec<String>,
    analysis_tags: Vec<String>,
    #[schema(schema_with = nonnegative_number_schema)]
    length_s: f64,
    #[schema(required = true, schema_with = nullable_integer_schema)]
    bpm: Option<u32>,
    #[schema(schema_with = unit_number_schema)]
    match_score: f64,
    #[schema(schema_with = confidence_schema)]
    confidence: ConfidenceWire,
    reasons: Vec<String>,
    default_selected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(required = false, schema_with = nullable_positive_integer_schema)]
    sequence_position: Option<usize>,
    #[schema(schema_with = unit_number_schema)]
    planning_energy: f64,
    #[schema(required = true, schema_with = nullable_playlist_audio_signal_schema)]
    audio_signal: Option<PlaylistAudioSignalResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = PlaylistSuggestionResponse)]
struct PlaylistSuggestionResponse {
    #[schema(min_length = 1, max_length = 128)]
    engine: String,
    #[schema(schema_with = openapi_integer)]
    library_tracks: usize,
    #[schema(schema_with = openapi_integer)]
    eligible_tracks: usize,
    intent: PlaylistIntentResponse,
    plan: PlaylistPlanResponse,
    candidates: Vec<PlaylistCandidateResponse>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagVocabularyEntry)]
struct TagVocabularyEntryWire {
    #[schema(pattern = "^[a-z0-9][a-z0-9._-]{1,63}$")]
    id: String,
    #[schema(min_length = 1, max_length = 64)]
    name: String,
    #[schema(min_length = 2, max_length = 300)]
    description: String,
    #[serde(default)]
    #[schema(required = false, max_items = 24)]
    aliases: Vec<String>,
    #[serde(default)]
    #[schema(required = false, max_items = 32)]
    context_cues: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagVocabularyGroup)]
struct TagVocabularyGroupWire {
    #[schema(pattern = "^[a-z0-9][a-z0-9_-]{1,31}$")]
    key: String,
    #[schema(min_length = 1, max_length = 64)]
    label: String,
    #[serde(default)]
    #[schema(required = false, max_length = 300, default = "")]
    description: String,
    #[serde(default)]
    #[schema(required = false, max_items = 100)]
    tags: Vec<TagVocabularyEntryWire>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagVocabularyUpdateRequest)]
struct TagVocabularyUpdateRequest {
    #[schema(schema_with = vocabulary_version_schema)]
    schema_version: String,
    #[schema(schema_with = positive_integer_schema)]
    expected_revision: u32,
    #[schema(min_items = 1, max_items = 20)]
    groups: Vec<TagVocabularyGroupWire>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagVocabularyOut)]
struct TagVocabularyResponse {
    #[schema(schema_with = vocabulary_version_schema)]
    schema_version: &'static str,
    #[schema(schema_with = positive_integer_schema)]
    revision: u32,
    #[schema(pattern = "^[a-f0-9]{64}$")]
    fingerprint: String,
    groups: Vec<TagVocabularyGroupWire>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = StarterTagGroupOut)]
struct StarterTagGroupResponse {
    key: String,
    label: String,
    tags: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ManualTagUsage)]
struct ManualTagUsageResponse {
    tag: String,
    #[schema(schema_with = positive_integer_schema)]
    track_count: u64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ManualTagCatalog)]
struct ManualTagCatalogResponse {
    starter_groups: Vec<StarterTagGroupResponse>,
    used_tags: Vec<String>,
    tag_usage: Vec<ManualTagUsageResponse>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ManualTagPatch)]
struct ManualTagPatchRequest {
    #[serde(default)]
    #[schema(required = false)]
    add: Vec<String>,
    #[serde(default)]
    #[schema(required = false)]
    remove: Vec<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = BulkManualTagPatch)]
struct BulkManualTagPatchRequest {
    #[serde(default)]
    #[schema(required = false)]
    add: Vec<String>,
    #[serde(default)]
    #[schema(required = false)]
    remove: Vec<String>,
    #[schema(schema_with = required_track_ids_schema)]
    track_ids: Vec<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = BulkManualTagFailure)]
struct BulkManualTagFailureResponse {
    #[schema(schema_with = openapi_integer)]
    track_id: i64,
    error: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = BulkManualTagResult)]
struct BulkManualTagResultResponse {
    #[schema(schema_with = nonnegative_integer_schema)]
    requested_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    matched_tracks: usize,
    #[schema(schema_with = integer_array_schema)]
    changed_track_ids: Vec<i64>,
    #[schema(schema_with = integer_array_schema)]
    missing_track_ids: Vec<i64>,
    failures: Vec<BulkManualTagFailureResponse>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ManualTagRenameRequest)]
struct ManualTagRenameRequest {
    source: String,
    target: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ManualTagRenameResult)]
struct ManualTagRenameResponse {
    source: String,
    target: String,
    #[schema(schema_with = positive_integer_schema)]
    affected_tracks: usize,
    merged: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagCleanupSuggestionOut)]
struct TagCleanupSuggestionResponse {
    #[schema(pattern = "^[a-f0-9]{64}$")]
    id: String,
    source: String,
    target: String,
    #[schema(schema_with = cleanup_reason_schema)]
    reason_code: String,
    reason: String,
    #[schema(schema_with = positive_integer_schema)]
    source_track_count: u64,
    #[schema(schema_with = nonnegative_integer_schema)]
    target_track_count: u64,
    merged: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagCleanupPreviewOut)]
struct TagCleanupPreviewResponse {
    #[schema(schema_with = cleanup_preview_version_schema)]
    schema_version: &'static str,
    #[schema(pattern = "^[a-f0-9]{64}$")]
    catalog_signature: String,
    #[schema(pattern = "^[a-f0-9]{64}$")]
    vocabulary_fingerprint: String,
    suggestions: Vec<TagCleanupSuggestionResponse>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagCleanupSelectionIn)]
struct TagCleanupSelectionRequest {
    source: String,
    target: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagCleanupApplyRequest)]
struct TagCleanupApplyRequest {
    #[schema(pattern = "^[a-f0-9]{64}$")]
    catalog_signature: String,
    #[schema(pattern = "^[a-f0-9]{64}$")]
    vocabulary_fingerprint: String,
    #[schema(min_items = 1, max_items = 100)]
    items: Vec<TagCleanupSelectionRequest>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagCleanupApplyResult)]
struct TagCleanupApplyResponse {
    #[schema(schema_with = cleanup_apply_version_schema)]
    schema_version: &'static str,
    #[schema(schema_with = positive_integer_schema)]
    requested_items: usize,
    applied: Vec<ManualTagRenameResponse>,
    #[schema(pattern = "^[a-f0-9]{64}$")]
    catalog_signature: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = AnalysisTagSuggestionOut)]
struct AnalysisTagSuggestionResponse {
    tag: String,
    analyzer_id: String,
    source_signature: String,
    #[schema(schema_with = confidence_schema)]
    confidence: ConfidenceWire,
    evidence: Vec<String>,
    #[schema(schema_with = review_status_schema)]
    status: ReviewStatusWire,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = AudioSignalProfileOut)]
pub(crate) struct AudioSignalProfileResponse {
    analyzer_id: String,
    #[schema(schema_with = confidence_schema)]
    confidence: ConfidenceWire,
    evidence: Vec<String>,
    #[schema(schema_with = audio_metrics_schema)]
    metrics: Map<String, Value>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = LibraryTagTrack)]
struct LibraryTagTrackResponse {
    #[schema(schema_with = openapi_integer)]
    track_id: i64,
    path: String,
    title: String,
    display_title: String,
    artist: String,
    album: String,
    manual_tags: Vec<String>,
    #[schema(required = true, schema_with = nullable_string_schema)]
    analysis_analyzer: Option<String>,
    analysis_tags: Vec<String>,
    #[schema(required = true, schema_with = nullable_confidence_schema)]
    analysis_confidence: Option<ConfidenceWire>,
    analysis_suggestions: Vec<AnalysisTagSuggestionResponse>,
    #[schema(required = true, schema_with = nullable_audio_profile_schema)]
    audio_signal: Option<AudioSignalProfileResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = LibraryTagPage)]
struct LibraryTagPageResponse {
    items: Vec<LibraryTagTrackResponse>,
    #[schema(schema_with = nonnegative_integer_schema)]
    total: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    offset: usize,
    #[schema(schema_with = positive_integer_schema)]
    limit: usize,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct LibraryTagListQuery {
    #[serde(default)]
    #[param(max_length = 128, default = "")]
    search: String,
    #[param(max_length = 64, schema_with = nullable_tag_schema)]
    tag: Option<String>,
    #[param(schema_with = nullable_review_status_schema)]
    review: Option<ReviewStatusWire>,
    #[serde(default)]
    #[param(minimum = 0, default = 0)]
    offset: usize,
    #[serde(default = "default_page_limit")]
    #[param(minimum = 1, maximum = 100, default = 50)]
    limit: usize,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = AnalysisTagReviewRequest)]
struct AnalysisTagReviewRequest {
    tag: String,
    #[schema(min_length = 1, max_length = 128)]
    analyzer_id: String,
    #[schema(min_length = 1, max_length = 128)]
    source_signature: String,
    #[schema(schema_with = review_status_schema)]
    decision: ReviewStatusWire,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = AnalysisTagReviewResult)]
struct AnalysisTagReviewResponse {
    #[schema(schema_with = openapi_integer)]
    track_id: i64,
    tag: String,
    analyzer_id: String,
    source_signature: String,
    #[schema(schema_with = review_status_schema)]
    decision: ReviewStatusWire,
    manual_tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = BulkAnalysisTagReviewItem)]
struct BulkAnalysisTagReviewItemRequest {
    tag: String,
    #[schema(min_length = 1, max_length = 128)]
    analyzer_id: String,
    #[schema(min_length = 1, max_length = 128)]
    source_signature: String,
    #[schema(schema_with = positive_exclusive_integer_schema)]
    track_id: i64,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum BulkReviewDecisionWire {
    Accepted,
    Rejected,
}

impl From<BulkReviewDecisionWire> for AnalysisReviewDecision {
    fn from(value: BulkReviewDecisionWire) -> Self {
        match value {
            BulkReviewDecisionWire::Accepted => Self::Accepted,
            BulkReviewDecisionWire::Rejected => Self::Rejected,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = BulkAnalysisTagReviewRequest)]
struct BulkAnalysisTagReviewRequest {
    #[schema(min_items = 1, max_items = 1000)]
    items: Vec<BulkAnalysisTagReviewItemRequest>,
    #[schema(schema_with = bulk_review_decision_schema)]
    decision: BulkReviewDecisionWire,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = BulkAnalysisTagReviewApplied)]
struct BulkAnalysisTagReviewAppliedResponse {
    #[schema(schema_with = openapi_integer)]
    track_id: i64,
    tag: String,
    analyzer_id: String,
    source_signature: String,
    #[schema(schema_with = bulk_review_decision_schema)]
    decision: BulkReviewDecisionWire,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = BulkAnalysisTagReviewFailure)]
struct BulkAnalysisTagReviewFailureResponse {
    #[schema(schema_with = openapi_integer)]
    track_id: i64,
    tag: String,
    analyzer_id: String,
    source_signature: String,
    #[schema(schema_with = review_failure_code_schema)]
    code: String,
    error: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = BulkAnalysisTagReviewResult)]
struct BulkAnalysisTagReviewResponse {
    #[schema(schema_with = nonnegative_integer_schema)]
    requested_items: usize,
    applied: Vec<BulkAnalysisTagReviewAppliedResponse>,
    failures: Vec<BulkAnalysisTagReviewFailureResponse>,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
#[schema(as = LibraryAnalysisStartRequest)]
struct LibraryAnalysisStartRequest {
    #[schema(required = false, default = false)]
    force: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = LibraryAnalysisSummary)]
struct LibraryAnalysisSummaryResponse {
    analyzer: String,
    #[schema(schema_with = nonnegative_integer_schema)]
    library_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    analyzed_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    failed_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    stale_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    high_confidence: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    medium_confidence: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    low_confidence: usize,
    #[schema(required = true, schema_with = openapi_nullable_datetime)]
    last_updated_at: Option<String>,
}

pub(crate) fn assistant_router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default()
        .routes(routes!(start_library_analysis))
        .routes(routes!(library_analysis_summary))
        .routes(routes!(start_library_audio_analysis))
        .routes(routes!(library_audio_analysis_summary))
        .routes(routes!(suggest_playlist))
        .routes(routes!(tag_catalog))
        .routes(routes!(get_tag_vocabulary))
        .routes(routes!(update_tag_vocabulary))
        .routes(routes!(preview_tag_cleanup))
        .routes(routes!(apply_tag_cleanup))
        .routes(routes!(rename_tag))
        .routes(routes!(patch_tags_bulk))
        .routes(routes!(review_analysis_tags_bulk))
        .routes(routes!(list_library_tags))
        .routes(routes!(patch_track_tags))
        .routes(routes!(review_analysis_tag))
}

#[utoipa::path(
    post,
    path = "/assistant/library-analysis/jobs",
    operation_id = "start_library_analysis_api_assistant_library_analysis_jobs_post",
    request_body = LibraryAnalysisStartRequest,
    responses(
        (status = 202, description = "Successful Response", body = BackgroundJobResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
async fn start_library_analysis(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<LibraryAnalysisStartRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<BackgroundJobResponse>), ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let (job, _created) = state
        .jobs
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)?
        .enqueue_unique_active(METADATA_ANALYSIS_JOB_KIND, json!({"force": payload.force}))
        .await
        .map_err(map_job_error)?;
    Ok((StatusCode::ACCEPTED, Json(job_response(job)?)))
}

#[utoipa::path(
    get,
    path = "/assistant/library-analysis/summary",
    operation_id = "library_analysis_summary_api_assistant_library_analysis_summary_get",
    responses((status = 200, description = "Successful Response", body = LibraryAnalysisSummaryResponse)),
    tag = "assistant"
)]
async fn library_analysis_summary(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<LibraryAnalysisSummaryResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let summary = analysis_service(&state)?
        .metadata_summary()
        .await
        .map_err(map_local_analysis_error)?;
    Ok(Json(library_analysis_summary_response(summary)?))
}

#[utoipa::path(
    post,
    path = "/assistant/library-audio-analysis/jobs",
    operation_id = "start_library_audio_analysis_api_assistant_library_audio_analysis_jobs_post",
    request_body = LibraryAnalysisStartRequest,
    responses(
        (status = 202, description = "Successful Response", body = BackgroundJobResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
async fn start_library_audio_analysis(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<LibraryAnalysisStartRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<BackgroundJobResponse>), ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let (job, _created) = state
        .jobs
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)?
        .enqueue_unique_active(AUDIO_ANALYSIS_JOB_KIND, json!({"force": payload.force}))
        .await
        .map_err(map_job_error)?;
    Ok((StatusCode::ACCEPTED, Json(job_response(job)?)))
}

#[utoipa::path(
    get,
    path = "/assistant/library-audio-analysis/summary",
    operation_id = "library_audio_analysis_summary_api_assistant_library_audio_analysis_summary_get",
    responses((status = 200, description = "Successful Response", body = LibraryAnalysisSummaryResponse)),
    tag = "assistant"
)]
async fn library_audio_analysis_summary(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<LibraryAnalysisSummaryResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let summary = analysis_service(&state)?
        .audio_summary()
        .await
        .map_err(map_local_analysis_error)?;
    Ok(Json(library_analysis_summary_response(summary)?))
}

#[utoipa::path(
    post,
    path = "/assistant/playlists/suggest",
    operation_id = "suggest_playlist_api_assistant_playlists_suggest_post",
    request_body = PlaylistSuggestionRequestWire,
    responses(
        (status = 200, description = "Successful Response", body = PlaylistSuggestionResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
async fn suggest_playlist(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<PlaylistSuggestionRequestWire>, JsonRejection>,
) -> Result<Json<PlaylistSuggestionResponse>, ApiError> {
    authorize(&state, &headers).await?;
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
    path = "/assistant/library-tags/catalog",
    operation_id = "tag_catalog_api_assistant_library_tags_catalog_get",
    responses((status = 200, description = "Successful Response", body = ManualTagCatalogResponse)),
    tag = "assistant-tags"
)]
async fn tag_catalog(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<ManualTagCatalogResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let service = service(&state)?;
    let usage = service.tag_usage().await.map_err(map_assistant_error)?;
    let vocabulary = service.vocabulary().await.map_err(map_assistant_error)?;
    Ok(Json(ManualTagCatalogResponse {
        starter_groups: vocabulary
            .document
            .groups
            .into_iter()
            .map(|group| StarterTagGroupResponse {
                key: group.key,
                label: group.label,
                tags: group.tags.into_iter().map(|tag| tag.name).collect(),
            })
            .collect(),
        used_tags: usage.iter().map(|item| item.tag.clone()).collect(),
        tag_usage: usage
            .into_iter()
            .map(|item| ManualTagUsageResponse {
                tag: item.tag,
                track_count: item.track_count,
            })
            .collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/assistant/library-tags/vocabulary",
    operation_id = "get_tag_vocabulary_api_assistant_library_tags_vocabulary_get",
    responses((status = 200, description = "Successful Response", body = TagVocabularyResponse)),
    tag = "assistant-tags"
)]
async fn get_tag_vocabulary(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<TagVocabularyResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let snapshot = service(&state)?
        .vocabulary()
        .await
        .map_err(map_assistant_error)?;
    Ok(Json(vocabulary_response(snapshot)))
}

#[utoipa::path(
    put,
    path = "/assistant/library-tags/vocabulary",
    operation_id = "update_tag_vocabulary_api_assistant_library_tags_vocabulary_put",
    request_body = TagVocabularyUpdateRequest,
    responses(
        (status = 200, description = "Successful Response", body = TagVocabularyResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant-tags"
)]
async fn update_tag_vocabulary(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<TagVocabularyUpdateRequest>, JsonRejection>,
) -> Result<Json<TagVocabularyResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let document = TagVocabularyDocument {
        schema_version: payload.schema_version,
        groups: payload.groups.into_iter().map(Into::into).collect(),
    };
    let snapshot = service(&state)?
        .replace_vocabulary(payload.expected_revision, document)
        .await
        .map_err(map_assistant_error)?;
    Ok(Json(vocabulary_response(snapshot)))
}

#[utoipa::path(
    get,
    path = "/assistant/library-tags/catalog/cleanup-preview",
    operation_id = "preview_library_tag_cleanup_api_assistant_library_tags_catalog_cleanup_preview_get",
    responses((status = 200, description = "Successful Response", body = TagCleanupPreviewResponse)),
    tag = "assistant-tags"
)]
async fn preview_tag_cleanup(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<TagCleanupPreviewResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let preview = service(&state)?
        .cleanup_preview()
        .await
        .map_err(map_assistant_error)?;
    Ok(Json(TagCleanupPreviewResponse {
        schema_version: TAG_CLEANUP_PREVIEW_SCHEMA,
        catalog_signature: preview.catalog_signature,
        vocabulary_fingerprint: preview.vocabulary_fingerprint,
        suggestions: preview
            .suggestions
            .into_iter()
            .map(|item| TagCleanupSuggestionResponse {
                id: item.id,
                source: item.source,
                target: item.target,
                reason_code: item.reason_code.as_str().to_owned(),
                reason: item.reason,
                source_track_count: item.source_track_count,
                target_track_count: item.target_track_count,
                merged: item.merged,
            })
            .collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/assistant/library-tags/catalog/cleanup-apply",
    operation_id = "apply_library_tag_cleanup_api_assistant_library_tags_catalog_cleanup_apply_post",
    request_body = TagCleanupApplyRequest,
    responses(
        (status = 200, description = "Successful Response", body = TagCleanupApplyResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant-tags"
)]
async fn apply_tag_cleanup(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<TagCleanupApplyRequest>, JsonRejection>,
) -> Result<Json<TagCleanupApplyResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let requested_items = payload.items.len();
    let outcome = service(&state)?
        .apply_cleanup(
            &payload.catalog_signature,
            &payload.vocabulary_fingerprint,
            &payload
                .items
                .into_iter()
                .map(Into::into)
                .collect::<Vec<_>>(),
        )
        .await
        .map_err(map_assistant_error)?;
    Ok(Json(TagCleanupApplyResponse {
        schema_version: TAG_CLEANUP_APPLY_SCHEMA,
        requested_items,
        applied: outcome.applied.into_iter().map(rename_response).collect(),
        catalog_signature: outcome.catalog_signature,
    }))
}

#[utoipa::path(
    post,
    path = "/assistant/library-tags/catalog/rename",
    operation_id = "rename_library_tag_api_assistant_library_tags_catalog_rename_post",
    request_body = ManualTagRenameRequest,
    responses(
        (status = 200, description = "Successful Response", body = ManualTagRenameResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant-tags"
)]
async fn rename_tag(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<ManualTagRenameRequest>, JsonRejection>,
) -> Result<Json<ManualTagRenameResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let outcome = service(&state)?
        .rename_tag(&payload.source, &payload.target)
        .await
        .map_err(map_assistant_error)?;
    Ok(Json(rename_response(outcome)))
}

#[utoipa::path(
    post,
    path = "/assistant/library-tags/bulk",
    operation_id = "update_library_tags_bulk_api_assistant_library_tags_bulk_post",
    request_body = BulkManualTagPatchRequest,
    responses(
        (status = 200, description = "Successful Response", body = BulkManualTagResultResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant-tags"
)]
async fn patch_tags_bulk(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<BulkManualTagPatchRequest>, JsonRejection>,
) -> Result<Json<BulkManualTagResultResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let track_ids = track_ids(&payload.track_ids)?;
    let outcome = service(&state)?
        .patch_tags(&track_ids, &payload.add, &payload.remove)
        .await
        .map_err(map_assistant_error)?;
    Ok(Json(BulkManualTagResultResponse {
        requested_tracks: outcome.requested_tracks,
        matched_tracks: outcome.matched_tracks,
        changed_track_ids: outcome
            .changed_track_ids
            .into_iter()
            .map(TrackId::get)
            .collect(),
        missing_track_ids: outcome
            .missing_track_ids
            .into_iter()
            .map(TrackId::get)
            .collect(),
        failures: outcome
            .failures
            .into_iter()
            .map(|item| BulkManualTagFailureResponse {
                track_id: item.track_id.get(),
                error: item.error,
            })
            .collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/assistant/library-tags/analysis-tags/reviews/bulk",
    operation_id = "update_analysis_tag_reviews_bulk_api_assistant_library_tags_analysis_tags_reviews_bulk_post",
    request_body = BulkAnalysisTagReviewRequest,
    responses(
        (status = 200, description = "Successful Response", body = BulkAnalysisTagReviewResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant-tags"
)]
async fn review_analysis_tags_bulk(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<BulkAnalysisTagReviewRequest>, JsonRejection>,
) -> Result<Json<BulkAnalysisTagReviewResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let decision: AnalysisReviewDecision = payload.decision.into();
    let targets = payload
        .items
        .into_iter()
        .map(review_target)
        .collect::<Result<Vec<_>, _>>()?;
    let outcome = service(&state)?
        .review_analysis(&targets, decision)
        .await
        .map_err(map_assistant_error)?;
    let response_decision = match decision {
        AnalysisReviewDecision::Accepted => BulkReviewDecisionWire::Accepted,
        AnalysisReviewDecision::Rejected => BulkReviewDecisionWire::Rejected,
        AnalysisReviewDecision::Pending => return Err(ApiError::validation()),
    };
    Ok(Json(BulkAnalysisTagReviewResponse {
        requested_items: outcome.requested_items,
        applied: outcome
            .applied
            .into_iter()
            .map(|item| BulkAnalysisTagReviewAppliedResponse {
                track_id: item.track_id.get(),
                tag: item.tag,
                analyzer_id: item.analyzer_id,
                source_signature: item.source_signature,
                decision: response_decision,
            })
            .collect(),
        failures: outcome
            .failures
            .into_iter()
            .map(|item| BulkAnalysisTagReviewFailureResponse {
                track_id: item.target.track_id.get(),
                tag: item.target.tag,
                analyzer_id: item.target.analyzer_id,
                source_signature: item.target.source_signature,
                code: item.code.as_str().to_owned(),
                error: item.error,
            })
            .collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/assistant/library-tags",
    operation_id = "list_library_tags_api_assistant_library_tags_get",
    params(LibraryTagListQuery),
    responses(
        (status = 200, description = "Successful Response", body = LibraryTagPageResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant-tags"
)]
async fn list_library_tags(
    State(state): State<HttpState>,
    headers: HeaderMap,
    query: Result<Query<LibraryTagListQuery>, QueryRejection>,
) -> Result<Json<LibraryTagPageResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Query(query) = query.map_err(|_| ApiError::validation())?;
    let page = service(&state)?
        .tag_page(ManualTagQuery {
            search: query.search,
            tag: query.tag,
            review: query.review.map(Into::into),
            offset: query.offset,
            limit: query.limit,
            analyzer_ids: None,
        })
        .await
        .map_err(map_assistant_error)?;
    Ok(Json(LibraryTagPageResponse {
        items: page.items.into_iter().map(track_response).collect(),
        total: page.total,
        offset: page.offset,
        limit: page.limit,
    }))
}

#[utoipa::path(
    patch,
    path = "/assistant/library-tags/{track_id}",
    operation_id = "update_library_tags_api_assistant_library_tags__track_id__patch",
    params(("track_id" = i128, Path)),
    request_body = ManualTagPatchRequest,
    responses(
        (status = 200, description = "Successful Response", body = LibraryTagTrackResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant-tags"
)]
async fn patch_track_tags(
    State(state): State<HttpState>,
    headers: HeaderMap,
    track_id: Result<Path<i128>, PathRejection>,
    payload: Result<Json<ManualTagPatchRequest>, JsonRejection>,
) -> Result<Json<LibraryTagTrackResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Path(raw_track_id) = track_id.map_err(|_| ApiError::validation())?;
    let track_id = TrackId::new(i64::try_from(raw_track_id).map_err(|_| ApiError::validation())?)
        .map_err(|_| ApiError::validation())?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let track = service(&state)?
        .patch_track(track_id, &payload.add, &payload.remove)
        .await
        .map_err(map_assistant_error)?;
    Ok(Json(track_response(track)))
}

#[utoipa::path(
    put,
    path = "/assistant/library-tags/{track_id}/analysis-tags/review",
    operation_id = "update_analysis_tag_review_api_assistant_library_tags__track_id__analysis_tags_review_put",
    params(("track_id" = i128, Path)),
    request_body = AnalysisTagReviewRequest,
    responses(
        (status = 200, description = "Successful Response", body = AnalysisTagReviewResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant-tags"
)]
async fn review_analysis_tag(
    State(state): State<HttpState>,
    headers: HeaderMap,
    track_id: Result<Path<i128>, PathRejection>,
    payload: Result<Json<AnalysisTagReviewRequest>, JsonRejection>,
) -> Result<Json<AnalysisTagReviewResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Path(raw_track_id) = track_id.map_err(|_| ApiError::validation())?;
    let track_id = TrackId::new(i64::try_from(raw_track_id).map_err(|_| ApiError::validation())?)
        .map_err(|_| ApiError::validation())?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let decision: AnalysisReviewDecision = payload.decision.into();
    let (outcome, manual_tags) = service(&state)?
        .review_one(
            AnalysisReviewTarget {
                track_id,
                tag: payload.tag,
                analyzer_id: payload.analyzer_id,
                source_signature: payload.source_signature,
            },
            decision,
        )
        .await
        .map_err(map_assistant_error)?;
    Ok(Json(AnalysisTagReviewResponse {
        track_id: outcome.track_id.get(),
        tag: outcome.tag,
        analyzer_id: outcome.analyzer_id,
        source_signature: outcome.source_signature,
        decision: outcome.decision.into(),
        manual_tags,
    }))
}

fn playlist_request(
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

fn playlist_response(value: PlaylistSuggestion) -> PlaylistSuggestionResponse {
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
            audio_profile_tracks: value.plan.audio_profile_tracks,
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
                analysis_tags: item.analysis_tags,
                length_s: item.length_s,
                bpm: item.bpm,
                match_score: item.match_score,
                confidence: item.confidence.into(),
                reasons: item.reasons,
                default_selected: item.default_selected,
                sequence_position: item.sequence_position,
                planning_energy: item.planning_energy,
                audio_signal: item.audio_signal.map(|signal| PlaylistAudioSignalResponse {
                    analyzer_id: signal.analyzer_id,
                    energy: signal.energy,
                    brightness: signal.brightness,
                    tension: signal.tension,
                    tempo_bpm: signal.tempo_bpm,
                    confidence: signal.confidence.into(),
                }),
            })
            .collect(),
    }
}

fn vocabulary_response(snapshot: TagVocabularySnapshot) -> TagVocabularyResponse {
    TagVocabularyResponse {
        schema_version: TAG_VOCABULARY_SCHEMA,
        revision: snapshot.revision,
        fingerprint: snapshot.fingerprint,
        groups: snapshot
            .document
            .groups
            .into_iter()
            .map(Into::into)
            .collect(),
    }
}

impl From<TagVocabularyEntryWire> for TagVocabularyEntry {
    fn from(value: TagVocabularyEntryWire) -> Self {
        Self {
            id: value.id,
            name: value.name,
            description: value.description,
            aliases: value.aliases,
            context_cues: value.context_cues,
        }
    }
}

impl From<TagVocabularyEntry> for TagVocabularyEntryWire {
    fn from(value: TagVocabularyEntry) -> Self {
        Self {
            id: value.id,
            name: value.name,
            description: value.description,
            aliases: value.aliases,
            context_cues: value.context_cues,
        }
    }
}

impl From<TagVocabularyGroupWire> for TagVocabularyGroup {
    fn from(value: TagVocabularyGroupWire) -> Self {
        Self {
            key: value.key,
            label: value.label,
            description: value.description,
            tags: value.tags.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<TagVocabularyGroup> for TagVocabularyGroupWire {
    fn from(value: TagVocabularyGroup) -> Self {
        Self {
            key: value.key,
            label: value.label,
            description: value.description,
            tags: value.tags.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<TagCleanupSelectionRequest> for CleanupSelection {
    fn from(value: TagCleanupSelectionRequest) -> Self {
        Self {
            source: value.source,
            target: value.target,
        }
    }
}

fn rename_response(
    value: music_application::assistant::RenameTagOutcome,
) -> ManualTagRenameResponse {
    ManualTagRenameResponse {
        source: value.source,
        target: value.target,
        affected_tracks: value.affected_tracks,
        merged: value.merged,
    }
}

fn track_response(value: AssistantTrackView) -> LibraryTagTrackResponse {
    LibraryTagTrackResponse {
        track_id: value.track.id.get(),
        path: value.track.path.as_str().to_owned(),
        title: value.track.metadata.title,
        display_title: value.track.display_title,
        artist: value.track.metadata.artist,
        album: value.track.metadata.album,
        manual_tags: value.manual_tags,
        analysis_analyzer: value.analysis_analyzer,
        analysis_tags: value.analysis_tags,
        analysis_confidence: value.analysis_confidence.map(Into::into),
        analysis_suggestions: value
            .analysis_suggestions
            .into_iter()
            .map(|item| AnalysisTagSuggestionResponse {
                tag: item.tag,
                analyzer_id: item.analyzer_id,
                source_signature: item.source_signature,
                confidence: item.confidence.into(),
                evidence: item.evidence,
                status: item.status.into(),
            })
            .collect(),
        audio_signal: value.audio_signal.map(|signal| AudioSignalProfileResponse {
            analyzer_id: signal.analyzer_id,
            confidence: signal.confidence.into(),
            evidence: signal.evidence,
            metrics: signal.metrics,
        }),
    }
}

fn review_target(
    value: BulkAnalysisTagReviewItemRequest,
) -> Result<AnalysisReviewTarget, ApiError> {
    Ok(AnalysisReviewTarget {
        track_id: TrackId::new(value.track_id).map_err(|_| ApiError::validation())?,
        tag: value.tag,
        analyzer_id: value.analyzer_id,
        source_signature: value.source_signature,
    })
}

fn track_ids(values: &[i64]) -> Result<Vec<TrackId>, ApiError> {
    values
        .iter()
        .map(|value| TrackId::new(*value).map_err(|_| ApiError::validation()))
        .collect()
}

fn service(state: &HttpState) -> Result<&AssistantService, ApiError> {
    state
        .assistant
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)
}

fn analysis_service(state: &HttpState) -> Result<&LocalAnalysisService, ApiError> {
    state
        .local_analysis
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)
}

fn library_analysis_summary_response(
    summary: LibraryAnalysisSummary,
) -> Result<LibraryAnalysisSummaryResponse, ApiError> {
    Ok(LibraryAnalysisSummaryResponse {
        analyzer: summary.analyzer,
        library_tracks: summary.library_tracks,
        analyzed_tracks: summary.analyzed_tracks,
        failed_tracks: summary.failed_tracks,
        stale_tracks: summary.stale_tracks,
        high_confidence: summary.high_confidence,
        medium_confidence: summary.medium_confidence,
        low_confidence: summary.low_confidence,
        last_updated_at: summary
            .last_updated_at_unix_seconds
            .map(UnixSeconds::new)
            .map(format_rfc3339)
            .transpose()?,
    })
}

fn map_local_analysis_error(error: LocalAnalysisError) -> ApiError {
    tracing::error!(error = %error, "Local analysis request failed");
    ApiError::internal()
}

async fn authorize(state: &HttpState, headers: &HeaderMap) -> Result<(), ApiError> {
    current_session(state, headers, SessionTouch::UpdateLastSeen)
        .await
        .map(|_| ())
}

fn map_assistant_error(error: AssistantServiceError) -> ApiError {
    match error {
        AssistantServiceError::Validation(_) | AssistantServiceError::Vocabulary(_) => {
            ApiError::validation()
        }
        AssistantServiceError::NotFound(message) => ApiError::not_found_message(message),
        AssistantServiceError::Conflict(message) | AssistantServiceError::Stale(message) => {
            ApiError::conflict_message(message)
        }
        AssistantServiceError::InvalidCleanupSelection(message) => {
            ApiError::unprocessable_message(message)
        }
        AssistantServiceError::Dependency(source) => {
            tracing::error!(error = %source, "Assistant request failed");
            ApiError::internal()
        }
    }
}

const fn default_target_minutes() -> u16 {
    60
}
const fn default_candidate_limit() -> u16 {
    40
}
const fn default_page_limit() -> usize {
    50
}
const fn default_true() -> bool {
    true
}

fn string_schema() -> RefOr<Schema> {
    ObjectBuilder::new().schema_type(Type::String).into()
}

fn nullable_string_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(string_schema())
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_integer_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(openapi_integer())
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_positive_integer_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::Integer)
                    .minimum(Some(1)),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_bpm_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::Integer)
                    .minimum(Some(1))
                    .maximum(Some(999)),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_positive_bpm_number_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::Number)
                    .exclusive_minimum(Some(0))
                    .maximum(Some(999)),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn excluded_track_ids_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(openapi_integer())
        .max_items(Some(5_000))
        .into()
}

fn required_track_ids_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(openapi_integer())
        .min_items(Some(1))
        .max_items(Some(5_000))
        .into()
}

fn integer_array_schema() -> RefOr<Schema> {
    ArrayBuilder::new().items(openapi_integer()).into()
}

fn target_minutes_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(5))
        .maximum(Some(600))
        .default(Some(json!(60)))
        .into()
}

fn candidate_limit_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(5))
        .maximum(Some(100))
        .default(Some(json!(40)))
        .into()
}

fn positive_integer_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(1))
        .into()
}

fn positive_exclusive_integer_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .exclusive_minimum(Some(0))
        .into()
}

fn nonnegative_integer_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(0))
        .into()
}

fn nonnegative_number_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Number)
        .minimum(Some(0))
        .into()
}

fn unit_number_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Number)
        .minimum(Some(0))
        .maximum(Some(1))
        .into()
}

fn confidence_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["high", "medium", "low"]))
        .into()
}

fn nullable_confidence_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(confidence_schema())
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn review_status_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["pending", "accepted", "rejected"]))
        .into()
}

fn nullable_review_status_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(review_status_schema())
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn bulk_review_decision_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["accepted", "rejected"]))
        .into()
}

fn review_failure_code_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["not_found", "stale", "tag_limit"]))
        .into()
}

fn cleanup_reason_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some([
            "vocabulary_alias",
            "vocabulary_plural",
            "vocabulary_typo",
        ]))
        .into()
}

fn energy_curve_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["steady", "rising", "falling", "arc"]))
        .into()
}

fn energy_curve_default_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["steady", "rising", "falling", "arc"]))
        .default(Some(json!("steady")))
        .into()
}

fn nullable_tag_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .max_length(Some(64)),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn vocabulary_version_schema() -> RefOr<Schema> {
    const_string_schema(TAG_VOCABULARY_SCHEMA)
}
fn cleanup_preview_version_schema() -> RefOr<Schema> {
    const_string_schema(TAG_CLEANUP_PREVIEW_SCHEMA)
}
fn cleanup_apply_version_schema() -> RefOr<Schema> {
    const_string_schema(TAG_CLEANUP_APPLY_SCHEMA)
}

fn const_string_schema(value: &'static str) -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .extensions(Some([("const", json!(value))].into_iter().collect()))
        .into()
}

fn nullable_playlist_audio_signal_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(RefOr::Ref(utoipa::openapi::Ref::from_schema_name(
                "PlaylistAudioSignal",
            )))
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_audio_profile_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(RefOr::Ref(utoipa::openapi::Ref::from_schema_name(
                "AudioSignalProfileOut",
            )))
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn audio_metrics_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Object)
        .additional_properties(Some(AdditionalProperties::RefOr(
            Schema::AnyOf(
                AnyOfBuilder::new()
                    .item(string_schema())
                    .item(openapi_integer())
                    .item(openapi_number())
                    .item(ObjectBuilder::new().schema_type(Type::Null))
                    .build(),
            )
            .into(),
        )))
        .into()
}
