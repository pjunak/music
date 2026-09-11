use crate::{
    error::{ApiError, HttpValidationErrorBody},
    http::HttpState,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use music_application::auth::SessionTouch;
use music_application::cleanup::rejections::{
    CleanupReviewProposal, REJECTION_PAGE_SIZE, RejectionError,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

pub(crate) fn router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default()
        .routes(routes!(list, reject))
        .routes(routes!(matching))
        .routes(routes!(restore))
        .routes(routes!(forget))
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = CleanupReviewProposal)]
struct Proposal {
    op_id: String,
    track_id: i64,
    path: String,
    kind: String,
    field: Option<String>,
    #[schema(schema_with = crate::cleanup::cleanup_value_schema)]
    old: Value,
    #[schema(schema_with = crate::cleanup::cleanup_value_schema)]
    new: Value,
    rules: Vec<String>,
    confidence: String,
    verified: bool,
    evidence: Option<Value>,
    evidence_context: Option<String>,
}

impl From<Proposal> for CleanupReviewProposal {
    fn from(p: Proposal) -> Self {
        Self {
            op_id: p.op_id,
            track_id: p.track_id,
            path: p.path,
            kind: p.kind,
            field: p.field,
            old: p.old,
            new: p.new,
            rules: p.rules,
            confidence: p.confidence,
            verified: p.verified,
            evidence: p.evidence,
            evidence_context: p.evidence_context,
        }
    }
}
impl From<CleanupReviewProposal> for Proposal {
    fn from(p: CleanupReviewProposal) -> Self {
        Self {
            op_id: p.op_id,
            track_id: p.track_id,
            path: p.path,
            kind: p.kind,
            field: p.field,
            old: p.old,
            new: p.new,
            rules: p.rules,
            confidence: p.confidence,
            verified: p.verified,
            evidence: p.evidence,
            evidence_context: p.evidence_context,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
struct PageQuery {
    before: Option<i64>,
    #[serde(default)]
    search: String,
}
#[derive(Serialize, ToSchema)]
#[schema(as = CleanupRejectedItem)]
struct Item {
    id: i64,
    proposal: Proposal,
    rejected_at: i64,
    current: bool,
}
#[derive(Serialize, ToSchema)]
#[schema(as = CleanupRejectedPage)]
struct Page {
    items: Vec<Item>,
    next_before: Option<i64>,
}

fn error(error: RejectionError) -> ApiError {
    match error {
        RejectionError::Invalid => ApiError::validation(),
        RejectionError::Stale | RejectionError::Missing => ApiError::coded_conflict(
            "cleanup_rejection_stale",
            "This suggestion has changed or is no longer available. Check the track again.",
        ),
        RejectionError::Dependency => ApiError::internal(),
    }
}

#[utoipa::path(get, path = "/library/cleanup/rejections", params(PageQuery), responses((status = 200, body = Page)), tag = "library-cleanup")]
async fn list(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let rows = crate::library::library(&state)?
        .cleanup
        .rejected_page(query.before, &query.search)
        .await
        .map_err(error)?;
    let next_before =
        (rows.len() > REJECTION_PAGE_SIZE).then(|| rows[REJECTION_PAGE_SIZE - 1].0.id);
    let items = rows
        .into_iter()
        .take(REJECTION_PAGE_SIZE)
        .map(|(record, current)| Item {
            id: record.id,
            proposal: record.proposal.into(),
            rejected_at: record.rejected_at,
            current,
        })
        .collect();
    Ok(Json(Page { items, next_before }))
}

#[utoipa::path(post, path = "/library/cleanup/rejections", request_body = Proposal, responses((status = 200, body = Item), (status = 422, body = HttpValidationErrorBody)), tag = "library-cleanup")]
async fn reject(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(proposal): Json<Proposal>,
) -> Result<Json<Item>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let record = crate::library::library(&state)?
        .cleanup
        .reject_proposal(proposal.into())
        .await
        .map_err(error)?;
    Ok(Json(Item {
        id: record.id,
        proposal: record.proposal.into(),
        rejected_at: record.rejected_at,
        current: true,
    }))
}

#[utoipa::path(post, path = "/library/cleanup/rejections/match", request_body = Vec<Proposal>, responses((status = 200, body = Vec<Option<i64>>)), tag = "library-cleanup")]
async fn matching(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(proposals): Json<Vec<Proposal>>,
) -> Result<Json<Vec<Option<i64>>>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let proposals = proposals.into_iter().map(Into::into).collect::<Vec<_>>();
    Ok(Json(
        crate::library::library(&state)?
            .cleanup
            .rejected_matches(&proposals)
            .await
            .map_err(error)?,
    ))
}

#[utoipa::path(post, path = "/library/cleanup/rejections/{id}/restore", params(("id" = i64, Path)), responses((status = 200, body = Proposal)), tag = "library-cleanup")]
async fn restore(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Proposal>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    Ok(Json(
        crate::library::library(&state)?
            .cleanup
            .restore_rejected(id)
            .await
            .map_err(error)?
            .into(),
    ))
}

#[utoipa::path(delete, path = "/library/cleanup/rejections/{id}", params(("id" = i64, Path)), responses((status = 204)), tag = "library-cleanup")]
async fn forget(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<axum::http::StatusCode, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    crate::library::library(&state)?
        .cleanup
        .forget_rejected(id)
        .await
        .map_err(error)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
