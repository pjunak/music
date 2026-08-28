use std::collections::{BTreeMap, BTreeSet};

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use music_application::assistant::{
    AUDIO_ANALYSIS_JOB_KIND, AnalysisReviewDecision, AnalysisReviewTarget, AssistantService,
    AssistantServiceError, AssistantTrackView, CleanupSelection, Confidence, EnergyCurve,
    LIBRARY_CONTEXT_JOB_KIND, LibraryAnalysisSummary, LibraryContextPassSummary,
    LibraryContextSummary, LocalAnalysisError, LocalAnalysisService, MAX_MODEL_CLEANUP_TAGS,
    METADATA_ANALYSIS_JOB_KIND, MODEL_TAG_ANALYZER_ID, MODEL_TAG_BATCH_SIZE,
    MODEL_TAG_CLEANUP_BATCH_SIZE, MODEL_TAG_CLEANUP_ENGINE_ID,
    MODEL_TAGGER_INVALID_RESPONSE_RETRY_LIMIT, ManualTagQuery, ModelTagCleanupTask,
    PlaylistSuggestion, PlaylistSuggestionRequest, TagVocabularyDocument, TagVocabularyEntry,
    TagVocabularyGroup, TagVocabularySnapshot, TrackContextDetail, VoiceAnalyzerStatus,
    catalog_signature, model_tag_source_signature,
};
use music_application::auth::{SessionTouch, UnixSeconds};
use music_application::jobs::JobStatus;
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

use crate::analysis::{ContextAnalysisParameters, ContextScopeKind, ContextScopeParameters};
use crate::auth::{current_session, format_rfc3339};
use crate::error::{
    ApiError, HttpValidationErrorBody, openapi_integer, openapi_nullable_datetime, openapi_number,
};
use crate::http::HttpState;
use crate::jobs::{BackgroundJobResponse, job_response, map_job_error};
use crate::model_jobs::{
    MODEL_EQ_DRAFT_JOB_KIND, MODEL_PLAYLIST_SUGGESTION_JOB_KIND, MODEL_TAG_CLEANUP_JOB_KIND,
    MODEL_TAGGING_JOB_KIND,
};
use crate::provider_api::{map_model_quality_error, map_provider_error};

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

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum ReviewStatusWire {
    #[default]
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

#[derive(Debug, Deserialize, Serialize, ToSchema)]
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
#[schema(as = ModelPlaylistDisclosure)]
struct ModelPlaylistDisclosureResponse {
    #[schema(schema_with = playlist_disclosure_version_schema)]
    version: &'static str,
    shared_with_provider: Vec<&'static str>,
    never_shared: Vec<&'static str>,
    #[schema(schema_with = model_playlist_maximum_candidates_schema)]
    maximum_candidates: u16,
    may_incur_cost: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelPlaylistAvailability)]
struct ModelPlaylistAvailabilityResponse {
    available: bool,
    #[schema(required = true, schema_with = nullable_string_schema)]
    reason_code: Option<String>,
    #[schema(schema_with = playlist_role_id_schema)]
    role_id: &'static str,
    #[schema(required = true, schema_with = nullable_string_schema)]
    connection_name: Option<String>,
    #[schema(required = true, schema_with = nullable_string_schema)]
    model_id: Option<String>,
    #[schema(schema_with = playlist_quality_id_schema)]
    quality_evaluation_id: &'static str,
    job_kind: &'static str,
    disclosure: ModelPlaylistDisclosureResponse,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelPlaylistSuggestionStartRequest)]
struct ModelPlaylistSuggestionStartRequest {
    request: PlaylistSuggestionRequestWire,
    #[schema(schema_with = playlist_disclosure_version_schema)]
    disclosure_version: String,
    #[schema(schema_with = true_schema)]
    consent: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = EqDraftRequest)]
struct EqDraftRequestWire {
    #[schema(min_length = 1, max_length = 128)]
    name: String,
    #[schema(min_length = 2, max_length = 1000)]
    goal: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelEqDisclosure)]
struct ModelEqDisclosureResponse {
    #[schema(schema_with = eq_disclosure_version_schema)]
    version: &'static str,
    shared_with_provider: Vec<&'static str>,
    never_shared: Vec<&'static str>,
    may_incur_cost: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelEqAvailability)]
struct ModelEqAvailabilityResponse {
    available: bool,
    #[schema(required = true, schema_with = nullable_string_schema)]
    reason_code: Option<String>,
    #[schema(schema_with = eq_role_id_schema)]
    role_id: &'static str,
    #[schema(required = true, schema_with = nullable_string_schema)]
    connection_name: Option<String>,
    #[schema(required = true, schema_with = nullable_string_schema)]
    model_id: Option<String>,
    #[schema(schema_with = eq_quality_id_schema)]
    quality_evaluation_id: &'static str,
    job_kind: &'static str,
    disclosure: ModelEqDisclosureResponse,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelEqDraftStartRequest)]
struct ModelEqDraftStartRequest {
    request: EqDraftRequestWire,
    #[schema(schema_with = eq_disclosure_version_schema)]
    disclosure_version: String,
    #[schema(schema_with = true_schema)]
    consent: bool,
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
#[schema(as = ModelTagCleanupDisclosure)]
struct ModelTagCleanupDisclosureResponse {
    #[schema(schema_with = model_tag_cleanup_disclosure_version_schema)]
    version: &'static str,
    shared_with_provider: Vec<&'static str>,
    never_shared: Vec<&'static str>,
    #[schema(schema_with = model_tag_cleanup_maximum_tags_schema)]
    maximum_tags: usize,
    may_incur_cost: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelTagCleanupAvailability)]
struct ModelTagCleanupAvailabilityResponse {
    available: bool,
    #[schema(required = true, schema_with = nullable_string_schema)]
    reason_code: Option<String>,
    #[schema(schema_with = model_tag_cleanup_role_schema)]
    role_id: &'static str,
    #[schema(required = true, schema_with = nullable_string_schema)]
    connection_name: Option<String>,
    #[schema(required = true, schema_with = nullable_string_schema)]
    model_id: Option<String>,
    #[schema(schema_with = model_tag_cleanup_quality_schema)]
    quality_evaluation_id: &'static str,
    job_kind: &'static str,
    #[schema(pattern = "^[a-f0-9]{64}$")]
    catalog_signature: String,
    #[schema(pattern = "^[a-f0-9]{64}$")]
    vocabulary_fingerprint: String,
    #[schema(schema_with = nonnegative_integer_schema)]
    manual_tags: usize,
    #[schema(schema_with = model_tag_cleanup_request_count_schema)]
    estimated_provider_requests: usize,
    disclosure: ModelTagCleanupDisclosureResponse,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelTagCleanupStartRequest)]
struct ModelTagCleanupStartRequest {
    #[schema(schema_with = model_tag_cleanup_disclosure_version_schema)]
    disclosure_version: String,
    #[schema(schema_with = true_schema)]
    consent: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelTagCleanupApplyRequest)]
struct ModelTagCleanupApplyRequest {
    #[schema(pattern = "^[a-f0-9]{64}$")]
    catalog_signature: String,
    #[schema(pattern = "^[a-f0-9]{64}$")]
    vocabulary_fingerprint: String,
    #[schema(min_items = 1, max_items = 100)]
    items: Vec<TagCleanupSelectionRequest>,
    #[schema(min_length = 1, max_length = 32)]
    job_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredModelTagCleanupResult {
    schema_version: String,
    disclosure_version: String,
    role_id: String,
    role_fingerprint: String,
    engine_id: String,
    catalog_signature: String,
    vocabulary_fingerprint: String,
    catalog_tags: usize,
    suggestions: Vec<StoredModelTagCleanupSuggestion>,
    usage: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredModelTagCleanupSuggestion {
    id: String,
    source: String,
    target: String,
    origin: String,
    confidence: String,
    reason: String,
    source_track_count: u64,
    target_track_count: u64,
    merged: bool,
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

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
#[schema(as = ModelTaggingReviewQuery)]
struct ModelTaggingReviewQueryRequest {
    #[schema(required = false, schema_with = model_tagging_scope_ref)]
    scope: ModelTaggingScopeWire,
    #[schema(required = false, schema_with = review_status_default_pending_schema)]
    review: ReviewStatusWire,
    #[schema(required = false, minimum = 0, default = 0)]
    offset: usize,
    #[serde(default = "default_page_limit")]
    #[schema(required = false, minimum = 1, maximum = 100, default = 50)]
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

#[derive(Debug, Clone, Default, Deserialize, Serialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
#[schema(as = LibraryContextStartRequest)]
struct LibraryContextStartRequest {
    #[schema(required = false, default = false)]
    force: bool,
    #[schema(required = false, schema_with = model_tagging_scope_ref)]
    scope: ModelTaggingScopeWire,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
#[schema(as = ModelTaggingScope)]
pub(crate) struct ModelTaggingScopeWire {
    #[serde(rename = "type")]
    #[schema(required = false, schema_with = context_scope_kind_schema)]
    kind: ContextScopeKindWire,
    #[schema(required = false, max_length = 1024, default = "")]
    path: String,
    #[schema(required = false, default = true)]
    recursive: bool,
    #[schema(required = false, schema_with = context_track_ids_schema)]
    track_ids: Vec<i64>,
}

impl Default for ModelTaggingScopeWire {
    fn default() -> Self {
        Self {
            kind: ContextScopeKindWire::All,
            path: String::new(),
            recursive: true,
            track_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum ContextScopeKindWire {
    #[default]
    All,
    Folder,
    Tracks,
}

impl ModelTaggingScopeWire {
    fn into_parameters(mut self) -> Result<ContextScopeParameters, ApiError> {
        if self.path.len() > 1_024 || self.track_ids.len() > 5_000 {
            return Err(ApiError::validation());
        }
        let raw_path = self.path.trim();
        if raw_path.starts_with(['/', '\\'])
            || raw_path
                .as_bytes()
                .get(1)
                .is_some_and(|value| *value == b':')
        {
            return Err(ApiError::validation());
        }
        let normalized_path = raw_path.replace('\\', "/").trim_matches('/').to_owned();
        if normalized_path
            .split('/')
            .filter(|part| !part.is_empty())
            .any(|part| matches!(part, "." | ".."))
        {
            return Err(ApiError::validation());
        }
        let mut seen = std::collections::BTreeSet::new();
        self.track_ids.retain(|value| seen.insert(*value));
        if self.track_ids.iter().any(|value| *value <= 0) {
            return Err(ApiError::validation());
        }
        let kind = match self.kind {
            ContextScopeKindWire::All => ContextScopeKind::All,
            ContextScopeKindWire::Folder => ContextScopeKind::Folder,
            ContextScopeKindWire::Tracks => ContextScopeKind::Tracks,
        };
        match kind {
            ContextScopeKind::Tracks if self.track_ids.is_empty() => {
                return Err(ApiError::validation());
            }
            ContextScopeKind::All | ContextScopeKind::Folder if !self.track_ids.is_empty() => {
                return Err(ApiError::validation());
            }
            ContextScopeKind::All | ContextScopeKind::Tracks if !normalized_path.is_empty() => {
                return Err(ApiError::validation());
            }
            _ => {}
        }
        Ok(ContextScopeParameters {
            kind,
            path: normalized_path,
            recursive: self.recursive,
            track_ids: self.track_ids,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum ModelTaggingContextPolicyWire {
    #[default]
    Include,
    Skip,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
#[schema(as = ModelTaggingPlanRequest)]
struct ModelTaggingPlanRequest {
    #[schema(required = false, schema_with = model_tagging_scope_ref)]
    scope: ModelTaggingScopeWire,
    #[schema(required = false, schema_with = model_tagging_context_policy_schema)]
    context_policy: ModelTaggingContextPolicyWire,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelTaggingStartRequest)]
struct ModelTaggingStartRequest {
    #[serde(default)]
    #[schema(required = false, default = false)]
    force: bool,
    #[serde(default)]
    #[schema(required = false, schema_with = model_tagging_scope_ref)]
    scope: ModelTaggingScopeWire,
    #[serde(default)]
    #[schema(required = false, schema_with = model_tagging_context_policy_schema)]
    context_policy: ModelTaggingContextPolicyWire,
    #[schema(schema_with = model_tagging_disclosure_version_schema)]
    disclosure_version: String,
    #[schema(schema_with = true_schema)]
    consent: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelTaggingDisclosure)]
struct ModelTaggingDisclosureResponse {
    #[schema(schema_with = model_tagging_disclosure_version_schema)]
    version: &'static str,
    shared_with_provider: Vec<&'static str>,
    never_shared: Vec<&'static str>,
    allowed_tags: Vec<String>,
    #[schema(schema_with = model_tagging_tracks_per_request_schema)]
    tracks_per_request: usize,
    #[schema(schema_with = model_tagging_retry_limit_schema)]
    invalid_response_retry_limit: u8,
    may_incur_cost: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelTaggingAvailability)]
struct ModelTaggingAvailabilityResponse {
    available: bool,
    #[schema(required = true, schema_with = nullable_string_schema)]
    reason_code: Option<String>,
    #[schema(schema_with = model_tagging_role_schema)]
    role_id: &'static str,
    #[schema(required = true, schema_with = nullable_string_schema)]
    connection_name: Option<String>,
    #[schema(required = true, schema_with = nullable_string_schema)]
    model_id: Option<String>,
    #[schema(schema_with = model_tagging_quality_schema)]
    quality_evaluation_id: &'static str,
    job_kind: &'static str,
    #[schema(schema_with = nonnegative_integer_schema)]
    library_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    scope_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    planned_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    tracks_with_full_context: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    tracks_with_partial_context: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    tracks_missing_context: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    current_profiles: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    tracks_needing_tags: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    estimated_provider_requests: usize,
    disclosure: ModelTaggingDisclosureResponse,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = VoiceAnalyzerStatus)]
struct VoiceAnalyzerStatusResponse {
    #[schema(schema_with = voice_analyzer_id_schema)]
    analyzer_id: String,
    #[schema(schema_with = voice_analyzer_status_schema)]
    status: String,
    #[schema(required = true, schema_with = nullable_voice_reason_schema)]
    reason: Option<String>,
    model_filename: String,
    model_sha256: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = LibraryContextPassSummary)]
struct LibraryContextPassSummaryResponse {
    #[schema(schema_with = nonnegative_integer_schema)]
    completed_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    failed_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    skipped_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    total_tracks: usize,
    #[schema(required = false, default = true)]
    enabled: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = LibraryContextPasses)]
struct LibraryContextPassesResponse {
    audio_context: LibraryContextPassSummaryResponse,
    voice_detection: LibraryContextPassSummaryResponse,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = LibraryContextSummary)]
struct LibraryContextSummaryResponse {
    #[schema(schema_with = context_analyzer_id_schema)]
    analyzer: String,
    voice_analyzer: VoiceAnalyzerStatusResponse,
    passes: LibraryContextPassesResponse,
    #[schema(schema_with = nonnegative_integer_schema)]
    library_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    analyzed_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    full_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    partial_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    missing_tracks: usize,
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

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TrackContextDetail)]
struct TrackContextDetailResponse {
    #[schema(schema_with = positive_exclusive_integer_schema)]
    track_id: i64,
    title: String,
    artist: String,
    #[schema(schema_with = context_status_schema)]
    status: String,
    #[schema(schema_with = context_analyzer_id_schema)]
    analyzer_id: String,
    #[schema(required = true, schema_with = nullable_confidence_schema)]
    confidence: Option<String>,
    #[schema(required = true, schema_with = openapi_nullable_datetime)]
    updated_at: Option<String>,
    #[schema(required = true, schema_with = nullable_object_schema)]
    summary: Option<Map<String, Value>>,
    #[schema(schema_with = context_timeline_schema)]
    timeline: Vec<BTreeMap<String, f64>>,
    #[schema(schema_with = context_sections_schema)]
    sections: Vec<Map<String, Value>>,
    #[schema(required = true, schema_with = nullable_object_schema)]
    technical: Option<Map<String, Value>>,
    #[schema(required = true, schema_with = nullable_object_schema)]
    stages: Option<Map<String, Value>>,
    #[schema(required = true, schema_with = nullable_string_schema)]
    error: Option<String>,
}

pub(crate) fn assistant_router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default()
        .routes(routes!(start_library_analysis))
        .routes(routes!(library_analysis_summary))
        .routes(routes!(start_library_audio_analysis))
        .routes(routes!(library_audio_analysis_summary))
        .routes(routes!(start_library_context_analysis))
        .routes(routes!(library_context_summary))
        .routes(routes!(library_track_context))
        .routes(routes!(suggest_playlist))
        .routes(routes!(model_playlist_status))
        .routes(routes!(start_model_playlist_suggestion))
        .routes(routes!(model_eq_status))
        .routes(routes!(start_model_eq_draft))
        .routes(routes!(tag_catalog))
        .routes(routes!(get_tag_vocabulary))
        .routes(routes!(update_tag_vocabulary))
        .routes(routes!(model_tag_cleanup_status))
        .routes(routes!(start_model_tag_cleanup))
        .routes(routes!(apply_model_tag_cleanup))
        .routes(routes!(preview_tag_cleanup))
        .routes(routes!(apply_tag_cleanup))
        .routes(routes!(model_tagging_status))
        .routes(routes!(plan_model_tagging))
        .routes(routes!(start_model_tagging))
        .routes(routes!(rename_tag))
        .routes(routes!(patch_tags_bulk))
        .routes(routes!(review_analysis_tags_bulk))
        .routes(routes!(query_model_library_tags))
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
    path = "/assistant/library-context/jobs",
    operation_id = "start_library_context_analysis_api_assistant_library_context_jobs_post",
    request_body = LibraryContextStartRequest,
    responses(
        (status = 202, description = "Successful Response", body = BackgroundJobResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
async fn start_library_context_analysis(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<LibraryContextStartRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<BackgroundJobResponse>), ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let parameters = ContextAnalysisParameters {
        force: payload.force,
        scope: payload.scope.into_parameters()?,
    };
    let parameters = serde_json::to_value(parameters).map_err(|_| ApiError::internal())?;
    let (job, created) = state
        .jobs
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)?
        .enqueue_unique_active(LIBRARY_CONTEXT_JOB_KIND, parameters.clone())
        .await
        .map_err(map_job_error)?;
    if !created && job.parameters != parameters.as_object().cloned().unwrap_or_default() {
        return Err(ApiError::conflict_message(
            "Another library context analysis is already running. Wait for it to finish or cancel it first.",
        ));
    }
    Ok((StatusCode::ACCEPTED, Json(job_response(job)?)))
}

#[utoipa::path(
    get,
    path = "/assistant/library-context/summary",
    operation_id = "library_context_summary_api_assistant_library_context_summary_get",
    responses((status = 200, description = "Successful Response", body = LibraryContextSummaryResponse)),
    tag = "assistant"
)]
async fn library_context_summary(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<LibraryContextSummaryResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let summary = analysis_service(&state)?
        .context_summary()
        .await
        .map_err(map_local_analysis_error)?;
    Ok(Json(library_context_summary_response(summary)?))
}

#[utoipa::path(
    get,
    path = "/assistant/library-context/tracks/{track_id}",
    operation_id = "library_track_context_api_assistant_library_context_tracks__track_id__get",
    params(("track_id" = i128, Path, description = "Track Id")),
    responses(
        (status = 200, description = "Successful Response", body = TrackContextDetailResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
async fn library_track_context(
    State(state): State<HttpState>,
    headers: HeaderMap,
    track_id: Result<Path<i128>, PathRejection>,
) -> Result<Json<TrackContextDetailResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Path(track_id) = track_id.map_err(|_| ApiError::validation())?;
    let track_id = i64::try_from(track_id)
        .ok()
        .and_then(|value| TrackId::new(value).ok())
        .ok_or_else(ApiError::validation)?;
    let detail = analysis_service(&state)?
        .context_detail(track_id)
        .await
        .map_err(map_local_analysis_error)?
        .ok_or_else(|| ApiError::not_found_message("Track not found"))?;
    Ok(Json(track_context_detail_response(detail)?))
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
    path = "/assistant/playlists/model-status",
    operation_id = "model_playlist_status_api_assistant_playlists_model_status_get",
    responses((status = 200, description = "Successful Response", body = ModelPlaylistAvailabilityResponse)),
    tag = "assistant"
)]
async fn model_playlist_status(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<ModelPlaylistAvailabilityResponse>, ApiError> {
    authorize(&state, &headers).await?;
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
    operation_id = "start_model_playlist_suggestion_api_assistant_playlists_model_suggestions_jobs_post",
    request_body = ModelPlaylistSuggestionStartRequest,
    responses(
        (status = 202, description = "Successful Response", body = BackgroundJobResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
async fn start_model_playlist_suggestion(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<ModelPlaylistSuggestionStartRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<BackgroundJobResponse>), ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if payload.disclosure_version != "assistant-playlist-model-disclosure/v2" || !payload.consent {
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
    operation_id = "model_eq_status_api_assistant_eq_model_status_get",
    responses((status = 200, description = "Successful Response", body = ModelEqAvailabilityResponse)),
    tag = "assistant"
)]
async fn model_eq_status(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<ModelEqAvailabilityResponse>, ApiError> {
    authorize(&state, &headers).await?;
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
    operation_id = "start_model_eq_draft_api_assistant_eq_drafts_jobs_post",
    request_body = ModelEqDraftStartRequest,
    responses(
        (status = 202, description = "Successful Response", body = BackgroundJobResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
async fn start_model_eq_draft(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<ModelEqDraftStartRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<BackgroundJobResponse>), ApiError> {
    authorize(&state, &headers).await?;
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

struct ModelRoleAvailability {
    reason_code: Option<String>,
    connection_name: Option<String>,
    model_id: Option<String>,
    runtime_fingerprint: Option<String>,
}

async fn model_role_availability(
    state: &HttpState,
    role_id: &str,
    evaluation_id: &str,
) -> Result<ModelRoleAvailability, ApiError> {
    let providers = state
        .providers
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)?;
    let role = providers
        .provider_service()
        .list_model_roles()
        .await
        .map_err(map_provider_error)?
        .into_iter()
        .find(|role| role.role_id == role_id);
    let (reason_code, runtime_fingerprint) = match providers
        .quality_service()
        .prepare_quality_gated_role_execution(role_id, evaluation_id)
        .await
    {
        Ok(execution) => (None, Some(execution.fingerprint)),
        Err(error)
            if error.kind()
                == music_application::assistant::ProviderServiceErrorKind::Dependency =>
        {
            return Err(map_model_quality_error(error));
        }
        Err(error) => {
            let fingerprint = providers
                .provider_service()
                .current_role_runtime_fingerprint(role_id)
                .await
                .map_err(map_provider_error)?;
            (Some(error.code().to_owned()), fingerprint)
        }
    };
    Ok(ModelRoleAvailability {
        connection_name: role.as_ref().and_then(|role| role.connection_name.clone()),
        model_id: role
            .filter(|role| role.configuration_available)
            .map(|role| role.model_id),
        reason_code,
        runtime_fingerprint,
    })
}

async fn enqueue_model_feature(
    state: &HttpState,
    job_kind: &'static str,
    parameters: Value,
    conflict_code: &'static str,
    conflict_message: &'static str,
) -> Result<(StatusCode, Json<BackgroundJobResponse>), ApiError> {
    let expected = parameters
        .as_object()
        .cloned()
        .ok_or_else(ApiError::validation)?;
    let jobs = state
        .jobs
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)?;
    let (job, created) = jobs
        .enqueue_unique_active(job_kind, Value::Object(expected.clone()))
        .await
        .map_err(map_job_error)?;
    if !created && job.parameters != expected {
        return Err(ApiError::coded_conflict(conflict_code, conflict_message));
    }
    Ok((StatusCode::ACCEPTED, Json(job_response(job)?)))
}

fn playlist_disclosure() -> ModelPlaylistDisclosureResponse {
    ModelPlaylistDisclosureResponse {
        version: "assistant-playlist-model-disclosure/v2",
        shared_with_provider: vec![
            "Your mood prompt, duration, tempo filters, and requested energy flow",
            "Up to 100 locally prefiltered candidate IDs and descriptive metadata",
            "Candidate titles, artists, albums, origins, genres, durations, and BPM values",
            "Your database mood tags, generated analysis tags, and numeric audio-signal summaries",
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

fn eq_disclosure() -> ModelEqDisclosureResponse {
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

#[utoipa::path(
    get,
    path = "/assistant/library-tags/model-status",
    operation_id = "model_music_tagging_status_api_assistant_library_tags_model_status_get",
    responses((status = 200, description = "Successful Response", body = ModelTaggingAvailabilityResponse)),
    tag = "assistant-tags"
)]
async fn model_tagging_status(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<ModelTaggingAvailabilityResponse>, ApiError> {
    authorize(&state, &headers).await?;
    Ok(Json(
        model_tagging_availability(
            &state,
            ContextScopeParameters::default(),
            ModelTaggingContextPolicyWire::Include,
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
async fn plan_model_tagging(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<ModelTaggingPlanRequest>, JsonRejection>,
) -> Result<Json<ModelTaggingAvailabilityResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let scope = payload.scope.into_parameters()?;
    Ok(Json(
        model_tagging_availability(&state, scope, payload.context_policy).await?,
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
async fn start_model_tagging(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<ModelTaggingStartRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<BackgroundJobResponse>), ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if payload.disclosure_version != "assistant-model-music-tagging-disclosure/v10"
        || !payload.consent
    {
        return Err(ApiError::validation());
    }
    let scope = payload.scope.into_parameters()?;
    scope.to_scope().map_err(|_| ApiError::validation())?;
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
        "disclosure_version": payload.disclosure_version,
        "consent": true,
        "role_fingerprint": role.fingerprint,
        "vocabulary_fingerprint": vocabulary.fingerprint,
        "scope": scope,
        "context_policy": payload.context_policy,
        "force": payload.force,
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

async fn model_tagging_availability(
    state: &HttpState,
    scope: ContextScopeParameters,
    context_policy: ModelTaggingContextPolicyWire,
) -> Result<ModelTaggingAvailabilityResponse, ApiError> {
    let scope_filter = scope.to_scope().map_err(|_| ApiError::validation())?;
    let role = model_role_availability(state, "music_tagger", "music-tagging-quality-v1").await?;
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
    let current_profiles = if let Some(fingerprint) = role.runtime_fingerprint.as_deref() {
        let mut current = 0_usize;
        for track in &planned {
            let signature = model_tag_source_signature(
                &track.track,
                fingerprint,
                &vocabulary.fingerprint,
                contexts.get(&track.track.id),
            )
            .map_err(|_| ApiError::internal())?;
            if track.analyses.iter().any(|analysis| {
                analysis.analyzer_id == MODEL_TAG_ANALYZER_ID
                    && analysis.source_signature == signature
            }) {
                current = current.saturating_add(1);
            }
        }
        current
    } else {
        0
    };
    let tracks_needing_tags = planned.len().saturating_sub(current_profiles);
    Ok(ModelTaggingAvailabilityResponse {
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
        estimated_provider_requests: tracks_needing_tags.div_ceil(MODEL_TAG_BATCH_SIZE),
        disclosure: model_tagging_disclosure(&vocabulary),
    })
}

fn model_tagging_disclosure(vocabulary: &TagVocabularySnapshot) -> ModelTaggingDisclosureResponse {
    ModelTaggingDisclosureResponse {
        version: "assistant-model-music-tagging-disclosure/v10",
        shared_with_provider: vec![
            "Indexed titles, display titles, artists, albums, origins, and genres",
            "Canonical library-relative paths, including folder and file names, treated as untrusted descriptive context",
            "Track durations and BPM values when available",
            "Current bounded local track context when available: intensity, loudness, rhythmic drive, brightness, density and spectral-change trajectories; tempo development; major acoustic sections and transitions; structural repetition; analyzer confidence; and optional local voice/instrumental classifier score and coverage (or explicit unknown/unavailable status)",
            "A server-assigned numeric track ID used only to match the response",
            "The full operator-managed canonical tag ID, name, group, definition, exact-alias, and bounded semantic context cue index; the model may return only IDs from this index",
        ],
        never_shared: vec![
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
        tracks_per_request: MODEL_TAG_BATCH_SIZE,
        invalid_response_retry_limit: MODEL_TAGGER_INVALID_RESPONSE_RETRY_LIMIT,
        may_incur_cost: true,
    }
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
    path = "/assistant/library-tags/catalog/model-cleanup-status",
    operation_id = "model_library_tag_cleanup_status_api_assistant_library_tags_catalog_model_cleanup_status_get",
    responses((status = 200, description = "Successful Response", body = ModelTagCleanupAvailabilityResponse)),
    tag = "assistant-tags"
)]
async fn model_tag_cleanup_status(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<ModelTagCleanupAvailabilityResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let role = model_role_availability(&state, "tag_cleanup", "tag-cleanup-quality-v1").await?;
    let assistant = service(&state)?;
    let usage = assistant.tag_usage().await.map_err(map_assistant_error)?;
    let vocabulary = assistant.vocabulary().await.map_err(map_assistant_error)?;
    let signature = catalog_signature(&usage).map_err(|_| ApiError::internal())?;
    let mut reason_code = role.reason_code;
    if reason_code.is_none() && usage.is_empty() {
        reason_code = Some("tag_catalog_empty".to_owned());
    }
    if reason_code.is_none() && usage.len() > MAX_MODEL_CLEANUP_TAGS {
        reason_code = Some("tag_catalog_too_large".to_owned());
    }
    let estimated_provider_requests = if usage.len() <= MAX_MODEL_CLEANUP_TAGS {
        ModelTagCleanupTask::new(&usage, vocabulary.clone())
            .map_err(|_| ApiError::internal())?
            .total_model_batches()
    } else {
        usage.len().div_ceil(MODEL_TAG_CLEANUP_BATCH_SIZE).min(25)
    };
    Ok(Json(ModelTagCleanupAvailabilityResponse {
        available: reason_code.is_none(),
        reason_code,
        role_id: "tag_cleanup",
        connection_name: role.connection_name,
        model_id: role.model_id,
        quality_evaluation_id: "tag-cleanup-quality-v1",
        job_kind: MODEL_TAG_CLEANUP_JOB_KIND,
        catalog_signature: signature,
        vocabulary_fingerprint: vocabulary.fingerprint,
        manual_tags: usage.len(),
        estimated_provider_requests,
        disclosure: model_tag_cleanup_disclosure(),
    }))
}

#[utoipa::path(
    post,
    path = "/assistant/library-tags/catalog/model-cleanup-jobs",
    operation_id = "start_model_library_tag_cleanup_api_assistant_library_tags_catalog_model_cleanup_jobs_post",
    request_body = ModelTagCleanupStartRequest,
    responses(
        (status = 202, description = "Successful Response", body = BackgroundJobResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant-tags"
)]
async fn start_model_tag_cleanup(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<ModelTagCleanupStartRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<BackgroundJobResponse>), ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if payload.disclosure_version != "assistant-model-tag-cleanup-disclosure/v3" || !payload.consent
    {
        return Err(ApiError::validation());
    }
    let providers = state
        .providers
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)?;
    let role = providers
        .quality_service()
        .prepare_quality_gated_role_execution("tag_cleanup", "tag-cleanup-quality-v1")
        .await
        .map_err(map_model_quality_error)?;
    let assistant = service(&state)?;
    let usage = assistant.tag_usage().await.map_err(map_assistant_error)?;
    if usage.is_empty() {
        return Err(ApiError::coded_conflict(
            "tag_catalog_empty",
            "Add at least one mood-library tag before requesting model cleanup.",
        ));
    }
    if usage.len() > MAX_MODEL_CLEANUP_TAGS {
        return Err(ApiError::coded_conflict(
            "tag_catalog_too_large",
            "Model cleanup currently supports at most 500 tags.",
        ));
    }
    let vocabulary = assistant.vocabulary().await.map_err(map_assistant_error)?;
    let parameters = json!({
        "role_id": "tag_cleanup",
        "quality_evaluation_id": "tag-cleanup-quality-v1",
        "disclosure_version": payload.disclosure_version,
        "consent": true,
        "role_fingerprint": role.fingerprint,
        "catalog_signature": catalog_signature(&usage).map_err(|_| ApiError::internal())?,
        "vocabulary_fingerprint": vocabulary.fingerprint,
    });
    enqueue_model_feature(
        &state,
        MODEL_TAG_CLEANUP_JOB_KIND,
        parameters,
        "model_tag_cleanup_in_progress",
        "Another model tag-cleanup job is already running. Wait for it to finish or cancel it first.",
    )
    .await
}

#[utoipa::path(
    post,
    path = "/assistant/library-tags/catalog/model-cleanup-apply",
    operation_id = "apply_model_library_tag_cleanup_api_assistant_library_tags_catalog_model_cleanup_apply_post",
    request_body = ModelTagCleanupApplyRequest,
    responses(
        (status = 200, description = "Successful Response", body = TagCleanupApplyResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant-tags"
)]
async fn apply_model_tag_cleanup(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<ModelTagCleanupApplyRequest>, JsonRejection>,
) -> Result<Json<TagCleanupApplyResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if !valid_sha256(&payload.catalog_signature)
        || !valid_sha256(&payload.vocabulary_fingerprint)
        || payload.job_id.is_empty()
        || payload.job_id.len() > 32
        || payload.items.is_empty()
        || payload.items.len() > 100
    {
        return Err(ApiError::validation());
    }
    let job = state
        .jobs
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)?
        .get(&payload.job_id)
        .await
        .map_err(map_job_error)?
        .filter(|job| job.kind == MODEL_TAG_CLEANUP_JOB_KIND)
        .ok_or_else(|| {
            ApiError::coded(
                StatusCode::NOT_FOUND,
                "model_tag_cleanup_job_not_found",
                "Model tag-cleanup proposal not found.",
            )
        })?;
    if job.status != JobStatus::Succeeded || job.result.is_none() {
        return Err(ApiError::coded_conflict(
            "model_tag_cleanup_job_incomplete",
            "Model tag-cleanup proposal is not complete.",
        ));
    }
    let result = serde_json::from_value::<StoredModelTagCleanupResult>(Value::Object(
        job.result.unwrap_or_default(),
    ))
    .ok()
    .filter(valid_stored_model_tag_cleanup_result)
    .ok_or_else(|| {
        ApiError::coded_conflict(
            "model_tag_cleanup_result_invalid",
            "Stored model tag-cleanup proposal is invalid.",
        )
    })?;
    if payload.catalog_signature != result.catalog_signature {
        return Err(ApiError::coded(
            StatusCode::UNPROCESSABLE_ENTITY,
            "model_tag_cleanup_signature_mismatch",
            "The selected proposal signature does not match its job.",
        ));
    }
    if payload.vocabulary_fingerprint != result.vocabulary_fingerprint {
        return Err(ApiError::coded(
            StatusCode::UNPROCESSABLE_ENTITY,
            "model_tag_cleanup_vocabulary_mismatch",
            "The selected vocabulary fingerprint does not match its job.",
        ));
    }
    let assistant = service(&state)?;
    if assistant
        .vocabulary()
        .await
        .map_err(map_assistant_error)?
        .fingerprint
        != result.vocabulary_fingerprint
    {
        return Err(ApiError::coded_conflict(
            "tag_cleanup_stale",
            "The tag vocabulary changed after this proposal was created. Run cleanup again.",
        ));
    }
    let allowed_pairs = result
        .suggestions
        .iter()
        .map(|item| CleanupSelection {
            source: item.source.clone(),
            target: item.target.clone(),
        })
        .collect::<Vec<_>>();
    let selections = payload
        .items
        .iter()
        .map(|item| CleanupSelection {
            source: item.source.clone(),
            target: item.target.clone(),
        })
        .collect::<Vec<_>>();
    let outcome = assistant
        .apply_reviewed_cleanup(
            &result.catalog_signature,
            &result.vocabulary_fingerprint,
            &selections,
            &allowed_pairs,
        )
        .await
        .map_err(map_model_cleanup_apply_error)?;
    Ok(Json(TagCleanupApplyResponse {
        schema_version: TAG_CLEANUP_APPLY_SCHEMA,
        requested_items: selections.len(),
        applied: outcome
            .applied
            .into_iter()
            .map(|item| ManualTagRenameResponse {
                source: item.source,
                target: item.target,
                affected_tracks: item.affected_tracks,
                merged: item.merged,
            })
            .collect(),
        catalog_signature: outcome.catalog_signature,
    }))
}

fn model_tag_cleanup_disclosure() -> ModelTagCleanupDisclosureResponse {
    ModelTagCleanupDisclosureResponse {
        version: "assistant-model-tag-cleanup-disclosure/v3",
        shared_with_provider: vec![
            "Manual source tags not already resolved by deterministic cleanup rules",
            "The number of tracks using each shared source tag",
            "The operator-managed canonical tag IDs, names, groups, and definitions; the model may return only those IDs or no match",
        ],
        never_shared: vec![
            "Audio or media files",
            "Track titles, artists, albums, metadata, or filesystem paths",
            "Playlists, generated tags, review history, or provider credentials",
        ],
        maximum_tags: MAX_MODEL_CLEANUP_TAGS,
        may_incur_cost: true,
    }
}

fn valid_stored_model_tag_cleanup_result(result: &StoredModelTagCleanupResult) -> bool {
    let mut ids = BTreeSet::new();
    let mut sources = BTreeSet::new();
    result.schema_version == "assistant-model-tag-cleanup-job-result/v3"
        && result.disclosure_version == "assistant-model-tag-cleanup-disclosure/v3"
        && result.role_id == "tag_cleanup"
        && valid_sha256(&result.role_fingerprint)
        && result.engine_id == MODEL_TAG_CLEANUP_ENGINE_ID
        && valid_sha256(&result.catalog_signature)
        && valid_sha256(&result.vocabulary_fingerprint)
        && (1..=MAX_MODEL_CLEANUP_TAGS).contains(&result.catalog_tags)
        && result.suggestions.len() <= 100
        && result.usage.is_object()
        && result.suggestions.iter().all(|item| {
            valid_sha256(&item.id)
                && !item.source.is_empty()
                && !item.target.is_empty()
                && ids.insert(item.id.as_str())
                && sources.insert(item.source.as_str())
                && matches!(item.origin.as_str(), "local-rule" | "model")
                && matches!(item.confidence.as_str(), "high" | "medium" | "low")
                && (1..=512).contains(&item.reason.chars().count())
                && item.source_track_count > 0
                && ((!item.merged && item.target_track_count == 0)
                    || (item.merged && item.target_track_count > 0))
        })
}

fn map_model_cleanup_apply_error(error: AssistantServiceError) -> ApiError {
    match error {
        AssistantServiceError::Stale(_) => ApiError::coded_conflict(
            "tag_cleanup_stale",
            "Mood-library tags changed after this proposal was created. Run cleanup again.",
        ),
        AssistantServiceError::InvalidCleanupSelection(_)
        | AssistantServiceError::Validation(_) => ApiError::coded(
            StatusCode::UNPROCESSABLE_ENTITY,
            "tag_cleanup_invalid_selection",
            "A selected cleanup item is not part of the current proposal.",
        ),
        other => map_assistant_error(other),
    }
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
            scope: None,
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
async fn query_model_library_tags(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<ModelTaggingReviewQueryRequest>, JsonRejection>,
) -> Result<Json<LibraryTagPageResponse>, ApiError> {
    authorize(&state, &headers).await?;
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

fn library_context_summary_response(
    summary: LibraryContextSummary,
) -> Result<LibraryContextSummaryResponse, ApiError> {
    Ok(LibraryContextSummaryResponse {
        analyzer: summary.analyzer,
        voice_analyzer: voice_analyzer_status_response(summary.voice_analyzer),
        passes: LibraryContextPassesResponse {
            audio_context: context_pass_response(summary.audio_context),
            voice_detection: context_pass_response(summary.voice_detection),
        },
        library_tracks: summary.library_tracks,
        analyzed_tracks: summary.analyzed_tracks,
        full_tracks: summary.full_tracks,
        partial_tracks: summary.partial_tracks,
        missing_tracks: summary.missing_tracks,
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

fn voice_analyzer_status_response(value: VoiceAnalyzerStatus) -> VoiceAnalyzerStatusResponse {
    VoiceAnalyzerStatusResponse {
        analyzer_id: value.analyzer_id,
        status: value.status,
        reason: value.reason,
        model_filename: value.model_filename,
        model_sha256: value.model_sha256,
    }
}

fn context_pass_response(value: LibraryContextPassSummary) -> LibraryContextPassSummaryResponse {
    LibraryContextPassSummaryResponse {
        completed_tracks: value.completed_tracks,
        failed_tracks: value.failed_tracks,
        skipped_tracks: value.skipped_tracks,
        total_tracks: value.total_tracks,
        enabled: value.enabled,
    }
}

fn track_context_detail_response(
    value: TrackContextDetail,
) -> Result<TrackContextDetailResponse, ApiError> {
    let timeline = value
        .timeline
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|(key, value)| {
                    value
                        .as_f64()
                        .filter(|number| number.is_finite())
                        .map(|number| (key, number))
                        .ok_or_else(ApiError::internal)
                })
                .collect::<Result<BTreeMap<_, _>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(TrackContextDetailResponse {
        track_id: value.track_id.get(),
        title: value.title,
        artist: value.artist,
        status: value.status,
        analyzer_id: value.analyzer_id,
        confidence: value.confidence,
        updated_at: value
            .updated_at_unix_seconds
            .map(UnixSeconds::new)
            .map(format_rfc3339)
            .transpose()?,
        summary: value.summary,
        timeline,
        sections: value.sections,
        technical: value.technical,
        stages: value.stages,
        error: value.error,
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

fn review_status_default_pending_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["pending", "accepted", "rejected"]))
        .default(Some(json!("pending")))
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
fn playlist_disclosure_version_schema() -> RefOr<Schema> {
    const_string_schema("assistant-playlist-model-disclosure/v2")
}
fn playlist_role_id_schema() -> RefOr<Schema> {
    const_string_schema("playlist_planner")
}
fn playlist_quality_id_schema() -> RefOr<Schema> {
    const_string_schema("playlist-quality-v1")
}
fn model_playlist_maximum_candidates_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(1))
        .maximum(Some(100))
        .into()
}
fn model_tag_cleanup_disclosure_version_schema() -> RefOr<Schema> {
    const_string_schema("assistant-model-tag-cleanup-disclosure/v3")
}
fn model_tag_cleanup_role_schema() -> RefOr<Schema> {
    const_string_schema("tag_cleanup")
}
fn model_tag_cleanup_quality_schema() -> RefOr<Schema> {
    const_string_schema("tag-cleanup-quality-v1")
}
fn model_tag_cleanup_maximum_tags_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(1))
        .maximum(Some(500))
        .into()
}
fn model_tag_cleanup_request_count_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(0))
        .maximum(Some(25))
        .into()
}
fn model_tagging_disclosure_version_schema() -> RefOr<Schema> {
    const_string_schema("assistant-model-music-tagging-disclosure/v10")
}
fn model_tagging_role_schema() -> RefOr<Schema> {
    const_string_schema("music_tagger")
}
fn model_tagging_quality_schema() -> RefOr<Schema> {
    const_string_schema("music-tagging-quality-v1")
}
fn model_tagging_context_policy_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["include", "skip"]))
        .default(Some(json!("include")))
        .into()
}
fn model_tagging_tracks_per_request_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(1))
        .maximum(Some(20))
        .into()
}
fn model_tagging_retry_limit_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(0))
        .maximum(Some(5))
        .into()
}
fn eq_disclosure_version_schema() -> RefOr<Schema> {
    const_string_schema("assistant-eq-draft-disclosure/v2")
}
fn eq_role_id_schema() -> RefOr<Schema> {
    const_string_schema("eq_assistant")
}
fn eq_quality_id_schema() -> RefOr<Schema> {
    const_string_schema("eq-quality-v1")
}

fn true_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Boolean)
        .extensions(Some([("const", json!(true))].into_iter().collect()))
        .into()
}

fn const_string_schema(value: &'static str) -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .extensions(Some([("const", json!(value))].into_iter().collect()))
        .into()
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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

fn context_scope_kind_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["all", "folder", "tracks"]))
        .default(Some(json!("all")))
        .into()
}

fn model_tagging_scope_ref() -> RefOr<Schema> {
    RefOr::Ref(utoipa::openapi::Ref::from_schema_name("ModelTaggingScope"))
}

fn context_track_ids_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(openapi_integer())
        .max_items(Some(5_000))
        .into()
}

fn context_analyzer_id_schema() -> RefOr<Schema> {
    const_string_schema("local-context/v2")
}

fn voice_analyzer_id_schema() -> RefOr<Schema> {
    const_string_schema("essentia-musicnn-voice/v1")
}

fn voice_analyzer_status_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["not_configured", "ready", "unavailable"]))
        .into()
}

fn nullable_voice_reason_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .enum_values(Some([
                        "model_missing",
                        "model_unreadable",
                        "unsupported_model",
                        "runtime_missing",
                    ])),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn context_status_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["full", "partial", "missing", "stale", "failed"]))
        .into()
}

fn free_object_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Object)
        .additional_properties(Some(AdditionalProperties::FreeForm(true)))
        .into()
}

fn nullable_object_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(free_object_schema())
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn context_timeline_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(
            ObjectBuilder::new()
                .schema_type(Type::Object)
                .additional_properties(Some(AdditionalProperties::RefOr(openapi_number()))),
        )
        .into()
}

fn context_sections_schema() -> RefOr<Schema> {
    ArrayBuilder::new().items(free_object_schema()).into()
}
