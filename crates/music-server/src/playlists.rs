use axum::Json;
use axum::body::Body;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use music_application::auth::{SessionTouch, UnixSeconds};
use music_application::playlists::{
    AUTOMATIC_APPLY_SCHEMA, AUTOMATIC_PREVIEW_SCHEMA, AUTOMATIC_RULE_SCHEMA, AutomaticMatch,
    AutomaticOrder, AutomaticPlaylistResolution, AutomaticPlaylistRule, AutomaticTagSources,
    PatchValue, PlaylistCreate, PlaylistFilter, PlaylistItemRecord, PlaylistPatch, PlaylistRecord,
    PlaylistService, PlaylistServiceError,
};
use music_domain::{IndexedTrack, TrackId};
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{
    AnyOfBuilder, ArrayBuilder, ObjectBuilder, Schema, SchemaType, Type,
};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::{current_session, format_rfc3339};
use crate::error::{
    ApiError, HttpValidationErrorBody, openapi_datetime, openapi_integer,
    openapi_nullable_datetime, openapi_nullable_integer, openapi_nullable_string, openapi_number,
};
use crate::http::HttpState;

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = PlaylistCreate)]
struct PlaylistCreateRequest {
    #[schema(min_length = 1, max_length = 256)]
    name: String,
    #[schema(required = false, schema_with = nullable_bounded_64_schema)]
    mode_id: Option<String>,
    #[schema(required = false, schema_with = nullable_bounded_64_schema)]
    category: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = PlaylistUpdate)]
struct PlaylistUpdateRequest {
    #[schema(required = false, schema_with = nullable_playlist_name_schema)]
    name: Option<String>,
    #[schema(required = false, schema_with = nullable_bounded_64_schema)]
    mode_id: Option<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = nullable_bounded_64_schema)]
    category: NullablePatch<String>,
}

#[derive(Debug, Clone, Default)]
enum NullablePatch<T> {
    #[default]
    Unchanged,
    Set(Option<T>),
}

impl<'de, T> Deserialize<'de> for NullablePatch<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Self::Set)
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = PlaylistMeta)]
struct PlaylistMetaResponse {
    #[schema(schema_with = openapi_integer)]
    id: i64,
    name: String,
    #[schema(required = true, schema_with = openapi_nullable_string)]
    mode_id: Option<String>,
    #[schema(required = true, schema_with = openapi_nullable_string)]
    category: Option<String>,
    automatic: bool,
    #[schema(required = true, schema_with = nullable_automatic_rule_schema)]
    automatic_rule: Option<AutomaticPlaylistRuleResponse>,
    #[schema(required = false, schema_with = openapi_nullable_string)]
    automatic_rule_error: Option<String>,
    #[schema(required = true, schema_with = openapi_nullable_datetime)]
    automatic_refreshed_at: Option<String>,
    #[schema(schema_with = openapi_datetime)]
    created_at: String,
    #[schema(schema_with = openapi_datetime)]
    updated_at: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = TrackSummary)]
struct TrackSummaryResponse {
    #[schema(schema_with = openapi_integer)]
    id: i64,
    path: String,
    title: String,
    artist: String,
    album: String,
    #[schema(schema_with = openapi_number)]
    length_s: f64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = TrackInPlaylist)]
struct TrackInPlaylistResponse {
    #[schema(schema_with = openapi_integer)]
    position: i64,
    #[schema(schema_with = openapi_integer)]
    track_id: i64,
    #[schema(required = true, schema_with = nullable_track_summary_schema)]
    track: Option<TrackSummaryResponse>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = TrackAddRequest)]
struct TrackAddRequest {
    #[schema(schema_with = openapi_integer)]
    track_id: i64,
    #[schema(required = false, schema_with = openapi_nullable_integer)]
    position: Option<i64>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = app__api__playlists__TrackMoveRequest)]
struct PlaylistTrackMoveRequest {
    #[schema(schema_with = nonnegative_integer_schema)]
    to_position: i64,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum AutomaticMatchRequest {
    Any,
    All,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum AutomaticTagSourcesRequest {
    Manual,
    ManualAndLocal,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum AutomaticOrderRequest {
    Title,
    Newest,
    BpmAscending,
    BpmDescending,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = AutomaticPlaylistRuleV1)]
struct AutomaticPlaylistRuleRequest {
    #[serde(rename = "schema")]
    #[schema(schema_with = automatic_rule_version_schema)]
    schema_version: String,
    #[serde(default)]
    #[schema(required = false, schema_with = automatic_tags_schema)]
    include_tags: Vec<String>,
    #[serde(default = "default_match")]
    #[schema(required = false, schema_with = automatic_match_schema)]
    r#match: AutomaticMatchRequest,
    #[serde(default)]
    #[schema(required = false, schema_with = automatic_tags_schema)]
    exclude_tags: Vec<String>,
    #[serde(default = "default_tag_sources")]
    #[schema(required = false, schema_with = automatic_tag_sources_schema)]
    tag_sources: AutomaticTagSourcesRequest,
    #[serde(default)]
    #[schema(required = false, schema_with = nullable_bpm_schema)]
    min_bpm: Option<u32>,
    #[serde(default)]
    #[schema(required = false, schema_with = nullable_bpm_schema)]
    max_bpm: Option<u32>,
    #[serde(default = "default_true")]
    #[schema(required = false, default = true)]
    include_unknown_bpm: bool,
    #[serde(default = "default_maximum_tracks")]
    #[schema(required = false, schema_with = automatic_maximum_tracks_schema)]
    maximum_tracks: u16,
    #[serde(default = "default_order")]
    #[schema(required = false, schema_with = automatic_order_schema)]
    order_by: AutomaticOrderRequest,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = AutomaticPlaylistPreviewRequest)]
struct AutomaticPlaylistPreviewRequest {
    rule: AutomaticPlaylistRuleRequest,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = AutomaticPlaylistConfigureRequest)]
struct AutomaticPlaylistConfigureRequest {
    rule: AutomaticPlaylistRuleRequest,
    #[schema(pattern = "^[a-f0-9]{64}$")]
    source_signature: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = AutomaticTrackSummary)]
struct AutomaticTrackSummaryResponse {
    #[schema(schema_with = openapi_integer)]
    id: i64,
    path: String,
    title: String,
    artist: String,
    album: String,
    #[schema(schema_with = openapi_number)]
    length_s: f64,
    #[schema(required = true, schema_with = openapi_nullable_integer)]
    bpm: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = AutomaticPlaylistPreview)]
struct AutomaticPlaylistPreviewResponse {
    #[schema(schema_with = automatic_preview_version_schema)]
    schema_version: &'static str,
    #[schema(pattern = "^[a-f0-9]{64}$")]
    source_signature: String,
    #[schema(schema_with = openapi_integer)]
    library_tracks: usize,
    #[schema(schema_with = openapi_integer)]
    matched_tracks: usize,
    tracks: Vec<AutomaticTrackSummaryResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = AutomaticPlaylistApplyResult)]
struct AutomaticPlaylistApplyResponse {
    #[schema(schema_with = automatic_apply_version_schema)]
    schema_version: &'static str,
    playlist: PlaylistMetaResponse,
    #[schema(schema_with = openapi_integer)]
    materialized_tracks: usize,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct PlaylistListQuery {
    #[param(schema_with = openapi_nullable_string)]
    mode_id: Option<String>,
    #[param(schema_with = openapi_nullable_string)]
    category: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum ExportFormat {
    #[default]
    M3u,
    Json,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct ExportQuery {
    #[serde(default)]
    #[param(schema_with = export_format_schema)]
    format: ExportFormat,
}

struct ExportFileResponse;

impl utoipa::PartialSchema for ExportFileResponse {
    fn schema() -> RefOr<Schema> {
        Schema::Object(
            ObjectBuilder::new()
                .schema_type(SchemaType::AnyValue)
                .build(),
        )
        .into()
    }
}

impl ToSchema for ExportFileResponse {}

pub(crate) fn playlist_router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default()
        .routes(routes!(create_playlist))
        .routes(routes!(list_playlists))
        .routes(routes!(get_playlist))
        .routes(routes!(update_playlist))
        .routes(routes!(delete_playlist))
        .routes(routes!(get_tracks))
        .routes(routes!(add_track))
        .routes(routes!(remove_track))
        .routes(routes!(move_track))
        .routes(routes!(preview_automatic_playlist))
        .routes(routes!(configure_automatic_playlist))
        .routes(routes!(refresh_automatic_playlist))
        .routes(routes!(disable_automatic_playlist))
        .routes(routes!(export_playlist))
}

#[utoipa::path(
    post,
    path = "/playlists",
    operation_id = "create_playlist_api_playlists_post",
    request_body = PlaylistCreateRequest,
    responses(
        (status = 201, description = "Successful Response", body = PlaylistMetaResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "playlists"
)]
async fn create_playlist(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<PlaylistCreateRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<PlaylistMetaResponse>), ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    validate_name(&payload.name)?;
    validate_optional_short(&payload.mode_id)?;
    validate_optional_short(&payload.category)?;
    validate_mode(&state, payload.mode_id.as_deref())?;
    let playlist = service(&state)?
        .create(&PlaylistCreate {
            name: payload.name,
            mode_id: payload.mode_id,
            category: payload.category,
        })
        .await
        .map_err(map_playlist_error)?;
    Ok((StatusCode::CREATED, Json(playlist_meta(playlist)?)))
}

#[utoipa::path(
    get,
    path = "/playlists",
    operation_id = "list_playlists_api_playlists_get",
    params(PlaylistListQuery),
    responses(
        (status = 200, description = "Successful Response", body = [PlaylistMetaResponse]),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "playlists"
)]
async fn list_playlists(
    State(state): State<HttpState>,
    headers: HeaderMap,
    query: Result<Query<PlaylistListQuery>, QueryRejection>,
) -> Result<Json<Vec<PlaylistMetaResponse>>, ApiError> {
    authorize(&state, &headers).await?;
    let Query(query) = query.map_err(|_| ApiError::validation())?;
    let rows = service(&state)?
        .list(&PlaylistFilter {
            mode_id: query.mode_id,
            category: query.category,
        })
        .await
        .map_err(map_playlist_error)?;
    Ok(Json(
        rows.into_iter()
            .map(playlist_meta)
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

#[utoipa::path(
    get,
    path = "/playlists/{playlist_id}",
    operation_id = "get_playlist_api_playlists__playlist_id__get",
    params(("playlist_id" = i128, Path)),
    responses(
        (status = 200, description = "Successful Response", body = PlaylistMetaResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "playlists"
)]
async fn get_playlist(
    State(state): State<HttpState>,
    headers: HeaderMap,
    playlist_id: Result<Path<i64>, PathRejection>,
) -> Result<Json<PlaylistMetaResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Path(playlist_id) = playlist_id.map_err(|_| ApiError::validation())?;
    let playlist = service(&state)?
        .get(playlist_id)
        .await
        .map_err(map_playlist_error)?;
    Ok(Json(playlist_meta(playlist)?))
}

#[utoipa::path(
    patch,
    path = "/playlists/{playlist_id}",
    operation_id = "update_playlist_api_playlists__playlist_id__patch",
    params(("playlist_id" = i128, Path)),
    request_body = PlaylistUpdateRequest,
    responses(
        (status = 200, description = "Successful Response", body = PlaylistMetaResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "playlists"
)]
async fn update_playlist(
    State(state): State<HttpState>,
    headers: HeaderMap,
    playlist_id: Result<Path<i64>, PathRejection>,
    payload: Result<Json<PlaylistUpdateRequest>, JsonRejection>,
) -> Result<Json<PlaylistMetaResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Path(playlist_id) = playlist_id.map_err(|_| ApiError::validation())?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if let Some(name) = &payload.name {
        validate_name(name)?;
    }
    validate_optional_short(&payload.mode_id)?;
    if let NullablePatch::Set(category) = &payload.category {
        validate_optional_short(category)?;
    }
    validate_mode(&state, payload.mode_id.as_deref())?;
    let category = match payload.category {
        NullablePatch::Unchanged => PatchValue::Unchanged,
        NullablePatch::Set(value) => PatchValue::Set(value),
    };
    let playlist = service(&state)?
        .update(
            playlist_id,
            &PlaylistPatch {
                name: payload.name.map_or(PatchValue::Unchanged, PatchValue::Set),
                mode_id: payload
                    .mode_id
                    .map_or(PatchValue::Unchanged, PatchValue::Set),
                category,
            },
        )
        .await
        .map_err(map_playlist_error)?;
    Ok(Json(playlist_meta(playlist)?))
}

#[utoipa::path(
    delete,
    path = "/playlists/{playlist_id}",
    operation_id = "delete_playlist_api_playlists__playlist_id__delete",
    params(("playlist_id" = i128, Path)),
    responses(
        (status = 204, description = "Successful Response"),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "playlists"
)]
async fn delete_playlist(
    State(state): State<HttpState>,
    headers: HeaderMap,
    playlist_id: Result<Path<i64>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    authorize(&state, &headers).await?;
    let Path(playlist_id) = playlist_id.map_err(|_| ApiError::validation())?;
    service(&state)?
        .delete(playlist_id)
        .await
        .map_err(map_playlist_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/playlists/{playlist_id}/tracks",
    operation_id = "get_tracks_api_playlists__playlist_id__tracks_get",
    params(("playlist_id" = i128, Path)),
    responses(
        (status = 200, description = "Successful Response", body = [TrackInPlaylistResponse]),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "playlists"
)]
async fn get_tracks(
    State(state): State<HttpState>,
    headers: HeaderMap,
    playlist_id: Result<Path<i64>, PathRejection>,
) -> Result<Json<Vec<TrackInPlaylistResponse>>, ApiError> {
    authorize(&state, &headers).await?;
    let Path(playlist_id) = playlist_id.map_err(|_| ApiError::validation())?;
    let items = service(&state)?
        .items(playlist_id)
        .await
        .map_err(map_playlist_error)?;
    Ok(Json(
        items.items.into_iter().map(track_in_playlist).collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/playlists/{playlist_id}/tracks",
    operation_id = "add_track_api_playlists__playlist_id__tracks_post",
    params(("playlist_id" = i128, Path)),
    request_body = TrackAddRequest,
    responses(
        (status = 201, description = "Successful Response", body = TrackInPlaylistResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "playlists"
)]
async fn add_track(
    State(state): State<HttpState>,
    headers: HeaderMap,
    playlist_id: Result<Path<i64>, PathRejection>,
    payload: Result<Json<TrackAddRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<TrackInPlaylistResponse>), ApiError> {
    authorize(&state, &headers).await?;
    let Path(playlist_id) = playlist_id.map_err(|_| ApiError::validation())?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let track_id = TrackId::new(payload.track_id)
        .map_err(|_| ApiError::plain_not_found("track not in library"))?;
    let position = payload
        .position
        .map(|position| {
            usize::try_from(position).map_err(|_| ApiError::bad_request("position out of range"))
        })
        .transpose()?;
    let item = service(&state)?
        .add_track(playlist_id, track_id, position)
        .await
        .map_err(map_playlist_error)?;
    Ok((StatusCode::CREATED, Json(track_in_playlist(item))))
}

#[utoipa::path(
    delete,
    path = "/playlists/{playlist_id}/tracks/{position}",
    operation_id = "remove_track_api_playlists__playlist_id__tracks__position__delete",
    params(("playlist_id" = i128, Path), ("position" = i128, Path)),
    responses(
        (status = 204, description = "Successful Response"),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "playlists"
)]
async fn remove_track(
    State(state): State<HttpState>,
    headers: HeaderMap,
    path: Result<Path<(i64, i64)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    authorize(&state, &headers).await?;
    let Path((playlist_id, position)) = path.map_err(|_| ApiError::validation())?;
    let position = usize::try_from(position)
        .map_err(|_| ApiError::plain_not_found("position out of range"))?;
    service(&state)?
        .remove_track(playlist_id, position)
        .await
        .map_err(map_remove_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    patch,
    path = "/playlists/{playlist_id}/tracks/{position}",
    operation_id = "move_track_api_playlists__playlist_id__tracks__position__patch",
    params(("playlist_id" = i128, Path), ("position" = i128, Path)),
    request_body = PlaylistTrackMoveRequest,
    responses(
        (status = 204, description = "Successful Response"),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "playlists"
)]
async fn move_track(
    State(state): State<HttpState>,
    headers: HeaderMap,
    path: Result<Path<(i64, i64)>, PathRejection>,
    payload: Result<Json<PlaylistTrackMoveRequest>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authorize(&state, &headers).await?;
    let Path((playlist_id, position)) = path.map_err(|_| ApiError::validation())?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let from =
        usize::try_from(position).map_err(|_| ApiError::bad_request("position out of range"))?;
    let to = usize::try_from(payload.to_position).map_err(|_| ApiError::validation())?;
    service(&state)?
        .move_track(playlist_id, from, to)
        .await
        .map_err(map_playlist_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/playlists/{playlist_id}/automatic/preview",
    operation_id = "preview_automatic_playlist_api_playlists__playlist_id__automatic_preview_post",
    params(("playlist_id" = i128, Path)),
    request_body = AutomaticPlaylistPreviewRequest,
    responses(
        (status = 200, description = "Successful Response", body = AutomaticPlaylistPreviewResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "playlists"
)]
async fn preview_automatic_playlist(
    State(state): State<HttpState>,
    headers: HeaderMap,
    playlist_id: Result<Path<i64>, PathRejection>,
    payload: Result<Json<AutomaticPlaylistPreviewRequest>, JsonRejection>,
) -> Result<Json<AutomaticPlaylistPreviewResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Path(playlist_id) = playlist_id.map_err(|_| ApiError::validation())?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let resolution = service(&state)?
        .preview(playlist_id, payload.rule.try_into()?)
        .await
        .map_err(map_playlist_error)?;
    Ok(Json(automatic_preview(resolution)))
}

#[utoipa::path(
    put,
    path = "/playlists/{playlist_id}/automatic",
    operation_id = "configure_automatic_playlist_api_playlists__playlist_id__automatic_put",
    params(("playlist_id" = i128, Path)),
    request_body = AutomaticPlaylistConfigureRequest,
    responses(
        (status = 200, description = "Successful Response", body = AutomaticPlaylistApplyResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "playlists"
)]
async fn configure_automatic_playlist(
    State(state): State<HttpState>,
    headers: HeaderMap,
    playlist_id: Result<Path<i64>, PathRejection>,
    payload: Result<Json<AutomaticPlaylistConfigureRequest>, JsonRejection>,
) -> Result<Json<AutomaticPlaylistApplyResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Path(playlist_id) = playlist_id.map_err(|_| ApiError::validation())?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if !valid_signature(&payload.source_signature) {
        return Err(ApiError::validation());
    }
    let (playlist, resolution) = service(&state)?
        .configure(
            playlist_id,
            payload.rule.try_into()?,
            &payload.source_signature,
        )
        .await
        .map_err(map_playlist_error)?;
    Ok(Json(automatic_apply(playlist, resolution)?))
}

#[utoipa::path(
    post,
    path = "/playlists/{playlist_id}/automatic/refresh",
    operation_id = "refresh_automatic_playlist_api_playlists__playlist_id__automatic_refresh_post",
    params(("playlist_id" = i128, Path)),
    responses(
        (status = 200, description = "Successful Response", body = AutomaticPlaylistApplyResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "playlists"
)]
async fn refresh_automatic_playlist(
    State(state): State<HttpState>,
    headers: HeaderMap,
    playlist_id: Result<Path<i64>, PathRejection>,
) -> Result<Json<AutomaticPlaylistApplyResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Path(playlist_id) = playlist_id.map_err(|_| ApiError::validation())?;
    let (playlist, resolution) = service(&state)?
        .refresh(playlist_id)
        .await
        .map_err(map_playlist_error)?;
    Ok(Json(automatic_apply(playlist, resolution)?))
}

#[utoipa::path(
    delete,
    path = "/playlists/{playlist_id}/automatic",
    operation_id = "disable_automatic_playlist_api_playlists__playlist_id__automatic_delete",
    params(("playlist_id" = i128, Path)),
    responses(
        (status = 200, description = "Successful Response", body = PlaylistMetaResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "playlists"
)]
async fn disable_automatic_playlist(
    State(state): State<HttpState>,
    headers: HeaderMap,
    playlist_id: Result<Path<i64>, PathRejection>,
) -> Result<Json<PlaylistMetaResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Path(playlist_id) = playlist_id.map_err(|_| ApiError::validation())?;
    let playlist = service(&state)?
        .disable_automatic(playlist_id)
        .await
        .map_err(map_playlist_error)?;
    Ok(Json(playlist_meta(playlist)?))
}

#[utoipa::path(
    get,
    path = "/playlists/{playlist_id}/export",
    operation_id = "export_playlist_api_playlists__playlist_id__export_get",
    description = "Download the playlist as M3U (default; for VLC, foobar2000, etc.) or\nJSON (structured, includes track metadata for re-import). M3U paths are\nrelative to MUSIC_DIR — drop the file at MUSIC_DIR's root to import.",
    params(("playlist_id" = i128, Path), ExportQuery),
    responses(
        (status = 200, description = "Successful Response", body = ExportFileResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "playlists"
)]
async fn export_playlist(
    State(state): State<HttpState>,
    headers: HeaderMap,
    playlist_id: Result<Path<i64>, PathRejection>,
    query: Result<Query<ExportQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    authorize(&state, &headers).await?;
    let Path(playlist_id) = playlist_id.map_err(|_| ApiError::validation())?;
    let Query(query) = query.map_err(|_| ApiError::validation())?;
    let items = service(&state)?
        .items(playlist_id)
        .await
        .map_err(map_playlist_error)?;
    let filename = safe_filename(&items.playlist.name);
    let (body, content_type, disposition) = match query.format {
        ExportFormat::M3u => (
            build_m3u(&items.playlist.name, &items.items),
            "application/vnd.apple.mpegurl",
            format!("attachment; filename=\"{filename}.m3u8\""),
        ),
        ExportFormat::Json => (
            build_json_export(&items.playlist, &items.items)?,
            "application/json",
            format!("attachment; filename=\"{filename}.json\""),
        ),
    };
    let mut response = Body::from(body).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition).map_err(|_| ApiError::internal())?,
    );
    Ok(response)
}

async fn authorize(state: &HttpState, headers: &HeaderMap) -> Result<(), ApiError> {
    current_session(state, headers, SessionTouch::UpdateLastSeen)
        .await
        .map(|_| ())
}

fn service(state: &HttpState) -> Result<&PlaylistService, ApiError> {
    state
        .playlists
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)
}

fn validate_mode(state: &HttpState, mode_id: Option<&str>) -> Result<(), ApiError> {
    let Some(mode_id) = mode_id else {
        return Ok(());
    };
    let catalog = state
        .modes
        .as_ref()
        .and_then(|modes| modes.snapshot())
        .ok_or_else(ApiError::service_unavailable)?;
    if catalog.modes.contains_key(mode_id) {
        Ok(())
    } else {
        Err(ApiError::bad_request("unknown mode"))
    }
}

fn validate_name(name: &str) -> Result<(), ApiError> {
    if (1..=256).contains(&name.chars().count()) {
        Ok(())
    } else {
        Err(ApiError::validation())
    }
}

fn validate_optional_short(value: &Option<String>) -> Result<(), ApiError> {
    if value
        .as_ref()
        .is_none_or(|value| value.chars().count() <= 64)
    {
        Ok(())
    } else {
        Err(ApiError::validation())
    }
}

fn playlist_meta(record: PlaylistRecord) -> Result<PlaylistMetaResponse, ApiError> {
    let automatic = record.is_automatic();
    let (automatic_rule, automatic_rule_error) = if automatic {
        match record.automatic_rule() {
            Ok(Some(rule)) => (Some(rule.into()), None),
            Ok(None) => (None, None),
            Err(_) => (None, Some("automatic_rule_invalid".to_owned())),
        }
    } else {
        (None, None)
    };
    Ok(PlaylistMetaResponse {
        id: record.id,
        name: record.name,
        mode_id: record.mode_id,
        category: record.category,
        automatic,
        automatic_rule,
        automatic_rule_error,
        automatic_refreshed_at: record
            .automatic_refreshed_at_unix_seconds
            .map(|value| format_rfc3339(UnixSeconds::new(value)))
            .transpose()?,
        created_at: format_rfc3339(UnixSeconds::new(record.created_at_unix_seconds))?,
        updated_at: format_rfc3339(UnixSeconds::new(record.updated_at_unix_seconds))?,
    })
}

fn track_summary(track: IndexedTrack) -> TrackSummaryResponse {
    TrackSummaryResponse {
        id: track.id.get(),
        path: track.path.into_string(),
        title: track.metadata.title,
        artist: track.metadata.artist,
        album: track.metadata.album,
        length_s: track.duration.as_secs_f64(),
    }
}

fn track_in_playlist(item: PlaylistItemRecord) -> TrackInPlaylistResponse {
    TrackInPlaylistResponse {
        position: item.position,
        track_id: item.track_id,
        track: item.track.map(track_summary),
    }
}

fn automatic_track(track: IndexedTrack) -> AutomaticTrackSummaryResponse {
    AutomaticTrackSummaryResponse {
        id: track.id.get(),
        path: track.path.into_string(),
        title: track.metadata.title,
        artist: track.metadata.artist,
        album: track.metadata.album,
        length_s: track.duration.as_secs_f64(),
        bpm: track.metadata.bpm,
    }
}

fn automatic_preview(resolution: AutomaticPlaylistResolution) -> AutomaticPlaylistPreviewResponse {
    let matched_tracks = resolution.tracks.len();
    AutomaticPlaylistPreviewResponse {
        schema_version: AUTOMATIC_PREVIEW_SCHEMA,
        source_signature: resolution.source_signature,
        library_tracks: resolution.library_tracks,
        matched_tracks,
        tracks: resolution.tracks.into_iter().map(automatic_track).collect(),
    }
}

fn automatic_apply(
    playlist: PlaylistRecord,
    resolution: AutomaticPlaylistResolution,
) -> Result<AutomaticPlaylistApplyResponse, ApiError> {
    Ok(AutomaticPlaylistApplyResponse {
        schema_version: AUTOMATIC_APPLY_SCHEMA,
        playlist: playlist_meta(playlist)?,
        materialized_tracks: resolution.tracks.len(),
    })
}

impl TryFrom<AutomaticPlaylistRuleRequest> for AutomaticPlaylistRule {
    type Error = ApiError;

    fn try_from(rule: AutomaticPlaylistRuleRequest) -> Result<Self, Self::Error> {
        AutomaticPlaylistRule {
            schema_version: rule.schema_version,
            include_tags: rule.include_tags,
            r#match: match rule.r#match {
                AutomaticMatchRequest::Any => AutomaticMatch::Any,
                AutomaticMatchRequest::All => AutomaticMatch::All,
            },
            exclude_tags: rule.exclude_tags,
            tag_sources: match rule.tag_sources {
                AutomaticTagSourcesRequest::Manual => AutomaticTagSources::Manual,
                AutomaticTagSourcesRequest::ManualAndLocal => AutomaticTagSources::ManualAndLocal,
            },
            min_bpm: rule.min_bpm,
            max_bpm: rule.max_bpm,
            include_unknown_bpm: rule.include_unknown_bpm,
            maximum_tracks: rule.maximum_tracks,
            order_by: match rule.order_by {
                AutomaticOrderRequest::Title => AutomaticOrder::Title,
                AutomaticOrderRequest::Newest => AutomaticOrder::Newest,
                AutomaticOrderRequest::BpmAscending => AutomaticOrder::BpmAscending,
                AutomaticOrderRequest::BpmDescending => AutomaticOrder::BpmDescending,
            },
        }
        .normalized()
        .map_err(|_| ApiError::validation())
    }
}

impl From<AutomaticPlaylistRule> for AutomaticPlaylistRuleResponse {
    fn from(rule: AutomaticPlaylistRule) -> Self {
        Self {
            schema_version: rule.schema_version,
            include_tags: rule.include_tags,
            r#match: match rule.r#match {
                AutomaticMatch::Any => AutomaticMatchRequest::Any,
                AutomaticMatch::All => AutomaticMatchRequest::All,
            },
            exclude_tags: rule.exclude_tags,
            tag_sources: match rule.tag_sources {
                AutomaticTagSources::Manual => AutomaticTagSourcesRequest::Manual,
                AutomaticTagSources::ManualAndLocal => AutomaticTagSourcesRequest::ManualAndLocal,
            },
            min_bpm: rule.min_bpm,
            max_bpm: rule.max_bpm,
            include_unknown_bpm: rule.include_unknown_bpm,
            maximum_tracks: rule.maximum_tracks,
            order_by: match rule.order_by {
                AutomaticOrder::Title => AutomaticOrderRequest::Title,
                AutomaticOrder::Newest => AutomaticOrderRequest::Newest,
                AutomaticOrder::BpmAscending => AutomaticOrderRequest::BpmAscending,
                AutomaticOrder::BpmDescending => AutomaticOrderRequest::BpmDescending,
            },
        }
    }
}

type AutomaticPlaylistRuleResponse = AutomaticPlaylistRuleRequest;

fn map_playlist_error(error: PlaylistServiceError) -> ApiError {
    match error {
        PlaylistServiceError::NotFound => ApiError::plain_not_found("playlist not found"),
        PlaylistServiceError::TrackNotFound => ApiError::plain_not_found("track not in library"),
        PlaylistServiceError::PositionOutOfRange => ApiError::bad_request("position out of range"),
        PlaylistServiceError::AutomaticItemsManaged => ApiError::coded_conflict(
            "automatic_playlist_items_managed",
            "This playlist is automatic. Edit its rule or make it manual before changing individual songs.",
        ),
        PlaylistServiceError::NotAutomatic => ApiError::coded_conflict(
            "playlist_not_automatic",
            "This playlist does not have an automatic rule.",
        ),
        PlaylistServiceError::StalePreview => ApiError::coded_conflict(
            "automatic_playlist_preview_stale",
            "The library or tags changed. Preview the rule again.",
        ),
        PlaylistServiceError::CapacityExceeded => ApiError::bad_request("playlist is too large"),
        PlaylistServiceError::ConcurrentChange => {
            ApiError::conflict("playlist changed; retry the request")
        }
        PlaylistServiceError::InvalidRule(_) => ApiError::validation(),
        PlaylistServiceError::Dependency(error) => {
            tracing::error!(error = %error, "playlist repository operation failed");
            ApiError::internal()
        }
    }
}

fn map_remove_error(error: PlaylistServiceError) -> ApiError {
    match error {
        PlaylistServiceError::PositionOutOfRange => {
            ApiError::plain_not_found("position out of range")
        }
        other => map_playlist_error(other),
    }
}

fn valid_signature(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn safe_filename(name: &str) -> String {
    let mut output = String::new();
    let mut replacing = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            output.push(character);
            replacing = false;
        } else if !replacing {
            output.push('_');
            replacing = true;
        }
    }
    let output = output.trim_matches(['.', '_']).to_owned();
    if output.is_empty() {
        "playlist".to_owned()
    } else {
        output
    }
}

fn build_m3u(name: &str, items: &[PlaylistItemRecord]) -> String {
    let mut lines = vec!["#EXTM3U".to_owned(), format!("#PLAYLIST:{name}")];
    for item in items {
        let Some(track) = &item.track else {
            lines.push(format!("# missing track #{}", item.track_id));
            continue;
        };
        let seconds = track.duration.as_secs_f64().round().max(0.0) as u64;
        let title = if track.metadata.title.is_empty() {
            track
                .path
                .as_str()
                .rsplit('/')
                .next()
                .unwrap_or(track.path.as_str())
        } else {
            &track.metadata.title
        };
        lines.push(format!(
            "#EXTINF:{seconds},{} - {}",
            track.metadata.artist.replace('\n', " "),
            title.replace('\n', " ")
        ));
        lines.push(track.path.as_str().to_owned());
    }
    format!("{}\n", lines.join("\n"))
}

fn build_json_export(
    playlist: &PlaylistRecord,
    items: &[PlaylistItemRecord],
) -> Result<String, ApiError> {
    let tracks = items
        .iter()
        .map(|item| {
            let track = item.track.as_ref();
            json!({
                "position": item.position,
                "path": track.map(|track| track.path.as_str()),
                "title": track.map(|track| track.metadata.title.as_str()),
                "artist": track.map(|track| track.metadata.artist.as_str()),
                "album": track.map(|track| track.metadata.album.as_str()),
                "length_s": track.map(|track| track.duration.as_secs_f64()),
            })
        })
        .collect::<Vec<_>>();
    let payload = json!({
        "playlist": {
            "name": playlist.name,
            "mode_id": playlist.mode_id,
            "category": playlist.category,
            "created_at": format_rfc3339(UnixSeconds::new(playlist.created_at_unix_seconds))?,
        },
        "tracks": tracks,
    });
    let mut encoded = serde_json::to_string_pretty(&payload).map_err(|_| ApiError::internal())?;
    encoded.push('\n');
    Ok(encoded)
}

const fn default_match() -> AutomaticMatchRequest {
    AutomaticMatchRequest::Any
}
const fn default_tag_sources() -> AutomaticTagSourcesRequest {
    AutomaticTagSourcesRequest::Manual
}
const fn default_order() -> AutomaticOrderRequest {
    AutomaticOrderRequest::Title
}
const fn default_true() -> bool {
    true
}
const fn default_maximum_tracks() -> u16 {
    200
}

fn nullable_bounded_64_schema() -> RefOr<Schema> {
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

fn nullable_playlist_name_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .min_length(Some(1))
                    .max_length(Some(256)),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_automatic_rule_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(RefOr::Ref(utoipa::openapi::Ref::from_schema_name(
                "AutomaticPlaylistRuleV1",
            )))
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_track_summary_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(<TrackSummaryResponse as utoipa::PartialSchema>::schema())
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nonnegative_integer_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(0))
        .into()
}

fn automatic_tags_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(ObjectBuilder::new().schema_type(Type::String))
        .max_items(Some(32))
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

fn automatic_rule_version_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .extensions(Some(
            [("const", json!(AUTOMATIC_RULE_SCHEMA))]
                .into_iter()
                .collect(),
        ))
        .into()
}

fn automatic_preview_version_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .extensions(Some(
            [("const", json!(AUTOMATIC_PREVIEW_SCHEMA))]
                .into_iter()
                .collect(),
        ))
        .into()
}

fn automatic_apply_version_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .extensions(Some(
            [("const", json!(AUTOMATIC_APPLY_SCHEMA))]
                .into_iter()
                .collect(),
        ))
        .into()
}

fn automatic_match_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["any", "all"]))
        .default(Some(json!("any")))
        .into()
}

fn automatic_tag_sources_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["manual", "manual_and_local"]))
        .default(Some(json!("manual")))
        .into()
}

fn automatic_order_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["title", "newest", "bpm_ascending", "bpm_descending"]))
        .default(Some(json!("title")))
        .into()
}

fn automatic_maximum_tracks_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(1))
        .maximum(Some(1_000))
        .default(Some(json!(200)))
        .into()
}

fn export_format_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["m3u", "json"]))
        .default(Some(json!("m3u")))
        .into()
}
