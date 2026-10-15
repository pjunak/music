//! Tag vocabulary, manual tags and local tag cleanup.

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use music_application::assistant::{
    CleanupSelection, TagVocabularyDocument, TagVocabularyEntry, TagVocabularyGroup,
    TagVocabularySnapshot,
};
use music_domain::TrackId;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::Operator;
use crate::error::{ApiError, HttpValidationErrorBody, openapi_integer};
use crate::http::HttpState;

use super::review::{LibraryTagTrackResponse, track_ids, track_response};
use super::schema::{
    cleanup_apply_version_schema, cleanup_preview_version_schema, cleanup_reason_schema,
    integer_array_schema, nonnegative_integer_schema, positive_integer_schema,
    required_track_ids_schema, vocabulary_version_schema,
};
use super::shared::{map_assistant_error, service};

pub(super) const TAG_VOCABULARY_SCHEMA: &str = "assistant-tag-vocabulary/v1";
pub(super) const TAG_CLEANUP_PREVIEW_SCHEMA: &str = "assistant-tag-cleanup-preview/v2";
pub(super) const TAG_CLEANUP_APPLY_SCHEMA: &str = "assistant-tag-cleanup-apply/v1";

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagVocabularyEntry)]
pub(super) struct TagVocabularyEntryWire {
    #[schema(required = true, pattern = "^[a-z0-9][a-z0-9._-]{1,63}$")]
    pub(super) id: String,
    #[schema(required = true, min_length = 1, max_length = 64)]
    pub(super) name: String,
    #[schema(required = true, min_length = 2, max_length = 300)]
    pub(super) description: String,
    #[serde(default)]
    #[schema(required = true, max_items = 24)]
    pub(super) aliases: Vec<String>,
    #[serde(default)]
    #[schema(required = true, max_items = 32)]
    pub(super) context_cues: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagVocabularyGroup)]
pub(super) struct TagVocabularyGroupWire {
    #[schema(required = true, pattern = "^[a-z0-9][a-z0-9_-]{1,31}$")]
    pub(super) key: String,
    #[schema(required = true, min_length = 1, max_length = 64)]
    pub(super) label: String,
    #[serde(default)]
    #[schema(required = true, max_length = 300, default = "")]
    pub(super) description: String,
    #[serde(default)]
    #[schema(required = true, max_items = 100)]
    pub(super) tags: Vec<TagVocabularyEntryWire>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagVocabularyUpdateRequest)]
pub(super) struct TagVocabularyUpdateRequest {
    #[schema(schema_with = vocabulary_version_schema)]
    pub(super) schema_version: String,
    #[schema(schema_with = positive_integer_schema)]
    pub(super) expected_revision: u32,
    #[schema(min_items = 1, max_items = 20)]
    pub(super) groups: Vec<TagVocabularyGroupWire>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagVocabularyOut)]
pub(super) struct TagVocabularyResponse {
    #[schema(schema_with = vocabulary_version_schema)]
    pub(super) schema_version: &'static str,
    #[schema(schema_with = positive_integer_schema)]
    pub(super) revision: u32,
    #[schema(pattern = "^[a-f0-9]{64}$")]
    pub(super) fingerprint: String,
    pub(super) groups: Vec<TagVocabularyGroupWire>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = StarterTagGroupOut)]
pub(super) struct StarterTagGroupResponse {
    pub(super) key: String,
    pub(super) label: String,
    pub(super) tags: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ManualTagUsage)]
pub(super) struct ManualTagUsageResponse {
    pub(super) tag: String,
    #[schema(schema_with = positive_integer_schema)]
    pub(super) track_count: u64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ManualTagCatalog)]
pub(super) struct ManualTagCatalogResponse {
    pub(super) starter_groups: Vec<StarterTagGroupResponse>,
    pub(super) used_tags: Vec<String>,
    pub(super) tag_usage: Vec<ManualTagUsageResponse>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ManualTagPatch)]
pub(super) struct ManualTagPatchRequest {
    #[serde(default)]
    #[schema(required = false)]
    pub(super) add: Vec<String>,
    #[serde(default)]
    #[schema(required = false)]
    pub(super) remove: Vec<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = BulkManualTagPatch)]
pub(super) struct BulkManualTagPatchRequest {
    #[serde(default)]
    #[schema(required = false)]
    pub(super) add: Vec<String>,
    #[serde(default)]
    #[schema(required = false)]
    pub(super) remove: Vec<String>,
    #[schema(schema_with = required_track_ids_schema)]
    pub(super) track_ids: Vec<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = BulkManualTagFailure)]
pub(super) struct BulkManualTagFailureResponse {
    #[schema(schema_with = openapi_integer)]
    pub(super) track_id: i64,
    pub(super) error: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = BulkManualTagResult)]
pub(super) struct BulkManualTagResultResponse {
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) requested_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) matched_tracks: usize,
    #[schema(schema_with = integer_array_schema)]
    pub(super) changed_track_ids: Vec<i64>,
    #[schema(schema_with = integer_array_schema)]
    pub(super) missing_track_ids: Vec<i64>,
    pub(super) failures: Vec<BulkManualTagFailureResponse>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ManualTagRenameRequest)]
pub(super) struct ManualTagRenameRequest {
    pub(super) source: String,
    pub(super) target: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ManualTagRenameResult)]
pub(super) struct ManualTagRenameResponse {
    pub(super) source: String,
    pub(super) target: String,
    #[schema(schema_with = positive_integer_schema)]
    pub(super) affected_tracks: usize,
    pub(super) merged: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagCleanupSuggestionOut)]
pub(super) struct TagCleanupSuggestionResponse {
    #[schema(pattern = "^[a-f0-9]{64}$")]
    pub(super) id: String,
    pub(super) source: String,
    pub(super) target: String,
    #[schema(schema_with = cleanup_reason_schema)]
    pub(super) reason_code: String,
    pub(super) reason: String,
    #[schema(schema_with = positive_integer_schema)]
    pub(super) source_track_count: u64,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) target_track_count: u64,
    pub(super) merged: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagCleanupPreviewOut)]
pub(super) struct TagCleanupPreviewResponse {
    #[schema(schema_with = cleanup_preview_version_schema)]
    pub(super) schema_version: &'static str,
    #[schema(pattern = "^[a-f0-9]{64}$")]
    pub(super) catalog_signature: String,
    #[schema(pattern = "^[a-f0-9]{64}$")]
    pub(super) vocabulary_fingerprint: String,
    pub(super) suggestions: Vec<TagCleanupSuggestionResponse>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagCleanupSelectionIn)]
pub(super) struct TagCleanupSelectionRequest {
    pub(super) source: String,
    pub(super) target: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagCleanupApplyRequest)]
pub(super) struct TagCleanupApplyRequest {
    #[schema(pattern = "^[a-f0-9]{64}$")]
    pub(super) catalog_signature: String,
    #[schema(pattern = "^[a-f0-9]{64}$")]
    pub(super) vocabulary_fingerprint: String,
    #[schema(min_items = 1, max_items = 100)]
    pub(super) items: Vec<TagCleanupSelectionRequest>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagCleanupApplyResult)]
pub(super) struct TagCleanupApplyResponse {
    #[schema(schema_with = cleanup_apply_version_schema)]
    pub(super) schema_version: &'static str,
    #[schema(schema_with = positive_integer_schema)]
    pub(super) requested_items: usize,
    pub(super) applied: Vec<ManualTagRenameResponse>,
    #[schema(pattern = "^[a-f0-9]{64}$")]
    pub(super) catalog_signature: String,
}

#[utoipa::path(
    get,
    path = "/assistant/library-tags/catalog",
    operation_id = "tag_catalog_api_assistant_library_tags_catalog_get",
    responses((status = 200, description = "Successful Response", body = ManualTagCatalogResponse)),
    tag = "assistant-tags"
)]
pub(super) async fn tag_catalog(
    State(state): State<HttpState>,
    _operator: Operator,
) -> Result<Json<ManualTagCatalogResponse>, ApiError> {
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
pub(super) async fn get_tag_vocabulary(
    State(state): State<HttpState>,
    _operator: Operator,
) -> Result<Json<TagVocabularyResponse>, ApiError> {
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
pub(super) async fn update_tag_vocabulary(
    State(state): State<HttpState>,
    _operator: Operator,
    payload: Result<Json<TagVocabularyUpdateRequest>, JsonRejection>,
) -> Result<Json<TagVocabularyResponse>, ApiError> {
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
pub(super) async fn preview_tag_cleanup(
    State(state): State<HttpState>,
    _operator: Operator,
) -> Result<Json<TagCleanupPreviewResponse>, ApiError> {
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
pub(super) async fn apply_tag_cleanup(
    State(state): State<HttpState>,
    _operator: Operator,
    payload: Result<Json<TagCleanupApplyRequest>, JsonRejection>,
) -> Result<Json<TagCleanupApplyResponse>, ApiError> {
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
pub(super) async fn rename_tag(
    State(state): State<HttpState>,
    _operator: Operator,
    payload: Result<Json<ManualTagRenameRequest>, JsonRejection>,
) -> Result<Json<ManualTagRenameResponse>, ApiError> {
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
pub(super) async fn patch_tags_bulk(
    State(state): State<HttpState>,
    _operator: Operator,
    payload: Result<Json<BulkManualTagPatchRequest>, JsonRejection>,
) -> Result<Json<BulkManualTagResultResponse>, ApiError> {
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
pub(super) async fn patch_track_tags(
    State(state): State<HttpState>,
    _operator: Operator,
    track_id: Result<Path<i128>, PathRejection>,
    payload: Result<Json<ManualTagPatchRequest>, JsonRejection>,
) -> Result<Json<LibraryTagTrackResponse>, ApiError> {
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

pub(super) fn vocabulary_response(snapshot: TagVocabularySnapshot) -> TagVocabularyResponse {
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

pub(super) fn rename_response(
    value: music_application::assistant::RenameTagOutcome,
) -> ManualTagRenameResponse {
    ManualTagRenameResponse {
        source: value.source,
        target: value.target,
        affected_tracks: value.affected_tracks,
        merged: value.merged,
    }
}
