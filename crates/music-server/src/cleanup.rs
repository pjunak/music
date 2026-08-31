use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use futures_util::TryStreamExt;
use music_application::auth::SessionTouch;
use music_application::cleanup::{
    CleanupAnalysis, CleanupApplyOperation, CleanupApplyResult, CleanupBatchDetail,
    CleanupBatchSummary, CleanupError, CleanupFuture, CleanupInputValue, CleanupNameLookup,
    CleanupNameScoreError, CleanupNameScores, CleanupOperationKind, CleanupRevertResult,
    CleanupScope, CleanupVerificationError, CleanupVerificationResult,
    MAX_CLEANUP_APPLY_OPERATIONS, MAX_CLEANUP_REVERT_ITEMS, MAX_CLEANUP_SCOPE_LABEL_CHARS,
    MAX_CLEANUP_VERIFY_NAMES,
};
use music_application::cleanup_sources::{CleanupSource, CleanupSourceError};
use music_application::library::LibraryCoordinatorError;
use music_domain::{
    CleanupFolderSuggestion, CleanupRule, CleanupRuleSet, CleanupSuggestion, CleanupTrackPlan,
    CleanupValue, DEFAULT_CLEANUP_RULES, LibraryPath, TrackId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::time::Instant;
use utoipa::ToSchema;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{
    AdditionalProperties, AnyOfBuilder, ArrayBuilder, ObjectBuilder, Schema, Type,
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::error::{
    ApiError, HttpValidationErrorBody, openapi_datetime, openapi_integer,
    openapi_nullable_datetime, openapi_nullable_integer, openapi_nullable_string,
};
use crate::http::HttpState;

const MAX_SCOPE_TRACKS: usize = 5_000;
const MUSICBRAINZ_ROOT: &str = "https://musicbrainz.org/ws/2";
const MUSICBRAINZ_USER_AGENT: &str = "music-dnd-orchestrator/0.1 (https://github.com/pjunak/music)";
const MUSICBRAINZ_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MUSICBRAINZ_MIN_INTERVAL: Duration = Duration::from_millis(1_100);
const MUSICBRAINZ_MAX_RESPONSE_BYTES: usize = 1024 * 1024;

pub(crate) fn cleanup_router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default()
        .routes(routes!(analyze))
        .routes(routes!(verify_names))
        .routes(routes!(list_sources))
        .routes(routes!(update_source))
        .routes(routes!(apply_cleanup))
        .routes(routes!(list_batches))
        .routes(routes!(get_batch))
        .routes(routes!(revert_batch))
        .routes(routes!(revert_from_journal))
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CleanupScopeKind {
    All,
    Folder,
    Tracks,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CleanupRuleRequest {
    StripTrackNumbers,
    StripArtist,
    StripAlbum,
    StripJunk,
    NormalizeSeparators,
    NormalizeCase,
    TagTitle,
    TagArtist,
    TagAlbum,
    TagNumber,
    TagYear,
    RenameFolders,
}

impl From<CleanupRuleRequest> for CleanupRule {
    fn from(value: CleanupRuleRequest) -> Self {
        match value {
            CleanupRuleRequest::StripTrackNumbers => Self::StripTrackNumbers,
            CleanupRuleRequest::StripArtist => Self::StripArtist,
            CleanupRuleRequest::StripAlbum => Self::StripAlbum,
            CleanupRuleRequest::StripJunk => Self::StripJunk,
            CleanupRuleRequest::NormalizeSeparators => Self::NormalizeSeparators,
            CleanupRuleRequest::NormalizeCase => Self::NormalizeCase,
            CleanupRuleRequest::TagTitle => Self::TagTitle,
            CleanupRuleRequest::TagArtist => Self::TagArtist,
            CleanupRuleRequest::TagAlbum => Self::TagAlbum,
            CleanupRuleRequest::TagNumber => Self::TagNumber,
            CleanupRuleRequest::TagYear => Self::TagYear,
            CleanupRuleRequest::RenameFolders => Self::RenameFolders,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = CleanupScope)]
struct CleanupScopeRequest {
    #[serde(rename = "type")]
    #[schema(rename = "type", schema_with = cleanup_scope_kind_schema)]
    kind: CleanupScopeKind,
    #[serde(default)]
    #[schema(required = false, schema_with = cleanup_scope_path_schema)]
    path: String,
    #[serde(default = "default_recursive")]
    #[schema(required = false, schema_with = cleanup_recursive_schema)]
    recursive: bool,
    #[serde(default)]
    #[schema(required = false, schema_with = cleanup_scope_track_ids_schema)]
    track_ids: Vec<i64>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = AnalyzeRequest)]
struct AnalyzeRequest {
    scope: CleanupScopeRequest,
    /// Enabled rule ids; omit for the default set.
    #[schema(required = false, schema_with = nullable_cleanup_rules_schema)]
    rules: Option<Vec<CleanupRuleRequest>>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = VerifyRequest)]
struct VerifyRequest {
    #[schema(schema_with = cleanup_verify_names_schema)]
    names: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = VerifyResult)]
struct VerifyResponse {
    #[schema(schema_with = openapi_integer)]
    verified: usize,
    failed: Vec<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = CleanupSourceUpdate)]
struct CleanupSourceUpdateRequest {
    enabled: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = CleanupSourceOut)]
struct CleanupSourceResponse {
    id: String,
    label: String,
    description: String,
    enabled: bool,
    capabilities: Vec<String>,
    #[schema(required = true, schema_with = openapi_nullable_string)]
    credential_kind: Option<String>,
}

impl From<CleanupSource> for CleanupSourceResponse {
    fn from(source: CleanupSource) -> Self {
        Self {
            id: source.id,
            label: source.label,
            description: source.description,
            enabled: source.enabled,
            capabilities: source.capabilities,
            credential_kind: source.credential_kind,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CleanupApplyKindRequest {
    Rename,
    Tag,
    FolderRename,
}

impl From<CleanupApplyKindRequest> for CleanupOperationKind {
    fn from(value: CleanupApplyKindRequest) -> Self {
        match value {
            CleanupApplyKindRequest::Rename => Self::Rename,
            CleanupApplyKindRequest::Tag => Self::Tag,
            CleanupApplyKindRequest::FolderRename => Self::FolderRename,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CleanupInputValueRequest {
    Integer(i64),
    Text(String),
}

impl From<CleanupInputValueRequest> for CleanupInputValue {
    fn from(value: CleanupInputValueRequest) -> Self {
        match value {
            CleanupInputValueRequest::Integer(value) => Self::Integer(value),
            CleanupInputValueRequest::Text(value) => Self::Text(value),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = CleanupOpIn)]
struct CleanupApplyOperationRequest {
    #[schema(schema_with = openapi_integer)]
    track_id: i64,
    #[schema(schema_with = cleanup_apply_kind_schema)]
    kind: CleanupApplyKindRequest,
    #[serde(default)]
    #[schema(required = false, schema_with = openapi_nullable_string)]
    field: Option<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = cleanup_value_schema)]
    old: Option<CleanupInputValueRequest>,
    #[serde(default)]
    #[schema(required = false, schema_with = cleanup_value_schema)]
    new: Option<CleanupInputValueRequest>,
    #[serde(default)]
    #[schema(required = false, default = "")]
    path: String,
}

impl From<CleanupApplyOperationRequest> for CleanupApplyOperation {
    fn from(operation: CleanupApplyOperationRequest) -> Self {
        Self {
            track_id: operation.track_id,
            kind: operation.kind.into(),
            field: operation.field,
            old: operation.old.map(Into::into),
            new: operation.new.map(Into::into),
            path: operation.path,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = ApplyRequest)]
struct CleanupApplyRequest {
    #[schema(min_items = 1, max_items = 500)]
    ops: Vec<CleanupApplyOperationRequest>,
    #[serde(default)]
    #[schema(required = false, schema_with = openapi_nullable_integer)]
    batch_id: Option<i64>,
    #[serde(default)]
    #[schema(required = false, default = "", max_length = 512)]
    scope_label: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = CleanupSkip)]
struct CleanupSkipResponse {
    #[schema(schema_with = openapi_integer)]
    track_id: i64,
    reason: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = ApplyResult)]
struct CleanupApplyResponse {
    #[schema(required = true, schema_with = openapi_nullable_integer)]
    batch_id: Option<i64>,
    #[schema(schema_with = openapi_integer)]
    applied: usize,
    skipped: Vec<CleanupSkipResponse>,
}

impl From<CleanupApplyResult> for CleanupApplyResponse {
    fn from(result: CleanupApplyResult) -> Self {
        Self {
            batch_id: result.batch_id,
            applied: result.applied,
            skipped: result
                .skipped
                .into_iter()
                .map(|skip| CleanupSkipResponse {
                    track_id: skip.track_id,
                    reason: skip.reason,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = RevertJournalRequest)]
struct CleanupRevertJournalRequest {
    #[schema(schema_with = cleanup_revert_items_schema)]
    items: Vec<BTreeMap<String, Value>>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = RevertResult)]
struct CleanupRevertResponse {
    #[schema(schema_with = openapi_integer)]
    reverted: usize,
    skipped: Vec<CleanupSkipResponse>,
}

impl From<CleanupRevertResult> for CleanupRevertResponse {
    fn from(result: CleanupRevertResult) -> Self {
        Self {
            reverted: result.reverted,
            skipped: result
                .skipped
                .into_iter()
                .map(|skip| CleanupSkipResponse {
                    track_id: skip.track_id,
                    reason: skip.reason,
                })
                .collect(),
        }
    }
}

impl From<CleanupVerificationResult> for VerifyResponse {
    fn from(result: CleanupVerificationResult) -> Self {
        Self {
            verified: result.verified,
            failed: result.failed,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = BatchSummary)]
struct CleanupBatchSummaryResponse {
    #[schema(schema_with = openapi_integer)]
    id: i64,
    #[schema(schema_with = openapi_datetime)]
    created_at: String,
    scope_label: String,
    #[schema(schema_with = openapi_integer)]
    item_count: usize,
    #[schema(required = true, schema_with = openapi_nullable_datetime)]
    reverted_at: Option<String>,
}

impl TryFrom<CleanupBatchSummary> for CleanupBatchSummaryResponse {
    type Error = ApiError;

    fn try_from(batch: CleanupBatchSummary) -> Result<Self, Self::Error> {
        Ok(Self {
            id: batch.id,
            created_at: crate::auth::format_rfc3339(music_application::auth::UnixSeconds::new(
                batch.created_at_unix_seconds,
            ))?,
            scope_label: batch.scope_label,
            item_count: batch.item_count,
            reverted_at: batch
                .reverted_at_unix_seconds
                .map(music_application::auth::UnixSeconds::new)
                .map(crate::auth::format_rfc3339)
                .transpose()?,
        })
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = BatchDetail)]
struct CleanupBatchDetailResponse {
    #[schema(schema_with = openapi_integer)]
    id: i64,
    #[schema(schema_with = openapi_datetime)]
    created_at: String,
    scope_label: String,
    #[schema(schema_with = openapi_integer)]
    item_count: usize,
    #[schema(required = true, schema_with = openapi_nullable_datetime)]
    reverted_at: Option<String>,
    #[schema(schema_with = cleanup_batch_items_schema)]
    items: Vec<BTreeMap<String, Value>>,
}

impl TryFrom<CleanupBatchDetail> for CleanupBatchDetailResponse {
    type Error = ApiError;

    fn try_from(batch: CleanupBatchDetail) -> Result<Self, Self::Error> {
        Ok(Self {
            id: batch.id,
            created_at: crate::auth::format_rfc3339(music_application::auth::UnixSeconds::new(
                batch.created_at_unix_seconds,
            ))?,
            scope_label: batch.scope_label,
            item_count: batch.item_count,
            reverted_at: batch
                .reverted_at_unix_seconds
                .map(music_application::auth::UnixSeconds::new)
                .map(crate::auth::format_rfc3339)
                .transpose()?,
            items: batch
                .items
                .into_iter()
                .map(|item| item.into_iter().collect())
                .collect(),
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum CleanupValueResponse {
    Number(u32),
    Text(String),
}

impl From<CleanupValue> for CleanupValueResponse {
    fn from(value: CleanupValue) -> Self {
        match value {
            CleanupValue::Number(value) => Self::Number(value),
            CleanupValue::Text(value) => Self::Text(value),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = CleanupOpOut)]
struct CleanupOperationResponse {
    op_id: String,
    #[schema(schema_with = openapi_integer)]
    track_id: i64,
    #[schema(schema_with = cleanup_operation_kind_schema)]
    kind: String,
    #[schema(required = true, schema_with = openapi_nullable_string)]
    field: Option<String>,
    #[schema(required = true, schema_with = cleanup_value_schema)]
    old: Option<CleanupValueResponse>,
    #[schema(required = true, schema_with = cleanup_value_schema)]
    new: Option<CleanupValueResponse>,
    rules: Vec<String>,
    #[schema(schema_with = cleanup_confidence_schema)]
    confidence: String,
    #[schema(required = false, default = false)]
    verified: bool,
}

impl From<CleanupSuggestion> for CleanupOperationResponse {
    fn from(operation: CleanupSuggestion) -> Self {
        Self {
            op_id: operation.operation_id(),
            track_id: operation.track_id.get(),
            kind: operation.kind.as_str().to_owned(),
            field: operation.field.map(|field| field.as_str().to_owned()),
            old: operation.old.map(Into::into),
            new: operation.new.map(Into::into),
            rules: operation.rules,
            confidence: operation.confidence.as_str().to_owned(),
            verified: operation.verified,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = CleanupTrackPlanOut)]
struct CleanupTrackPlanResponse {
    #[schema(schema_with = openapi_integer)]
    track_id: i64,
    path: String,
    ops: Vec<CleanupOperationResponse>,
    notes: Vec<String>,
}

impl From<CleanupTrackPlan> for CleanupTrackPlanResponse {
    fn from(plan: CleanupTrackPlan) -> Self {
        Self {
            track_id: plan.track_id.get(),
            path: plan.path,
            ops: plan.operations.into_iter().map(Into::into).collect(),
            notes: plan.notes,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = CleanupFolderOut)]
struct CleanupFolderResponse {
    op_id: String,
    path: String,
    old: String,
    new: String,
    rules: Vec<String>,
    #[schema(schema_with = cleanup_confidence_schema)]
    confidence: String,
}

impl From<CleanupFolderSuggestion> for CleanupFolderResponse {
    fn from(folder: CleanupFolderSuggestion) -> Self {
        Self {
            op_id: folder.operation_id(),
            path: folder.path,
            old: folder.old,
            new: folder.new,
            rules: folder.rules,
            confidence: folder.confidence.as_str().to_owned(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = AnalyzeResult)]
struct AnalyzeResponse {
    #[schema(schema_with = openapi_integer)]
    scanned: usize,
    plans: Vec<CleanupTrackPlanResponse>,
    folders: Vec<CleanupFolderResponse>,
    pending_lookups: Vec<String>,
}

impl From<CleanupAnalysis> for AnalyzeResponse {
    fn from(analysis: CleanupAnalysis) -> Self {
        Self {
            scanned: analysis.scanned,
            plans: analysis
                .plans
                .into_iter()
                .filter(|plan| !plan.operations.is_empty() || !plan.notes.is_empty())
                .map(Into::into)
                .collect(),
            folders: analysis.folders.into_iter().map(Into::into).collect(),
            pending_lookups: analysis.pending_lookups,
        }
    }
}

#[utoipa::path(
    post,
    path = "/library/cleanup/analyze",
    operation_id = "analyze_api_library_cleanup_analyze_post",
    summary = "Analyze",
    request_body = AnalyzeRequest,
    responses(
        (status = 200, description = "Successful Response", body = AnalyzeResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library-cleanup"
)]
async fn analyze(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<AnalyzeRequest>, JsonRejection>,
) -> Result<Json<AnalyzeResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if payload.scope.track_ids.len() > MAX_SCOPE_TRACKS {
        return Err(ApiError::validation());
    }
    let scope = cleanup_scope(payload.scope)?;
    let rules = payload.rules.map_or(DEFAULT_CLEANUP_RULES, |rules| {
        rules
            .into_iter()
            .map(Into::into)
            .collect::<CleanupRuleSet>()
    });
    let library = crate::library::library(&state)?;
    let use_online_evidence = library
        .cleanup_sources
        .musicbrainz_enabled()
        .await
        .map_err(map_cleanup_source_error)?;
    let analysis = library
        .cleanup
        .analyze_with_online_evidence(scope, rules, use_online_evidence)
        .await
        .map_err(map_cleanup_error)?;
    Ok(Json(analysis.into()))
}

#[utoipa::path(
    get,
    path = "/library/cleanup/sources",
    operation_id = "list_cleanup_sources_api_library_cleanup_sources_get",
    summary = "List Cleanup Sources",
    responses(
        (status = 200, description = "Successful Response", body = Vec<CleanupSourceResponse>)
    ),
    tag = "library-cleanup"
)]
async fn list_sources(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<CleanupSourceResponse>>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let sources = crate::library::library(&state)?
        .cleanup_sources
        .sources()
        .await
        .map_err(map_cleanup_source_error)?;
    Ok(Json(sources.into_iter().map(Into::into).collect()))
}

#[utoipa::path(
    put,
    path = "/library/cleanup/sources/{source_id}",
    operation_id = "update_cleanup_source_api_library_cleanup_sources_source_id_put",
    summary = "Update Cleanup Source",
    params(("source_id" = String, Path, description = "Cleanup source id")),
    request_body = CleanupSourceUpdateRequest,
    responses(
        (status = 200, description = "Successful Response", body = CleanupSourceResponse),
        (status = 404, description = "Source not found"),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library-cleanup"
)]
async fn update_source(
    State(state): State<HttpState>,
    headers: HeaderMap,
    source_id: Result<Path<String>, PathRejection>,
    payload: Result<Json<CleanupSourceUpdateRequest>, JsonRejection>,
) -> Result<Json<CleanupSourceResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Path(source_id) = source_id.map_err(|_| ApiError::validation())?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let source = crate::library::library(&state)?
        .cleanup_sources
        .update(&source_id, payload.enabled)
        .await
        .map_err(map_cleanup_source_error)?;
    Ok(Json(source.into()))
}

#[utoipa::path(
    post,
    path = "/library/cleanup/verify",
    operation_id = "verify_names_api_library_cleanup_verify_post",
    summary = "Verify Names",
    request_body = VerifyRequest,
    responses(
        (status = 200, description = "Successful Response", body = VerifyResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library-cleanup"
)]
async fn verify_names(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<VerifyRequest>, JsonRejection>,
) -> Result<Json<VerifyResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if !(1..=MAX_CLEANUP_VERIFY_NAMES).contains(&payload.names.len()) {
        return Err(ApiError::validation());
    }
    let library = crate::library::library(&state)?;
    if !library
        .cleanup_sources
        .musicbrainz_enabled()
        .await
        .map_err(map_cleanup_source_error)?
    {
        return Ok(Json(VerifyResponse {
            verified: 0,
            failed: payload.names,
        }));
    }
    let result = library
        .cleanup_verification
        .verify(payload.names)
        .await
        .map_err(map_cleanup_verification_error)?;
    Ok(Json(result.into()))
}

#[utoipa::path(
    post,
    path = "/library/cleanup/apply",
    operation_id = "apply_cleanup_api_library_cleanup_apply_post",
    summary = "Apply Cleanup",
    request_body = CleanupApplyRequest,
    responses(
        (status = 200, description = "Successful Response", body = CleanupApplyResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library-cleanup"
)]
async fn apply_cleanup(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<CleanupApplyRequest>, JsonRejection>,
) -> Result<Json<CleanupApplyResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if !(1..=MAX_CLEANUP_APPLY_OPERATIONS).contains(&payload.ops.len())
        || payload.scope_label.chars().count() > MAX_CLEANUP_SCOPE_LABEL_CHARS
    {
        return Err(ApiError::validation());
    }
    let result = crate::library::library(&state)?
        .coordinator
        .apply_cleanup(
            payload.batch_id,
            payload.scope_label,
            payload.ops.into_iter().map(Into::into).collect(),
        )
        .await
        .map_err(map_cleanup_apply_error)?;
    Ok(Json(result.into()))
}

#[utoipa::path(
    get,
    path = "/library/cleanup/batches",
    operation_id = "list_batches_api_library_cleanup_batches_get",
    summary = "List Batches",
    responses(
        (status = 200, description = "Successful Response", body = Vec<CleanupBatchSummaryResponse>)
    ),
    tag = "library-cleanup"
)]
async fn list_batches(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<CleanupBatchSummaryResponse>>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let batches = crate::library::library(&state)?
        .cleanup
        .batches()
        .await
        .map_err(map_cleanup_error)?;
    Ok(Json(
        batches
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<_, _>>()?,
    ))
}

#[utoipa::path(
    get,
    path = "/library/cleanup/batches/{batch_id}",
    operation_id = "get_batch_api_library_cleanup_batches__batch_id__get",
    summary = "Get Batch",
    params(("batch_id" = i128, Path)),
    responses(
        (status = 200, description = "Successful Response", body = CleanupBatchDetailResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library-cleanup"
)]
async fn get_batch(
    State(state): State<HttpState>,
    headers: HeaderMap,
    batch_id: Result<Path<i64>, PathRejection>,
) -> Result<Json<CleanupBatchDetailResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Path(batch_id) = batch_id.map_err(|_| ApiError::validation())?;
    let batch = crate::library::library(&state)?
        .cleanup
        .batch(batch_id)
        .await
        .map_err(map_cleanup_error)?
        .ok_or_else(|| ApiError::plain_not_found("cleanup batch not found"))?;
    Ok(Json(batch.try_into()?))
}

#[utoipa::path(
    post,
    path = "/library/cleanup/batches/{batch_id}/revert",
    operation_id = "revert_batch_api_library_cleanup_batches__batch_id__revert_post",
    summary = "Revert Batch",
    params(("batch_id" = i128, Path)),
    responses(
        (status = 200, description = "Successful Response", body = CleanupRevertResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library-cleanup"
)]
async fn revert_batch(
    State(state): State<HttpState>,
    headers: HeaderMap,
    batch_id: Result<Path<i64>, PathRejection>,
) -> Result<Json<CleanupRevertResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Path(batch_id) = batch_id.map_err(|_| ApiError::validation())?;
    let result = crate::library::library(&state)?
        .coordinator
        .revert_cleanup_batch(batch_id)
        .await
        .map_err(map_cleanup_revert_error)?;
    Ok(Json(result.into()))
}

#[utoipa::path(
    post,
    path = "/library/cleanup/revert",
    operation_id = "revert_from_journal_api_library_cleanup_revert_post",
    summary = "Revert From Journal",
    description = "Revert from an uploaded journal (the downloaded batch JSON). Exists\nfor the disaster path — app.db was wiped/rebuilt so the server-side\nbatch row is gone, but journal items carry paths and survive re-minted\ntrack ids.",
    request_body = CleanupRevertJournalRequest,
    responses(
        (status = 200, description = "Successful Response", body = CleanupRevertResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library-cleanup"
)]
async fn revert_from_journal(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<CleanupRevertJournalRequest>, JsonRejection>,
) -> Result<Json<CleanupRevertResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if !(1..=MAX_CLEANUP_REVERT_ITEMS).contains(&payload.items.len()) {
        return Err(ApiError::validation());
    }
    let result = crate::library::library(&state)?
        .coordinator
        .revert_cleanup_journal(
            payload
                .items
                .into_iter()
                .map(|item| item.into_iter().collect())
                .collect(),
        )
        .await
        .map_err(map_cleanup_revert_error)?;
    Ok(Json(result.into()))
}

fn cleanup_scope(scope: CleanupScopeRequest) -> Result<CleanupScope, ApiError> {
    match scope.kind {
        CleanupScopeKind::All => Ok(CleanupScope::All),
        CleanupScopeKind::Folder => {
            let normalized = scope.path.trim_matches('/').replace('\\', "/");
            let normalized = normalized.trim_matches('/');
            let path = if normalized.is_empty() {
                None
            } else {
                Some(
                    LibraryPath::parse(normalized.to_owned())
                        .map_err(|_| ApiError::bad_request("invalid cleanup folder path"))?,
                )
            };
            Ok(CleanupScope::Folder {
                path,
                recursive: scope.recursive,
            })
        }
        CleanupScopeKind::Tracks => {
            if scope.track_ids.is_empty() {
                return Err(ApiError::bad_request(
                    "scope.track_ids must be non-empty for a tracks scope",
                ));
            }
            let track_ids = scope
                .track_ids
                .into_iter()
                .map(|track_id| {
                    TrackId::new(track_id)
                        .map_err(|_| ApiError::bad_request("invalid cleanup track id"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CleanupScope::Tracks(track_ids))
        }
    }
}

fn map_cleanup_error(error: CleanupError) -> ApiError {
    match error {
        CleanupError::EmptyTrackScope => {
            ApiError::bad_request("scope.track_ids must be non-empty for a tracks scope")
        }
        CleanupError::Dependency { .. } => {
            tracing::error!(error = %error, "library cleanup analysis failed");
            ApiError::internal()
        }
    }
}

fn map_cleanup_verification_error(error: CleanupVerificationError) -> ApiError {
    match error {
        CleanupVerificationError::InvalidBatchSize => ApiError::validation(),
        CleanupVerificationError::Dependency { .. } => {
            tracing::error!(error = %error, "library cleanup verification persistence failed");
            ApiError::internal()
        }
    }
}

fn map_cleanup_source_error(error: CleanupSourceError) -> ApiError {
    match error {
        CleanupSourceError::UnknownSource => ApiError::plain_not_found("cleanup source not found"),
        CleanupSourceError::Dependency => {
            tracing::error!(error = %error, "cleanup source settings failed");
            ApiError::internal()
        }
    }
}

fn map_cleanup_apply_error(error: LibraryCoordinatorError) -> ApiError {
    match error {
        LibraryCoordinatorError::InvalidCleanupBatchSize
        | LibraryCoordinatorError::InvalidCleanupScopeLabel => ApiError::validation(),
        LibraryCoordinatorError::CleanupBatchNotFound => {
            ApiError::plain_not_found("batch not found")
        }
        LibraryCoordinatorError::CleanupBatchReverted => {
            ApiError::conflict("batch was already reverted; start a new cleanup run")
        }
        LibraryCoordinatorError::CommandQueueFull | LibraryCoordinatorError::Unavailable => {
            ApiError::service_unavailable()
        }
        error => {
            tracing::error!(error = %error, "library cleanup apply failed");
            ApiError::internal()
        }
    }
}

fn map_cleanup_revert_error(error: LibraryCoordinatorError) -> ApiError {
    match error {
        LibraryCoordinatorError::InvalidCleanupRevertSize => ApiError::validation(),
        LibraryCoordinatorError::CleanupBatchNotFound => {
            ApiError::plain_not_found("batch not found")
        }
        LibraryCoordinatorError::CleanupBatchReverted => {
            ApiError::conflict("batch already reverted")
        }
        LibraryCoordinatorError::CommandQueueFull | LibraryCoordinatorError::Unavailable => {
            ApiError::service_unavailable()
        }
        error => {
            tracing::error!(error = %error, "library cleanup revert failed");
            ApiError::internal()
        }
    }
}

const fn default_recursive() -> bool {
    true
}

fn cleanup_scope_kind_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["all", "folder", "tracks"]))
        .into()
}

fn cleanup_scope_path_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .default(Some(serde_json::json!("")))
        .into()
}

fn cleanup_recursive_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Boolean)
        .default(Some(serde_json::json!(true)))
        .into()
}

fn cleanup_scope_track_ids_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(openapi_integer())
        .max_items(Some(MAX_SCOPE_TRACKS))
        .into()
}

fn cleanup_verify_names_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(ObjectBuilder::new().schema_type(Type::String))
        .min_items(Some(1))
        .max_items(Some(MAX_CLEANUP_VERIFY_NAMES))
        .into()
}

fn nullable_cleanup_rules_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ArrayBuilder::new().items(
                    ObjectBuilder::new()
                        .schema_type(Type::String)
                        .enum_values(Some([
                            "strip_track_numbers",
                            "strip_artist",
                            "strip_album",
                            "strip_junk",
                            "normalize_separators",
                            "normalize_case",
                            "tag_title",
                            "tag_artist",
                            "tag_album",
                            "tag_number",
                            "tag_year",
                            "rename_folders",
                        ])),
                ),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .description(Some("Enabled rule ids; omit for the default set"))
            .build(),
    )
    .into()
}

fn cleanup_operation_kind_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["rename", "tag"]))
        .into()
}

fn cleanup_apply_kind_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["rename", "tag", "folder_rename"]))
        .into()
}

fn cleanup_confidence_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["high", "low"]))
        .into()
}

fn cleanup_value_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(openapi_integer())
            .item(ObjectBuilder::new().schema_type(Type::String))
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn cleanup_batch_items_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(
            ObjectBuilder::new()
                .schema_type(Type::Object)
                .additional_properties(Some(AdditionalProperties::FreeForm(true))),
        )
        .into()
}

fn cleanup_revert_items_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(
            ObjectBuilder::new()
                .schema_type(Type::Object)
                .additional_properties(Some(AdditionalProperties::FreeForm(true))),
        )
        .min_items(Some(1))
        .max_items(Some(MAX_CLEANUP_REVERT_ITEMS))
        .into()
}

#[derive(Debug, Clone)]
pub(crate) struct MusicBrainzNameLookup {
    client: reqwest::Client,
    base_url: Arc<str>,
    last_request_at: Arc<Mutex<Option<Instant>>>,
    minimum_interval: Duration,
}

impl MusicBrainzNameLookup {
    pub(crate) fn new() -> Result<Self, reqwest::Error> {
        Self::with_endpoint(MUSICBRAINZ_ROOT, MUSICBRAINZ_MIN_INTERVAL)
    }

    fn with_endpoint(
        base_url: impl Into<Arc<str>>,
        minimum_interval: Duration,
    ) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .user_agent(MUSICBRAINZ_USER_AGENT)
            .timeout(MUSICBRAINZ_REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            client,
            base_url: base_url.into(),
            last_request_at: Arc::new(Mutex::new(None)),
            minimum_interval,
        })
    }

    async fn scores(&self, name: &str) -> Result<CleanupNameScores, MusicBrainzLookupError> {
        let quoted = lucene_quote(name);
        let artist_query = format!("artist:{quoted}");
        let album_query = format!("releasegroup:{quoted}");
        let artist = self.top_score("artist", &artist_query, "artists").await?;
        let album = self
            .top_score("release-group", &album_query, "release-groups")
            .await?;
        CleanupNameScores::new(artist, album).map_err(MusicBrainzLookupError::InvalidScore)
    }

    async fn top_score(
        &self,
        resource: &str,
        query: &str,
        list_key: &'static str,
    ) -> Result<i32, MusicBrainzLookupError> {
        let endpoint = format!("{}/{resource}", self.base_url.trim_end_matches('/'));
        // Hold admission through receipt of the response headers and start the
        // next interval there. This is deliberately more conservative than
        // spacing local request construction: network latency can otherwise
        // make two requests arrive at MusicBrainz less than one interval apart.
        let mut last_request_at = self.last_request_at.lock().await;
        if let Some(previous_request) = *last_request_at {
            let next_request_at = previous_request + self.minimum_interval;
            if next_request_at > Instant::now() {
                tokio::time::sleep_until(next_request_at).await;
            }
        }
        let response = self
            .client
            .get(endpoint)
            .query(&[("query", query), ("fmt", "json"), ("limit", "1")])
            .send()
            .await;
        *last_request_at = Some(Instant::now());
        drop(last_request_at);
        let response = response
            .map_err(MusicBrainzLookupError::Http)?
            .error_for_status()
            .map_err(MusicBrainzLookupError::Http)?;
        if response
            .content_length()
            .is_some_and(|length| length > MUSICBRAINZ_MAX_RESPONSE_BYTES as u64)
        {
            return Err(MusicBrainzLookupError::ResponseTooLarge);
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream
            .try_next()
            .await
            .map_err(MusicBrainzLookupError::Http)?
        {
            let new_length = body
                .len()
                .checked_add(chunk.len())
                .ok_or(MusicBrainzLookupError::ResponseTooLarge)?;
            if new_length > MUSICBRAINZ_MAX_RESPONSE_BYTES {
                return Err(MusicBrainzLookupError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        parse_top_score(&body, list_key)
    }
}

impl CleanupNameLookup for MusicBrainzNameLookup {
    fn fetch_name_scores<'a>(&'a self, name: &'a str) -> CleanupFuture<'a, CleanupNameScores> {
        Box::pin(async move {
            self.scores(name)
                .await
                .map_err(|source| Box::new(source) as Box<dyn Error + Send + Sync>)
        })
    }
}

fn lucene_quote(name: &str) -> String {
    format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
}

fn parse_top_score(body: &[u8], list_key: &'static str) -> Result<i32, MusicBrainzLookupError> {
    let payload: Value = serde_json::from_slice(body).map_err(MusicBrainzLookupError::Json)?;
    let Some(entries) = payload.get(list_key) else {
        return Ok(0);
    };
    let entries = entries
        .as_array()
        .ok_or(MusicBrainzLookupError::InvalidPayload(
            "search result list is not an array",
        ))?;
    let Some(score) = entries.first().and_then(|entry| entry.get("score")) else {
        return Ok(0);
    };
    let score = if let Some(score) = score.as_i64() {
        i32::try_from(score)
            .map_err(|_| MusicBrainzLookupError::InvalidPayload("search score is out of range"))?
    } else if let Some(score) = score.as_str() {
        score
            .parse()
            .map_err(|_| MusicBrainzLookupError::InvalidPayload("search score is invalid"))?
    } else {
        return Err(MusicBrainzLookupError::InvalidPayload(
            "search score is invalid",
        ));
    };
    if !(0..=100).contains(&score) {
        return Err(MusicBrainzLookupError::InvalidPayload(
            "search score is out of range",
        ));
    }
    Ok(score)
}

#[derive(Debug)]
enum MusicBrainzLookupError {
    Http(reqwest::Error),
    Json(serde_json::Error),
    InvalidScore(CleanupNameScoreError),
    InvalidPayload(&'static str),
    ResponseTooLarge,
}

impl Display for MusicBrainzLookupError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(_) => formatter.write_str("MusicBrainz request failed"),
            Self::Json(_) => formatter.write_str("MusicBrainz returned invalid JSON"),
            Self::InvalidScore(_) => {
                formatter.write_str("MusicBrainz returned an invalid search result")
            }
            Self::InvalidPayload(detail) => {
                write!(
                    formatter,
                    "MusicBrainz returned an invalid search result: {detail}"
                )
            }
            Self::ResponseTooLarge => {
                formatter.write_str("MusicBrainz response exceeded the limit")
            }
        }
    }
}

impl Error for MusicBrainzLookupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Http(source) => Some(source),
            Self::Json(source) => Some(source),
            Self::InvalidScore(source) => Some(source),
            Self::InvalidPayload(_) | Self::ResponseTooLarge => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Arc;
    use std::time::Duration;

    use axum::Json;
    use axum::Router;
    use axum::extract::State;
    use axum::http::{HeaderMap, Uri};
    use axum::routing::get;
    use serde_json::{Value, json};
    use tokio::sync::Mutex;
    use tokio::time::Instant;

    use super::{MUSICBRAINZ_USER_AGENT, MusicBrainzNameLookup, lucene_quote, parse_top_score};

    #[derive(Debug, Clone)]
    struct ObservedRequest {
        path: String,
        query: String,
        user_agent: String,
        started_at: Instant,
    }

    async fn musicbrainz_fixture(
        State(observed): State<Arc<Mutex<Vec<ObservedRequest>>>>,
        headers: HeaderMap,
        uri: Uri,
    ) -> Json<Value> {
        observed.lock().await.push(ObservedRequest {
            path: uri.path().to_owned(),
            query: uri.query().unwrap_or_default().to_owned(),
            user_agent: headers
                .get("user-agent")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
            started_at: Instant::now(),
        });
        if uri.path().ends_with("/artist") {
            Json(json!({"artists": [{"score": 100}]}))
        } else {
            Json(json!({"release-groups": [{"score": "25"}]}))
        }
    }

    #[test]
    fn parser_accepts_musicbrainz_score_shapes_and_rejects_malformed_lists()
    -> Result<(), Box<dyn Error>> {
        assert_eq!(
            parse_top_score(br#"{"artists":[{"score":100}]}"#, "artists")?,
            100
        );
        assert_eq!(
            parse_top_score(br#"{"release-groups":[{"score":"25"}]}"#, "release-groups")?,
            25
        );
        assert_eq!(parse_top_score(br#"{}"#, "artists")?, 0);
        assert!(parse_top_score(br#"{"artists":{}}"#, "artists").is_err());
        assert!(parse_top_score(br#"{"artists":[{"score":101}]}"#, "artists").is_err());
        assert_eq!(lucene_quote(r#"AC/DC \ "Live""#), r#""AC/DC \\ \"Live\"""#);
        Ok(())
    }

    #[tokio::test]
    async fn client_identifies_paces_and_bounds_the_two_searches()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let router = Router::new()
            .route("/ws/2/artist", get(musicbrainz_fixture))
            .route("/ws/2/release-group", get(musicbrainz_fixture))
            .with_state(observed.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move { axum::serve(listener, router).await });
        let minimum_interval = Duration::from_millis(25);
        let lookup = MusicBrainzNameLookup::with_endpoint(
            format!("http://{address}/ws/2"),
            minimum_interval,
        )?;

        let scores = lookup.scores(r#"AC/DC "Live""#).await?;
        assert_eq!(scores.artist(), 100);
        assert_eq!(scores.album(), 25);
        let requests = observed.lock().await.clone();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].path, "/ws/2/artist");
        assert_eq!(requests[1].path, "/ws/2/release-group");
        assert_eq!(requests[0].user_agent, MUSICBRAINZ_USER_AGENT);
        assert_eq!(requests[1].user_agent, MUSICBRAINZ_USER_AGENT);
        assert!(
            requests[1]
                .started_at
                .duration_since(requests[0].started_at)
                >= minimum_interval
        );
        for request in requests {
            let url = reqwest::Url::parse(&format!("http://fixture/?{}", request.query))?;
            let parameters = url
                .query_pairs()
                .collect::<std::collections::BTreeMap<_, _>>();
            assert_eq!(
                parameters.get("fmt").map(|value| value.as_ref()),
                Some("json")
            );
            assert_eq!(
                parameters.get("limit").map(|value| value.as_ref()),
                Some("1")
            );
            assert!(parameters.get("query").is_some_and(|query| {
                query.starts_with("artist:\"") || query.starts_with("releasegroup:\"")
            }));
        }

        server.abort();
        let join = server.await;
        assert!(join.is_err_and(|error| error.is_cancelled()));
        Ok(())
    }
}
