use crate::{
    error::{ApiError, HttpValidationErrorBody},
    http::HttpState,
    jobs::{BackgroundJobResponse, job_response, map_job_error},
};
use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use music_application::assistant::{LIBRARY_CLEANUP_DISCLOSURE, LIBRARY_CLEANUP_QUALITY_ID};
use music_application::auth::SessionTouch;
use music_application::cleanup_enrichment::ai::{CLEANUP_AI_JOB_KIND, CleanupAiParameters};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

pub(crate) fn router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default()
        .routes(routes!(status))
        .routes(routes!(start))
}

#[derive(Serialize, ToSchema)]
struct CleanupModelStatus {
    available: bool,
    reason_code: Option<String>,
    model_id: Option<String>,
    disclosure_version: &'static str,
    shared_with_provider: Vec<&'static str>,
    never_shared: Vec<&'static str>,
    maximum_candidates: u32,
    may_incur_cost: bool,
}

#[utoipa::path(get, path = "/library/cleanup/model", operation_id = "cleanup_model_status",
    responses((status = 200, body = CleanupModelStatus)), tag = "library")]
async fn status(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<CleanupModelStatus>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let execution = match &state.providers {
        Some(providers) => providers
            .quality_service()
            .prepare_quality_gated_role_execution("library_cleanup", LIBRARY_CLEANUP_QUALITY_ID)
            .await
            .map_err(|e| e.code().to_owned()),
        None => Err("provider_unavailable".into()),
    };
    Ok(Json(CleanupModelStatus {
        available: execution.is_ok(),
        reason_code: execution.as_ref().err().cloned(),
        model_id: execution.ok().map(|r| r.execution.model_id),
        disclosure_version: LIBRARY_CLEANUP_DISCLOSURE,
        shared_with_provider: vec![
            "This track's title, artist, album and duration",
            "Up to 25 catalog candidates with opaque IDs, titles, artists, album names and durations",
            "Locally computed comparison facts and evidence references",
            "For edition advice: up to five release titles/descriptions, folder comparison counts and eight distinguishing song titles per edition",
        ],
        never_shared: vec![
            "Audio and artwork",
            "Library paths, filenames and track IDs",
            "Credentials, raw webpages and unrelated tracks",
        ],
        maximum_candidates: 25,
        may_incur_cost: true,
    }))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct CleanupModelStart {
    #[serde(default)]
    edition_review: bool,
    track_id: i64,
    catalog_job_id: String,
    disclosure_version: String,
    consent: bool,
}

#[utoipa::path(post, path = "/library/cleanup/model/jobs", operation_id = "cleanup_model_start", request_body = CleanupModelStart,
    responses((status = 202, body = BackgroundJobResponse), (status = 422, body = HttpValidationErrorBody)), tag = "library")]
async fn start(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<CleanupModelStart>, JsonRejection>,
) -> Result<(StatusCode, Json<BackgroundJobResponse>), ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if !payload.consent
        || payload.disclosure_version != LIBRARY_CLEANUP_DISCLOSURE
        || payload.track_id <= 0
        || payload.catalog_job_id.is_empty()
        || payload.catalog_job_id.len() > 128
    {
        return Err(ApiError::validation());
    }
    let role = state
        .providers
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?
        .quality_service()
        .prepare_quality_gated_role_execution("library_cleanup", LIBRARY_CLEANUP_QUALITY_ID)
        .await
        .map_err(|e| {
            ApiError::coded_conflict(
                e.code(),
                "Configure and pass the Library cleanup model checks in AI setup before using it.",
            )
        })?;
    let parameters = json!(CleanupAiParameters {
        edition_review: payload.edition_review,
        track_id: payload.track_id,
        catalog_job_id: payload.catalog_job_id,
        consent: true,
        disclosure_version: payload.disclosure_version,
        role_fingerprint: role.fingerprint
    });
    let (job, created) = state
        .jobs
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?
        .enqueue_unique_active(CLEANUP_AI_JOB_KIND, parameters.clone())
        .await
        .map_err(map_job_error)?;
    if !created && json!(job.parameters) != parameters {
        return Err(ApiError::conflict(
            "Another library metadata review is running.",
        ));
    }
    Ok((StatusCode::ACCEPTED, Json(job_response(job)?)))
}
