//! Model-assisted tag cleanup: availability, start, and applying reviewed suggestions.

use std::collections::BTreeSet;

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use music_application::assistant::{
    AssistantServiceError, CleanupSelection, MAX_MODEL_CLEANUP_TAGS, MODEL_TAG_CLEANUP_BATCH_SIZE,
    MODEL_TAG_CLEANUP_ENGINE_ID, ModelTagCleanupTask, catalog_signature,
};
use music_application::jobs::JobStatus;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;

use crate::auth::Operator;
use crate::error::{ApiError, HttpValidationErrorBody};
use crate::http::HttpState;
use crate::jobs::{BackgroundJobResponse, map_job_error};
use crate::model_jobs::MODEL_TAG_CLEANUP_JOB_KIND;
use crate::provider_api::map_model_quality_error;

use super::schema::{
    model_tag_cleanup_disclosure_version_schema, model_tag_cleanup_maximum_tags_schema,
    model_tag_cleanup_quality_schema, model_tag_cleanup_request_count_schema,
    model_tag_cleanup_role_schema, nonnegative_integer_schema, nullable_string_schema, true_schema,
};
use super::shared::{
    enqueue_model_feature, map_assistant_error, model_role_availability, service, valid_sha256,
};
use super::tags::{
    ManualTagRenameResponse, TAG_CLEANUP_APPLY_SCHEMA, TagCleanupApplyResponse,
    TagCleanupSelectionRequest,
};

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelTagCleanupDisclosure)]
pub(super) struct ModelTagCleanupDisclosureResponse {
    #[schema(schema_with = model_tag_cleanup_disclosure_version_schema)]
    pub(super) version: &'static str,
    pub(super) shared_with_provider: Vec<&'static str>,
    pub(super) never_shared: Vec<&'static str>,
    #[schema(schema_with = model_tag_cleanup_maximum_tags_schema)]
    pub(super) maximum_tags: usize,
    pub(super) may_incur_cost: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelTagCleanupAvailability)]
pub(super) struct ModelTagCleanupAvailabilityResponse {
    pub(super) available: bool,
    #[schema(required = true, schema_with = nullable_string_schema)]
    pub(super) reason_code: Option<String>,
    #[schema(schema_with = model_tag_cleanup_role_schema)]
    pub(super) role_id: &'static str,
    #[schema(required = true, schema_with = nullable_string_schema)]
    pub(super) connection_name: Option<String>,
    #[schema(required = true, schema_with = nullable_string_schema)]
    pub(super) model_id: Option<String>,
    #[schema(schema_with = model_tag_cleanup_quality_schema)]
    pub(super) quality_evaluation_id: &'static str,
    pub(super) job_kind: &'static str,
    #[schema(pattern = "^[a-f0-9]{64}$")]
    pub(super) catalog_signature: String,
    #[schema(pattern = "^[a-f0-9]{64}$")]
    pub(super) vocabulary_fingerprint: String,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) manual_tags: usize,
    #[schema(schema_with = model_tag_cleanup_request_count_schema)]
    pub(super) estimated_provider_requests: usize,
    pub(super) disclosure: ModelTagCleanupDisclosureResponse,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelTagCleanupStartRequest)]
pub(super) struct ModelTagCleanupStartRequest {
    #[schema(schema_with = model_tag_cleanup_disclosure_version_schema)]
    pub(super) disclosure_version: String,
    #[schema(schema_with = true_schema)]
    pub(super) consent: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelTagCleanupApplyRequest)]
pub(super) struct ModelTagCleanupApplyRequest {
    #[schema(pattern = "^[a-f0-9]{64}$")]
    pub(super) catalog_signature: String,
    #[schema(pattern = "^[a-f0-9]{64}$")]
    pub(super) vocabulary_fingerprint: String,
    #[schema(min_items = 1, max_items = 100)]
    pub(super) items: Vec<TagCleanupSelectionRequest>,
    #[schema(min_length = 1, max_length = 32)]
    pub(super) job_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredModelTagCleanupResult {
    pub(super) schema_version: String,
    pub(super) disclosure_version: String,
    pub(super) role_id: String,
    pub(super) role_fingerprint: String,
    pub(super) engine_id: String,
    pub(super) catalog_signature: String,
    pub(super) vocabulary_fingerprint: String,
    pub(super) catalog_tags: usize,
    pub(super) suggestions: Vec<StoredModelTagCleanupSuggestion>,
    pub(super) usage: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredModelTagCleanupSuggestion {
    pub(super) id: String,
    pub(super) source: String,
    pub(super) target: String,
    pub(super) origin: String,
    pub(super) confidence: String,
    pub(super) reason: String,
    pub(super) source_track_count: u64,
    pub(super) target_track_count: u64,
    pub(super) merged: bool,
}

#[utoipa::path(
    get,
    path = "/assistant/library-tags/catalog/model-cleanup-status",
    operation_id = "model_library_tag_cleanup_status_api_assistant_library_tags_catalog_model_cleanup_status_get",
    responses((status = 200, description = "Successful Response", body = ModelTagCleanupAvailabilityResponse)),
    tag = "assistant-tags"
)]
pub(super) async fn model_tag_cleanup_status(
    State(state): State<HttpState>,
    _operator: Operator,
) -> Result<Json<ModelTagCleanupAvailabilityResponse>, ApiError> {
    let role = model_role_availability(&state, "tag_cleanup", "tag-cleanup-quality-v1").await?;
    let assistant = service(&state)?;
    let usage = assistant.tag_usage().await.map_err(map_assistant_error)?;
    let vocabulary = assistant.vocabulary().await.map_err(map_assistant_error)?;
    let signature = catalog_signature(&usage)
        .map_err(|error| ApiError::internal_from("tag catalog signature failed", &error))?;
    let mut reason_code = role.reason_code;
    if reason_code.is_none() && usage.is_empty() {
        reason_code = Some("tag_catalog_empty".to_owned());
    }
    if reason_code.is_none() && usage.len() > MAX_MODEL_CLEANUP_TAGS {
        reason_code = Some("tag_catalog_too_large".to_owned());
    }
    let estimated_provider_requests = if usage.len() <= MAX_MODEL_CLEANUP_TAGS {
        ModelTagCleanupTask::new(&usage, vocabulary.clone())
            .map_err(|error| {
                ApiError::internal_from("tag cleanup task could not be planned", &error)
            })?
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
pub(super) async fn start_model_tag_cleanup(
    State(state): State<HttpState>,
    _operator: Operator,
    payload: Result<Json<ModelTagCleanupStartRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<BackgroundJobResponse>), ApiError> {
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
        "catalog_signature": catalog_signature(&usage).map_err(|error| ApiError::internal_from("tag catalog signature failed", &error))?,
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
pub(super) async fn apply_model_tag_cleanup(
    State(state): State<HttpState>,
    _operator: Operator,
    payload: Result<Json<ModelTagCleanupApplyRequest>, JsonRejection>,
) -> Result<Json<TagCleanupApplyResponse>, ApiError> {
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

pub(super) fn model_tag_cleanup_disclosure() -> ModelTagCleanupDisclosureResponse {
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

pub(super) fn valid_stored_model_tag_cleanup_result(result: &StoredModelTagCleanupResult) -> bool {
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

pub(super) fn map_model_cleanup_apply_error(error: AssistantServiceError) -> ApiError {
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
