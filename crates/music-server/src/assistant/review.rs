//! Library tag listing and review of analysis tag suggestions.

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use music_application::assistant::{
    AnalysisReviewDecision, AnalysisReviewTarget, AssistantTrackView, CATALOG_TAG_ANALYZER_ID,
    MODEL_TAG_ANALYZER_ID, ManualTagQuery, ModelAnalysisStatus, ModelTagFilter, TagReviewSummary,
};
use music_domain::TrackId;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::auth::Operator;
use crate::error::{ApiError, HttpValidationErrorBody, openapi_integer};
use crate::http::HttpState;

use super::schema::{
    bulk_review_decision_schema, model_analysis_status_schema, model_tag_filter_schema,
    nonnegative_integer_schema, nullable_review_status_schema, nullable_string_schema,
    nullable_tag_schema, positive_exclusive_integer_schema, positive_integer_schema,
    review_failure_code_schema, review_status_schema, suggestion_source_schema, support_schema,
};
use super::shared::{
    ContextScopeKindWire, ModelTaggingScopeWire, ReviewStatusWire, default_page_limit,
    map_assistant_error, service,
};

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = AnalysisTagSuggestionOut)]
pub(super) struct AnalysisTagSuggestionResponse {
    pub(super) tag: String,
    pub(super) analyzer_id: String,
    pub(super) source_signature: String,
    #[schema(schema_with = support_schema)]
    pub(super) support: String,
    pub(super) evidence_ids: Vec<String>,
    pub(super) contradiction_ids: Vec<String>,
    pub(super) evidence: Vec<String>,
    #[schema(schema_with = review_status_schema)]
    pub(super) status: ReviewStatusWire,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = LibraryTagTrack)]
pub(super) struct LibraryTagTrackResponse {
    #[schema(schema_with = openapi_integer)]
    pub(super) track_id: i64,
    pub(super) path: String,
    pub(super) title: String,
    pub(super) display_title: String,
    pub(super) artist: String,
    pub(super) album: String,
    pub(super) manual_tags: Vec<String>,
    pub(super) analysis_tags: Vec<String>,
    pub(super) analysis_suggestions: Vec<AnalysisTagSuggestionResponse>,
    #[schema(schema_with = model_analysis_status_schema)]
    pub(super) model_analysis: ModelAnalysisStatus,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = LibraryTagPage)]
pub(super) struct LibraryTagPageResponse {
    pub(super) items: Vec<LibraryTagTrackResponse>,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) total: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) offset: usize,
    #[schema(schema_with = positive_integer_schema)]
    pub(super) limit: usize,
    pub(super) review_summary: TagReviewSummaryResponse,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagReviewSource)]
pub(super) struct TagReviewSourceResponse {
    pub(super) analyzer_id: String,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) pending: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) accepted: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) rejected: usize,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TagReviewSummary)]
pub(super) struct TagReviewSummaryResponse {
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) matching_tracks: usize,
    pub(super) sources: Vec<TagReviewSourceResponse>,
}

impl From<TagReviewSummary> for TagReviewSummaryResponse {
    fn from(summary: TagReviewSummary) -> Self {
        Self {
            matching_tracks: summary.matching_tracks,
            sources: summary
                .sources
                .into_iter()
                .map(|(analyzer_id, counts)| TagReviewSourceResponse {
                    analyzer_id,
                    pending: counts.pending,
                    accepted: counts.accepted,
                    rejected: counts.rejected,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct LibraryTagListQuery {
    #[param(schema_with = model_tag_filter_schema)]
    pub(super) model_status: Option<String>,
    #[param(max_length = 128)]
    pub(super) model_job_id: Option<String>,
    #[param(schema_with = suggestion_source_schema)]
    pub(super) suggestion_source: Option<String>,
    #[serde(default)]
    #[param(max_length = 128, default = "")]
    pub(super) search: String,
    #[param(max_length = 64, schema_with = nullable_tag_schema)]
    pub(super) tag: Option<String>,
    #[param(schema_with = nullable_review_status_schema)]
    pub(super) review: Option<ReviewStatusWire>,
    #[param(max_length = 1024, schema_with = nullable_string_schema)]
    pub(super) folder: Option<String>,
    #[serde(default)]
    #[param(default = false)]
    pub(super) recursive: bool,
    #[serde(default)]
    #[param(minimum = 0, default = 0)]
    pub(super) offset: usize,
    #[serde(default = "default_page_limit")]
    #[param(minimum = 1, maximum = 100, default = 50)]
    pub(super) limit: usize,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = AnalysisTagReviewRequest)]
pub(super) struct AnalysisTagReviewRequest {
    pub(super) tag: String,
    #[schema(min_length = 1, max_length = 128)]
    pub(super) analyzer_id: String,
    #[schema(min_length = 1, max_length = 128)]
    pub(super) source_signature: String,
    #[schema(schema_with = review_status_schema)]
    pub(super) decision: ReviewStatusWire,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = AnalysisTagReviewResult)]
pub(super) struct AnalysisTagReviewResponse {
    #[schema(schema_with = openapi_integer)]
    pub(super) track_id: i64,
    pub(super) tag: String,
    pub(super) analyzer_id: String,
    pub(super) source_signature: String,
    #[schema(schema_with = review_status_schema)]
    pub(super) decision: ReviewStatusWire,
    pub(super) manual_tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = BulkAnalysisTagReviewItem)]
pub(super) struct BulkAnalysisTagReviewItemRequest {
    pub(super) tag: String,
    #[schema(min_length = 1, max_length = 128)]
    pub(super) analyzer_id: String,
    #[schema(min_length = 1, max_length = 128)]
    pub(super) source_signature: String,
    #[schema(schema_with = positive_exclusive_integer_schema)]
    pub(super) track_id: i64,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(super) enum BulkReviewDecisionWire {
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
pub(super) struct BulkAnalysisTagReviewRequest {
    #[schema(min_items = 1, max_items = 1000)]
    pub(super) items: Vec<BulkAnalysisTagReviewItemRequest>,
    #[schema(schema_with = bulk_review_decision_schema)]
    pub(super) decision: BulkReviewDecisionWire,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = BulkAnalysisTagReviewApplied)]
pub(super) struct BulkAnalysisTagReviewAppliedResponse {
    #[schema(schema_with = openapi_integer)]
    pub(super) track_id: i64,
    pub(super) tag: String,
    pub(super) analyzer_id: String,
    pub(super) source_signature: String,
    #[schema(schema_with = bulk_review_decision_schema)]
    pub(super) decision: BulkReviewDecisionWire,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = BulkAnalysisTagReviewFailure)]
pub(super) struct BulkAnalysisTagReviewFailureResponse {
    #[schema(schema_with = openapi_integer)]
    pub(super) track_id: i64,
    pub(super) tag: String,
    pub(super) analyzer_id: String,
    pub(super) source_signature: String,
    #[schema(schema_with = review_failure_code_schema)]
    pub(super) code: String,
    pub(super) error: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = BulkAnalysisTagReviewResult)]
pub(super) struct BulkAnalysisTagReviewResponse {
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) requested_items: usize,
    pub(super) applied: Vec<BulkAnalysisTagReviewAppliedResponse>,
    pub(super) failures: Vec<BulkAnalysisTagReviewFailureResponse>,
}

#[utoipa::path(
    post,
    path = "/assistant/library-tags/analysis-tags/reviews/bulk",
    request_body = BulkAnalysisTagReviewRequest,
    responses(
        (status = 200, description = "Successful Response", body = BulkAnalysisTagReviewResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant-tags"
)]
pub(super) async fn review_analysis_tags_bulk(
    State(state): State<HttpState>,
    _operator: Operator,
    payload: Result<Json<BulkAnalysisTagReviewRequest>, JsonRejection>,
) -> Result<Json<BulkAnalysisTagReviewResponse>, ApiError> {
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
    params(LibraryTagListQuery),
    responses(
        (status = 200, description = "Successful Response", body = LibraryTagPageResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant-tags"
)]
pub(super) async fn list_library_tags(
    State(state): State<HttpState>,
    _operator: Operator,
    query: Result<Query<LibraryTagListQuery>, QueryRejection>,
) -> Result<Json<LibraryTagPageResponse>, ApiError> {
    let Query(query) = query.map_err(|_| ApiError::validation())?;
    let scope = query
        .folder
        .map(|path| ModelTaggingScopeWire {
            kind: ContextScopeKindWire::Folder,
            path,
            recursive: query.recursive,
            track_ids: Vec::new(),
        })
        .map(ModelTaggingScopeWire::into_parameters)
        .transpose()?
        .map(|parameters| parameters.to_scope().map_err(|()| ApiError::validation()))
        .transpose()?;
    let page = service(&state)?
        .tag_page(ManualTagQuery {
            model_status: query
                .model_status
                .as_deref()
                .map(|value| ModelTagFilter::parse(value).ok_or_else(ApiError::validation))
                .transpose()?,
            model_job_id: query.model_job_id,
            search: query.search,
            tag: query.tag,
            review: query.review.map(Into::into),
            offset: query.offset,
            limit: query.limit,
            analyzer_ids: query
                .suggestion_source
                .as_deref()
                .map(|value| match value {
                    "model" => Ok(vec![MODEL_TAG_ANALYZER_ID.to_owned()]),
                    "catalog" => Ok(vec![CATALOG_TAG_ANALYZER_ID.to_owned()]),
                    _ => Err(ApiError::validation()),
                })
                .transpose()?,
            scope,
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

#[utoipa::path(
    put,
    path = "/assistant/library-tags/{track_id}/analysis-tags/review",
    params(("track_id" = i128, Path)),
    request_body = AnalysisTagReviewRequest,
    responses(
        (status = 200, description = "Successful Response", body = AnalysisTagReviewResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant-tags"
)]
pub(super) async fn review_analysis_tag(
    State(state): State<HttpState>,
    _operator: Operator,
    track_id: Result<Path<i128>, PathRejection>,
    payload: Result<Json<AnalysisTagReviewRequest>, JsonRejection>,
) -> Result<Json<AnalysisTagReviewResponse>, ApiError> {
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

pub(super) fn track_response(value: AssistantTrackView) -> LibraryTagTrackResponse {
    LibraryTagTrackResponse {
        track_id: value.track.id.get(),
        path: value.track.path.as_str().to_owned(),
        title: value.track.metadata.title,
        display_title: value.track.display_title,
        artist: value.track.metadata.artist,
        album: value.track.metadata.album,
        manual_tags: value.manual_tags,
        model_analysis: value.model_analysis,
        analysis_tags: value.analysis_tags,
        analysis_suggestions: value
            .analysis_suggestions
            .into_iter()
            .map(|item| AnalysisTagSuggestionResponse {
                tag: item.tag,
                analyzer_id: item.analyzer_id,
                source_signature: item.source_signature,
                support: item.support.as_str().to_owned(),
                evidence_ids: item.evidence_ids,
                contradiction_ids: item.contradiction_ids,
                evidence: item.evidence,
                status: item.status.into(),
            })
            .collect(),
    }
}

pub(super) fn review_target(
    value: BulkAnalysisTagReviewItemRequest,
) -> Result<AnalysisReviewTarget, ApiError> {
    Ok(AnalysisReviewTarget {
        track_id: TrackId::new(value.track_id).map_err(|_| ApiError::validation())?,
        tag: value.tag,
        analyzer_id: value.analyzer_id,
        source_signature: value.source_signature,
    })
}

pub(super) fn track_ids(values: &[i64]) -> Result<Vec<TrackId>, ApiError> {
    values
        .iter()
        .map(|value| TrackId::new(*value).map_err(|_| ApiError::validation()))
        .collect()
}
