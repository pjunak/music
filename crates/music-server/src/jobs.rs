use axum::Json;
use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use music_application::auth::{SessionTouch, UnixSeconds};
use music_application::jobs::{JobListFilter, JobRecord, JobService, JobServiceError, JobStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{AdditionalProperties, AnyOfBuilder, ObjectBuilder, Schema, Type};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::{current_session, format_rfc3339};
use crate::error::{
    ApiError, HttpValidationErrorBody, openapi_datetime, openapi_nullable_datetime,
    openapi_nullable_string,
};
use crate::http::HttpState;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum JobStatusResponse {
    Queued,
    Running,
    CancelRequested,
    Succeeded,
    Failed,
    Cancelled,
}

impl From<JobStatus> for JobStatusResponse {
    fn from(value: JobStatus) -> Self {
        match value {
            JobStatus::Queued => Self::Queued,
            JobStatus::Running => Self::Running,
            JobStatus::CancelRequested => Self::CancelRequested,
            JobStatus::Succeeded => Self::Succeeded,
            JobStatus::Failed => Self::Failed,
            JobStatus::Cancelled => Self::Cancelled,
        }
    }
}

impl From<JobStatusResponse> for JobStatus {
    fn from(value: JobStatusResponse) -> Self {
        match value {
            JobStatusResponse::Queued => Self::Queued,
            JobStatusResponse::Running => Self::Running,
            JobStatusResponse::CancelRequested => Self::CancelRequested,
            JobStatusResponse::Succeeded => Self::Succeeded,
            JobStatusResponse::Failed => Self::Failed,
            JobStatusResponse::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = BackgroundJobOut)]
struct BackgroundJobResponse {
    id: String,
    kind: String,
    #[schema(schema_with = job_status_schema)]
    status: JobStatusResponse,
    #[schema(schema_with = json_object_schema)]
    parameters: Map<String, Value>,
    #[schema(required = true, schema_with = nullable_json_object_schema)]
    result: Option<Map<String, Value>>,
    #[schema(required = true, schema_with = openapi_nullable_string)]
    error: Option<String>,
    #[schema(schema_with = nonnegative_integer_schema)]
    progress_current: u64,
    #[schema(required = false, schema_with = nullable_nonnegative_integer_schema)]
    progress_total: Option<u64>,
    progress_phase: String,
    progress_message: String,
    #[schema(schema_with = nonnegative_integer_schema)]
    attempts: u32,
    #[schema(required = true, schema_with = openapi_nullable_string)]
    retry_of_id: Option<String>,
    #[schema(schema_with = openapi_datetime)]
    created_at: String,
    #[schema(schema_with = openapi_datetime)]
    updated_at: String,
    #[schema(required = true, schema_with = openapi_nullable_datetime)]
    started_at: Option<String>,
    #[schema(required = true, schema_with = openapi_nullable_datetime)]
    finished_at: Option<String>,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct JobListQuery {
    #[param(max_length = 128, schema_with = nullable_kind_schema)]
    kind: Option<String>,
    #[serde(rename = "status")]
    #[param(rename = "status", schema_with = nullable_job_status_schema)]
    job_status: Option<JobStatusResponse>,
    #[serde(default = "default_limit")]
    #[param(schema_with = limit_schema)]
    limit: u16,
}

pub(crate) fn jobs_router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default()
        .routes(routes!(list_jobs))
        .routes(routes!(get_job))
        .routes(routes!(cancel_job))
        .routes(routes!(retry_job))
}

#[utoipa::path(
    get,
    path = "/jobs",
    operation_id = "list_jobs_api_jobs_get",
    params(JobListQuery),
    responses(
        (status = 200, description = "Successful Response", body = [BackgroundJobResponse]),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "jobs"
)]
async fn list_jobs(
    State(state): State<HttpState>,
    headers: HeaderMap,
    query: Result<Query<JobListQuery>, QueryRejection>,
) -> Result<Json<Vec<BackgroundJobResponse>>, ApiError> {
    authorize(&state, &headers).await?;
    let Query(query) = query.map_err(|_| ApiError::validation())?;
    if query.kind.as_ref().is_some_and(|kind| kind.len() > 128) || !(1..=100).contains(&query.limit)
    {
        return Err(ApiError::validation());
    }
    let jobs = service(&state)?
        .list(&JobListFilter {
            kind: query.kind,
            status: query.job_status.map(Into::into),
            limit: query.limit,
        })
        .await
        .map_err(map_job_error)?;
    Ok(Json(
        jobs.into_iter()
            .map(job_response)
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

#[utoipa::path(
    get,
    path = "/jobs/{job_id}",
    operation_id = "get_job_api_jobs__job_id__get",
    params(("job_id" = String, Path)),
    responses(
        (status = 200, description = "Successful Response", body = BackgroundJobResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "jobs"
)]
async fn get_job(
    State(state): State<HttpState>,
    headers: HeaderMap,
    job_id: Result<Path<String>, PathRejection>,
) -> Result<Json<BackgroundJobResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Path(job_id) = job_id.map_err(|_| ApiError::validation())?;
    let job = service(&state)?
        .get(&job_id)
        .await
        .map_err(map_job_error)?
        .ok_or_else(|| ApiError::plain_not_found("job not found"))?;
    Ok(Json(job_response(job)?))
}

#[utoipa::path(
    post,
    path = "/jobs/{job_id}/cancel",
    operation_id = "cancel_job_api_jobs__job_id__cancel_post",
    params(("job_id" = String, Path)),
    responses(
        (status = 200, description = "Successful Response", body = BackgroundJobResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "jobs"
)]
async fn cancel_job(
    State(state): State<HttpState>,
    headers: HeaderMap,
    job_id: Result<Path<String>, PathRejection>,
) -> Result<Json<BackgroundJobResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Path(job_id) = job_id.map_err(|_| ApiError::validation())?;
    let job = service(&state)?
        .cancel(&job_id)
        .await
        .map_err(map_job_error)?;
    Ok(Json(job_response(job)?))
}

#[utoipa::path(
    post,
    path = "/jobs/{job_id}/retry",
    operation_id = "retry_job_api_jobs__job_id__retry_post",
    params(("job_id" = String, Path)),
    responses(
        (status = 200, description = "Successful Response", body = BackgroundJobResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "jobs"
)]
async fn retry_job(
    State(state): State<HttpState>,
    headers: HeaderMap,
    job_id: Result<Path<String>, PathRejection>,
) -> Result<Json<BackgroundJobResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Path(job_id) = job_id.map_err(|_| ApiError::validation())?;
    let job = service(&state)?
        .retry(&job_id)
        .await
        .map_err(map_job_error)?;
    Ok(Json(job_response(job)?))
}

async fn authorize(state: &HttpState, headers: &HeaderMap) -> Result<(), ApiError> {
    current_session(state, headers, SessionTouch::UpdateLastSeen)
        .await
        .map(|_| ())
}

fn service(state: &HttpState) -> Result<&JobService, ApiError> {
    state
        .jobs
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)
}

fn map_job_error(error: JobServiceError) -> ApiError {
    match error {
        JobServiceError::JobNotFound => ApiError::plain_not_found("job not found"),
        JobServiceError::NotRetryable => {
            ApiError::conflict("only failed or cancelled jobs can be retried")
        }
        JobServiceError::UnknownKind => ApiError::conflict("the job type is no longer available"),
        JobServiceError::AlreadyTerminal(status) => {
            ApiError::conflict_message(format!("job is already {}", status.as_str()))
        }
        JobServiceError::InvalidParameters => ApiError::validation(),
        JobServiceError::Dependency => ApiError::internal(),
    }
}

fn job_response(job: JobRecord) -> Result<BackgroundJobResponse, ApiError> {
    Ok(BackgroundJobResponse {
        id: job.id,
        kind: job.kind,
        status: job.status.into(),
        parameters: job.parameters,
        result: job.result,
        error: job.error,
        progress_current: job.progress_current,
        progress_total: job.progress_total,
        progress_phase: job.progress_phase,
        progress_message: job.progress_message,
        attempts: job.attempts,
        retry_of_id: job.retry_of_id,
        created_at: format_rfc3339(UnixSeconds::new(job.created_at_unix_seconds))?,
        updated_at: format_rfc3339(UnixSeconds::new(job.updated_at_unix_seconds))?,
        started_at: job
            .started_at_unix_seconds
            .map(UnixSeconds::new)
            .map(format_rfc3339)
            .transpose()?,
        finished_at: job
            .finished_at_unix_seconds
            .map(UnixSeconds::new)
            .map(format_rfc3339)
            .transpose()?,
    })
}

const fn default_limit() -> u16 {
    25
}

fn json_object_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Object)
        .additional_properties(Some(AdditionalProperties::FreeForm(true)))
        .into()
}

fn nonnegative_integer_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(0))
        .into()
}

fn nullable_json_object_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(json_object_schema())
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_nonnegative_integer_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::Integer)
                    .minimum(Some(0)),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_kind_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .max_length(Some(128)),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_job_status_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .enum_values(Some([
                        "queued",
                        "running",
                        "cancel_requested",
                        "succeeded",
                        "failed",
                        "cancelled",
                    ])),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn job_status_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some([
            "queued",
            "running",
            "cancel_requested",
            "succeeded",
            "failed",
            "cancelled",
        ]))
        .into()
}

fn limit_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(1))
        .maximum(Some(100))
        .default(Some(serde_json::json!(25)))
        .into()
}
