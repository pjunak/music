//! Library context analysis: start a pass, read the library summary and one track's detail.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use music_application::assistant::{
    LIBRARY_CONTEXT_JOB_KIND, LibraryContextPassSummary, LibraryContextSummary, TrackContextDetail,
    VoiceAnalyzerStatus,
};
use music_application::auth::UnixSeconds;
use music_domain::TrackId;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use utoipa::ToSchema;

use crate::analysis::ContextAnalysisParameters;
use crate::auth::{Operator, format_rfc3339};
use crate::error::{ApiError, HttpValidationErrorBody, openapi_nullable_datetime};
use crate::http::HttpState;
use crate::jobs::{BackgroundJobResponse, job_response, map_job_error};

use super::schema::{
    context_analyzer_id_schema, context_sections_schema, context_status_schema,
    context_timeline_schema, model_tagging_scope_ref, nonnegative_integer_schema,
    nullable_object_schema, nullable_string_schema, nullable_voice_reason_schema,
    positive_exclusive_integer_schema, voice_analyzer_id_schema, voice_analyzer_status_schema,
};
use super::shared::{ModelTaggingScopeWire, analysis_service, map_local_analysis_error};

#[derive(Debug, Clone, Default, Deserialize, Serialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
#[schema(as = LibraryContextStartRequest)]
pub(super) struct LibraryContextStartRequest {
    #[schema(required = false, default = false)]
    pub(super) force: bool,
    #[schema(required = false, schema_with = model_tagging_scope_ref)]
    pub(super) scope: ModelTaggingScopeWire,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = VoiceAnalyzerStatus)]
pub(super) struct VoiceAnalyzerStatusResponse {
    #[schema(schema_with = voice_analyzer_id_schema)]
    pub(super) analyzer_id: String,
    #[schema(schema_with = voice_analyzer_status_schema)]
    pub(super) status: String,
    #[schema(required = true, schema_with = nullable_voice_reason_schema)]
    pub(super) reason: Option<String>,
    pub(super) model_filename: String,
    pub(super) model_sha256: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = LibraryContextPassSummary)]
pub(super) struct LibraryContextPassSummaryResponse {
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) completed_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) failed_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) skipped_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) total_tracks: usize,
    #[schema(default = true)]
    pub(super) enabled: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = LibraryContextPasses)]
pub(super) struct LibraryContextPassesResponse {
    pub(super) audio_context: LibraryContextPassSummaryResponse,
    pub(super) voice_detection: LibraryContextPassSummaryResponse,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = LibraryContextSummary)]
pub(super) struct LibraryContextSummaryResponse {
    #[schema(schema_with = context_analyzer_id_schema)]
    pub(super) analyzer: String,
    pub(super) voice_analyzer: VoiceAnalyzerStatusResponse,
    pub(super) passes: LibraryContextPassesResponse,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) library_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) analyzed_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) full_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) partial_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) missing_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) failed_tracks: usize,
    #[schema(schema_with = nonnegative_integer_schema)]
    pub(super) stale_tracks: usize,
    #[schema(required = true, schema_with = openapi_nullable_datetime)]
    pub(super) last_updated_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TrackContextDetail)]
pub(super) struct TrackContextDetailResponse {
    #[schema(schema_with = positive_exclusive_integer_schema)]
    pub(super) track_id: i64,
    pub(super) title: String,
    pub(super) artist: String,
    #[schema(schema_with = context_status_schema)]
    pub(super) status: String,
    #[schema(schema_with = context_analyzer_id_schema)]
    pub(super) analyzer_id: String,
    #[schema(required = true, schema_with = openapi_nullable_datetime)]
    pub(super) updated_at: Option<String>,
    #[schema(required = true, schema_with = nullable_object_schema)]
    pub(super) summary: Option<Map<String, Value>>,
    #[schema(schema_with = context_timeline_schema)]
    pub(super) timeline: Vec<BTreeMap<String, f64>>,
    #[schema(schema_with = context_sections_schema)]
    pub(super) sections: Vec<Map<String, Value>>,
    #[schema(required = true, schema_with = nullable_object_schema)]
    pub(super) technical: Option<Map<String, Value>>,
    #[schema(required = true, schema_with = nullable_object_schema)]
    pub(super) stages: Option<Map<String, Value>>,
    #[schema(required = true, schema_with = nullable_string_schema)]
    pub(super) error: Option<String>,
}

#[utoipa::path(
    post,
    path = "/assistant/library-context/jobs",
    request_body = LibraryContextStartRequest,
    responses(
        (status = 202, description = "Successful Response", body = BackgroundJobResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
pub(super) async fn start_library_context_analysis(
    State(state): State<HttpState>,
    _operator: Operator,
    payload: Result<Json<LibraryContextStartRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<BackgroundJobResponse>), ApiError> {
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let parameters = ContextAnalysisParameters {
        force: payload.force,
        scope: payload.scope.into_parameters()?,
    };
    let parameters = serde_json::to_value(parameters).map_err(|error| {
        ApiError::internal_from("context analysis parameters could not be encoded", &error)
    })?;
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
    responses((status = 200, description = "Successful Response", body = LibraryContextSummaryResponse)),
    tag = "assistant"
)]
pub(super) async fn library_context_summary(
    State(state): State<HttpState>,
    _operator: Operator,
) -> Result<Json<LibraryContextSummaryResponse>, ApiError> {
    let summary = analysis_service(&state)?
        .context_summary()
        .await
        .map_err(map_local_analysis_error)?;
    Ok(Json(library_context_summary_response(summary)?))
}

#[utoipa::path(
    get,
    path = "/assistant/library-context/tracks/{track_id}",
    params(("track_id" = i128, Path, description = "Track Id")),
    responses(
        (status = 200, description = "Successful Response", body = TrackContextDetailResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
pub(super) async fn library_track_context(
    State(state): State<HttpState>,
    _operator: Operator,
    track_id: Result<Path<i128>, PathRejection>,
) -> Result<Json<TrackContextDetailResponse>, ApiError> {
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

pub(super) fn library_context_summary_response(
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
        last_updated_at: summary
            .last_updated_at_unix_seconds
            .map(UnixSeconds::new)
            .map(format_rfc3339)
            .transpose()?,
    })
}

pub(super) fn voice_analyzer_status_response(
    value: VoiceAnalyzerStatus,
) -> VoiceAnalyzerStatusResponse {
    VoiceAnalyzerStatusResponse {
        analyzer_id: value.analyzer_id,
        status: value.status.as_str().to_owned(),
        reason: value.reason,
        model_filename: value.model_filename,
        model_sha256: value.model_sha256,
    }
}

pub(super) fn context_pass_response(
    value: LibraryContextPassSummary,
) -> LibraryContextPassSummaryResponse {
    LibraryContextPassSummaryResponse {
        completed_tracks: value.completed_tracks,
        failed_tracks: value.failed_tracks,
        skipped_tracks: value.skipped_tracks,
        total_tracks: value.total_tracks,
        enabled: value.enabled,
    }
}

pub(super) fn track_context_detail_response(
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
