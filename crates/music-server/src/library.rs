use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::multipart::{Field, MultipartError, MultipartRejection};
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, Request, State};
use axum::http::header::{
    CONTENT_DISPOSITION, CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use music_application::auth::{SessionTouch, UnixSeconds};
use music_application::cleanup::{CleanupService, CleanupVerificationService};
use music_application::library::{
    FolderMutationResult, LibraryCoordinatorError, LibraryCoordinatorHandle,
    LibraryMutationFailureKind, LibrarySearch, LibraryService, LibrarySortKey,
    LibraryUploadBatchItem, SortOrder, StagedLibraryUpload, TrackMetadataField, TrackMetadataPatch,
    UploadConflictPolicy,
};
use music_domain::{IndexedTrack, LibraryPath, TrackId};
use music_media::{
    LibraryRoot, MediaDeliveryError, MetadataAdapter, library_upload_target_exists,
    list_library_directories, read_library_cover_art, resolve_library_media_file,
};
use serde::{Deserialize, Deserializer, Serialize};
use tokio::io::AsyncWriteExt;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{AnyOfBuilder, ArrayBuilder, ObjectBuilder, Schema, Type};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::error::{
    ApiError, HttpValidationErrorBody, openapi_datetime, openapi_integer, openapi_nullable_integer,
    openapi_nullable_string, openapi_number,
};
use crate::http::HttpState;

#[derive(Debug, Clone)]
pub(crate) struct RuntimeLibrary {
    pub(crate) service: Arc<LibraryService>,
    pub(crate) cleanup: Arc<CleanupService>,
    pub(crate) cleanup_verification: Arc<CleanupVerificationService>,
    pub(crate) coordinator: LibraryCoordinatorHandle,
    pub(crate) root: LibraryRoot,
    pub(crate) metadata: MetadataAdapter,
    pub(crate) max_upload_files: usize,
    pub(crate) max_upload_file_bytes: u64,
}

pub(crate) fn library_router() -> OpenApiRouter<HttpState> {
    let upload = OpenApiRouter::default()
        .routes(routes!(upload))
        .layer(DefaultBodyLimit::disable());
    OpenApiRouter::default()
        .routes(routes!(search))
        .routes(routes!(tree))
        .routes(routes!(folders))
        .routes(routes!(create_folder))
        .routes(routes!(delete_folder))
        .routes(routes!(rename_folder))
        .merge(upload)
        .routes(routes!(upload_check))
        .routes(routes!(tracks_batch))
        .routes(routes!(bulk_update_metadata))
        .routes(routes!(bulk_move_tracks))
        .routes(routes!(bulk_delete_tracks))
        .routes(routes!(track))
        .routes(routes!(update_metadata))
        .routes(routes!(move_track))
        .routes(routes!(delete_track))
        .routes(routes!(stream))
        .routes(routes!(cover))
        .routes(routes!(rescan))
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = TrackOut)]
struct TrackResponse {
    #[schema(schema_with = openapi_integer)]
    id: i64,
    path: String,
    title: String,
    artist: String,
    album_artist: String,
    album: String,
    #[schema(required = true, schema_with = openapi_nullable_integer)]
    track_no: Option<u32>,
    #[schema(required = true, schema_with = openapi_nullable_integer)]
    disc_no: Option<u32>,
    #[schema(required = true, schema_with = openapi_nullable_integer)]
    year: Option<u32>,
    genre: String,
    #[schema(schema_with = openapi_number)]
    length_s: f64,
    #[schema(required = true, schema_with = openapi_nullable_integer)]
    bpm: Option<u32>,
    #[schema(schema_with = openapi_integer)]
    size_bytes: u64,
    #[schema(schema_with = openapi_datetime)]
    added_at: String,
    display_title: String,
    origin: String,
}

impl TryFrom<IndexedTrack> for TrackResponse {
    type Error = ApiError;

    fn try_from(track: IndexedTrack) -> Result<Self, Self::Error> {
        Ok(Self {
            id: track.id.get(),
            path: track.path.into_string(),
            title: track.metadata.title,
            artist: track.metadata.artist,
            album_artist: track.metadata.album_artist,
            album: track.metadata.album,
            track_no: track.metadata.track_no,
            disc_no: track.metadata.disc_no,
            year: track.metadata.year,
            genre: track.metadata.genre,
            length_s: track.duration.as_secs_f64(),
            bpm: track.metadata.bpm,
            size_bytes: track.size_bytes,
            added_at: crate::auth::format_rfc3339(UnixSeconds::new(track.added_at_unix_seconds))?,
            display_title: track.display_title,
            origin: track.origin,
        })
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = SearchResponse)]
struct SearchResponse {
    tracks: Vec<TrackResponse>,
    #[schema(schema_with = openapi_integer)]
    total: u64,
    #[schema(schema_with = openapi_integer)]
    limit: u16,
    #[schema(schema_with = openapi_integer)]
    offset: u64,
    sort: SearchSort,
    order: SearchOrder,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = TreeResponse)]
struct TreeResponse {
    path: String,
    tracks: Vec<TrackResponse>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = FolderOut)]
struct FolderResponse {
    name: String,
    path: String,
    #[schema(schema_with = openapi_integer)]
    track_count: u64,
    has_children: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = FoldersResponse)]
struct FoldersResponse {
    folders: Vec<FolderResponse>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = FolderCreateRequest)]
struct FolderCreateRequest {
    #[schema(min_length = 1)]
    path: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = FolderRenameRequest)]
struct FolderRenameRequest {
    #[schema(min_length = 1)]
    src: String,
    #[schema(min_length = 1)]
    dst: String,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[schema(as = FolderDeleteResult)]
struct FolderDeleteResponse {
    #[schema(schema_with = openapi_integer)]
    removed_tracks: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = TrackMoveRequest)]
struct TrackMoveRequest {
    destination: String,
    #[serde(default)]
    #[schema(schema_with = openapi_nullable_string)]
    new_filename: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = BulkMoveRequest)]
struct BulkMoveRequest {
    #[schema(schema_with = bounded_track_id_array_schema)]
    track_ids: Vec<i64>,
    destination: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = BulkDeleteRequest)]
struct BulkDeleteRequest {
    #[schema(schema_with = bounded_track_id_array_schema)]
    track_ids: Vec<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = BulkActionSkip)]
struct BulkActionSkipResponse {
    #[schema(schema_with = openapi_integer)]
    track_id: i64,
    reason: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = BulkMoveResult)]
struct BulkMoveResponse {
    moved: Vec<TrackResponse>,
    skipped: Vec<BulkActionSkipResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = BulkDeleteResult)]
struct BulkDeleteResponse {
    #[schema(value_type = Vec<i128>)]
    deleted_ids: Vec<i64>,
    skipped: Vec<BulkActionSkipResponse>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum UploadConflictQuery {
    #[default]
    Rename,
    Overwrite,
    Skip,
}

impl From<UploadConflictQuery> for UploadConflictPolicy {
    fn from(value: UploadConflictQuery) -> Self {
        match value {
            UploadConflictQuery::Rename => Self::Rename,
            UploadConflictQuery::Overwrite => Self::Overwrite,
            UploadConflictQuery::Skip => Self::Skip,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct UploadQuery {
    /// Destination folder under MUSIC_DIR.
    #[serde(default = "default_upload_destination")]
    #[param(default = "Uploads", required = false)]
    dest: String,
    /// Policy for existing files.
    #[serde(default)]
    #[param(schema_with = upload_conflict_schema, required = false)]
    conflict: UploadConflictQuery,
}

fn upload_conflict_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["rename", "overwrite", "skip"]))
        .default(Some(serde_json::json!("rename")))
        .into()
}

fn default_upload_destination() -> String {
    "Uploads".to_owned()
}

#[derive(Debug, ToSchema)]
#[schema(as = Body_upload_api_library_upload_post)]
#[allow(dead_code)]
struct LibraryUploadMultipartBody {
    #[schema(schema_with = multipart_upload_files_schema)]
    files: Vec<String>,
}

fn multipart_upload_files_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(
            ObjectBuilder::new()
                .schema_type(Type::String)
                .content_media_type("application/octet-stream"),
        )
        .into()
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = UploadResult)]
struct UploadResponse {
    saved: Vec<TrackResponse>,
    destination: String,
    #[schema(required = false)]
    skipped: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[schema(as = UploadCheckItem)]
struct UploadCheckItemRequest {
    dest: String,
    name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = UploadCheckRequest)]
struct UploadCheckRequest {
    items: Vec<UploadCheckItemRequest>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = UploadCheckResponse)]
struct UploadCheckResponse {
    collisions: Vec<UploadCheckItemRequest>,
}

#[derive(Debug, Clone, Default)]
enum MetadataUpdateValue<T> {
    #[default]
    Unset,
    Set(Option<T>),
}

impl<'de, T> Deserialize<'de> for MetadataUpdateValue<T>
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

#[derive(Debug, Default, Deserialize, ToSchema)]
#[schema(as = TrackMetadataUpdate)]
struct TrackMetadataUpdateRequest {
    #[serde(default)]
    #[schema(required = false, schema_with = metadata_nullable_string_schema)]
    title: MetadataUpdateValue<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = metadata_nullable_string_schema)]
    artist: MetadataUpdateValue<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = metadata_nullable_string_schema)]
    album_artist: MetadataUpdateValue<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = metadata_nullable_string_schema)]
    album: MetadataUpdateValue<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = metadata_nullable_integer_schema)]
    track_no: MetadataUpdateValue<u32>,
    #[serde(default)]
    #[schema(required = false, schema_with = metadata_nullable_integer_schema)]
    disc_no: MetadataUpdateValue<u32>,
    #[serde(default)]
    #[schema(required = false, schema_with = metadata_nullable_integer_schema)]
    year: MetadataUpdateValue<u32>,
    #[serde(default)]
    #[schema(required = false, schema_with = metadata_nullable_genre_schema)]
    genre: MetadataUpdateValue<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = metadata_nullable_integer_schema)]
    bpm: MetadataUpdateValue<u32>,
    #[serde(default)]
    #[schema(required = false, schema_with = metadata_nullable_string_schema)]
    display_title: MetadataUpdateValue<String>,
    #[serde(default)]
    #[schema(required = false, schema_with = metadata_nullable_string_schema)]
    origin: MetadataUpdateValue<String>,
}

impl TrackMetadataUpdateRequest {
    fn into_patch(self) -> Result<TrackMetadataPatch, ApiError> {
        let mut patch = TrackMetadataPatch::new();
        insert_text_update(&mut patch, TrackMetadataField::Title, self.title)?;
        insert_text_update(&mut patch, TrackMetadataField::Artist, self.artist)?;
        insert_text_update(
            &mut patch,
            TrackMetadataField::AlbumArtist,
            self.album_artist,
        )?;
        insert_text_update(&mut patch, TrackMetadataField::Album, self.album)?;
        insert_number_update(&mut patch, TrackMetadataField::TrackNumber, self.track_no)?;
        insert_number_update(&mut patch, TrackMetadataField::DiscNumber, self.disc_no)?;
        insert_number_update(&mut patch, TrackMetadataField::Year, self.year)?;
        insert_text_update(&mut patch, TrackMetadataField::Genre, self.genre)?;
        insert_number_update(&mut patch, TrackMetadataField::Bpm, self.bpm)?;
        insert_text_update(
            &mut patch,
            TrackMetadataField::DisplayTitle,
            self.display_title,
        )?;
        insert_text_update(&mut patch, TrackMetadataField::Origin, self.origin)?;
        Ok(patch)
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = BulkMetadataUpdate)]
struct BulkMetadataUpdateRequest {
    #[schema(schema_with = bounded_track_id_array_schema)]
    track_ids: Vec<i64>,
    updates: TrackMetadataUpdateRequest,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = BulkMetadataSkip)]
struct BulkMetadataSkipResponse {
    #[schema(schema_with = openapi_integer)]
    track_id: i64,
    reason: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = BulkMetadataResult)]
struct BulkMetadataResponse {
    updated: Vec<TrackResponse>,
    skipped: Vec<BulkMetadataSkipResponse>,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[schema(as = RescanResult)]
struct RescanResponse {
    #[schema(schema_with = openapi_integer)]
    added: u64,
    #[schema(schema_with = openapi_integer)]
    updated: u64,
    #[schema(schema_with = openapi_integer)]
    removed: u64,
    #[schema(schema_with = openapi_integer)]
    unchanged: u64,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum SearchSort {
    Title,
    #[default]
    Artist,
    Album,
    AlbumArtist,
    Year,
    LengthS,
    TrackNo,
    AddedAt,
    Path,
}

impl From<SearchSort> for LibrarySortKey {
    fn from(value: SearchSort) -> Self {
        match value {
            SearchSort::Title => Self::Title,
            SearchSort::Artist => Self::Artist,
            SearchSort::Album => Self::Album,
            SearchSort::AlbumArtist => Self::AlbumArtist,
            SearchSort::Year => Self::Year,
            SearchSort::LengthS => Self::LengthSeconds,
            SearchSort::TrackNo => Self::TrackNumber,
            SearchSort::AddedAt => Self::AddedAt,
            SearchSort::Path => Self::Path,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum SearchOrder {
    #[default]
    Asc,
    Desc,
}

impl From<SearchOrder> for SortOrder {
    fn from(value: SearchOrder) -> Self {
        match value {
            SearchOrder::Asc => Self::Ascending,
            SearchOrder::Desc => Self::Descending,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct SearchQuery {
    #[serde(default)]
    #[param(default = "")]
    q: String,
    #[serde(default = "default_search_limit")]
    #[param(value_type = i128, default = 100, minimum = 1, maximum = 500)]
    limit: u16,
    #[serde(default)]
    #[param(value_type = i128, default = 0, minimum = 0)]
    offset: u64,
    #[serde(default)]
    #[param(schema_with = search_sort_parameter_schema)]
    sort: SearchSort,
    #[serde(default)]
    #[param(schema_with = search_order_parameter_schema)]
    order: SearchOrder,
}

fn search_sort_parameter_schema() -> RefOr<Schema> {
    Schema::Object(
        ObjectBuilder::new()
            .schema_type(Type::String)
            .enum_values(Some([
                "title",
                "artist",
                "album",
                "album_artist",
                "year",
                "length_s",
                "track_no",
                "added_at",
                "path",
            ]))
            .default(Some(serde_json::Value::String("artist".to_owned())))
            .build(),
    )
    .into()
}

fn search_order_parameter_schema() -> RefOr<Schema> {
    Schema::Object(
        ObjectBuilder::new()
            .schema_type(Type::String)
            .enum_values(Some(["asc", "desc"]))
            .default(Some(serde_json::Value::String("asc".to_owned())))
            .build(),
    )
    .into()
}

const fn default_search_limit() -> u16 {
    100
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct TreeQuery {
    #[param(default = "")]
    path: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct BatchQuery {
    ids: String,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct FolderDeleteQuery {
    /// Folder path relative to the configured music directory.
    path: String,
    /// Delete contents too; otherwise non-empty folders are refused.
    #[serde(default)]
    #[param(default = false)]
    recursive: bool,
}

fn bounded_track_id_array_schema() -> RefOr<Schema> {
    Schema::Array(
        ArrayBuilder::new()
            .items(openapi_integer())
            .min_items(Some(1))
            .max_items(Some(1000))
            .build(),
    )
    .into()
}

fn metadata_nullable_string_schema() -> RefOr<Schema> {
    metadata_nullable_text_schema(512)
}

fn metadata_nullable_genre_schema() -> RefOr<Schema> {
    metadata_nullable_text_schema(128)
}

fn metadata_nullable_text_schema(maximum: usize) -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .max_length(Some(maximum))
                    .build(),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null).build())
            .build(),
    )
    .into()
}

fn metadata_nullable_integer_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::Integer)
                    .minimum(Some(0.0_f64))
                    .maximum(Some(9_999.0_f64))
                    .build(),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null).build())
            .build(),
    )
    .into()
}

fn insert_text_update(
    patch: &mut TrackMetadataPatch,
    field: TrackMetadataField,
    update: MetadataUpdateValue<String>,
) -> Result<(), ApiError> {
    if let MetadataUpdateValue::Set(value) = update {
        patch
            .insert_text(field, value)
            .map_err(|_| ApiError::validation())?;
    }
    Ok(())
}

fn insert_number_update(
    patch: &mut TrackMetadataPatch,
    field: TrackMetadataField,
    update: MetadataUpdateValue<u32>,
) -> Result<(), ApiError> {
    if let MetadataUpdateValue::Set(value) = update {
        patch
            .insert_number(field, value)
            .map_err(|_| ApiError::validation())?;
    }
    Ok(())
}

#[utoipa::path(
    get,
    path = "/library/search",
    params(SearchQuery),
    responses(
        (status = 200, description = "Successful Response", body = SearchResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library"
)]
async fn search(
    State(state): State<HttpState>,
    headers: HeaderMap,
    query: Result<Query<SearchQuery>, QueryRejection>,
) -> Result<Json<SearchResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Query(query) = query.map_err(|_| ApiError::validation())?;
    let request = LibrarySearch::new(
        query.q,
        query.limit,
        query.offset,
        query.sort.into(),
        query.order.into(),
    )
    .map_err(|_| ApiError::validation())?;
    let library = library(&state)?;
    let result = library.service.search(&request).await.map_err(|error| {
        tracing::error!(error = %error, "library search failed");
        ApiError::internal()
    })?;
    let tracks = result
        .tracks
        .into_iter()
        .map(TrackResponse::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(SearchResponse {
        tracks,
        total: result.total,
        limit: request.limit,
        offset: request.offset,
        sort: query.sort,
        order: query.order,
    }))
}

#[utoipa::path(
    get,
    path = "/library/tree",
    params(TreeQuery),
    responses(
        (status = 200, description = "Successful Response", body = TreeResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library"
)]
async fn tree(
    State(state): State<HttpState>,
    headers: HeaderMap,
    query: Result<Query<TreeQuery>, QueryRejection>,
) -> Result<Json<TreeResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Query(query) = query.map_err(|_| ApiError::validation())?;
    let path = query.path.unwrap_or_default();
    let directory = if path.is_empty() {
        None
    } else {
        Some(LibraryPath::parse(path.clone()).map_err(|_| ApiError::validation())?)
    };
    let tracks = library(&state)?
        .service
        .tracks_in_directory(directory.as_ref())
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "library tree query failed");
            ApiError::internal()
        })?
        .into_iter()
        .map(TrackResponse::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(TreeResponse { path, tracks }))
}

#[utoipa::path(
    get,
    path = "/library/folders",
    responses((status = 200, description = "Successful Response", body = FoldersResponse)),
    tag = "library"
)]
async fn folders(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<FoldersResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let library = library(&state)?;
    let root = library.root.clone();
    let directories = tokio::task::spawn_blocking(move || list_library_directories(&root));
    let counts = library.service.folder_track_counts();
    let (directories, counts) = tokio::join!(directories, counts);
    let directories = directories
        .map_err(|error| {
            tracing::error!(error = %error, "library directory worker failed");
            ApiError::internal()
        })?
        .map_err(|error| {
            tracing::error!(error = %error, "library directory enumeration failed");
            ApiError::internal()
        })?;
    let counts = counts.map_err(|error| {
        tracing::error!(error = %error, "library folder count query failed");
        ApiError::internal()
    })?;
    Ok(Json(FoldersResponse {
        folders: directories
            .into_iter()
            .map(|directory| FolderResponse {
                track_count: counts.get(&directory.path).copied().unwrap_or_default(),
                name: directory.name,
                path: directory.path.into_string(),
                has_children: directory.has_children,
            })
            .collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/library/folders",
    request_body = FolderCreateRequest,
    responses(
        (status = 201, description = "Successful Response", body = FolderResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library"
)]
async fn create_folder(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<FolderCreateRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<FolderResponse>), ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if payload.path.is_empty() {
        return Err(ApiError::validation());
    }
    let path = LibraryPath::parse(payload.path)
        .map_err(|_| ApiError::bad_request("invalid folder path"))?;
    let folder = library(&state)?
        .coordinator
        .create_folder(path)
        .await
        .map_err(map_folder_mutation_error)?;
    Ok((StatusCode::CREATED, Json(folder_response(folder))))
}

#[utoipa::path(
    delete,
    path = "/library/folders",
    params(FolderDeleteQuery),
    responses(
        (status = 200, description = "Successful Response", body = FolderDeleteResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library"
)]
async fn delete_folder(
    State(state): State<HttpState>,
    headers: HeaderMap,
    query: Result<Query<FolderDeleteQuery>, QueryRejection>,
) -> Result<Json<FolderDeleteResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Query(query) = query.map_err(|_| ApiError::validation())?;
    let path =
        LibraryPath::parse(query.path).map_err(|_| ApiError::bad_request("invalid folder path"))?;
    let result = library(&state)?
        .coordinator
        .delete_folder(path, query.recursive)
        .await
        .map_err(map_folder_mutation_error)?;
    Ok(Json(FolderDeleteResponse {
        removed_tracks: result.removed_tracks,
    }))
}

#[utoipa::path(
    post,
    path = "/library/folders/rename",
    request_body = FolderRenameRequest,
    responses(
        (status = 200, description = "Successful Response", body = FolderResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library"
)]
async fn rename_folder(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<FolderRenameRequest>, JsonRejection>,
) -> Result<Json<FolderResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if payload.src.is_empty() || payload.dst.is_empty() {
        return Err(ApiError::validation());
    }
    let source = LibraryPath::parse(payload.src)
        .map_err(|_| ApiError::bad_request("invalid source folder path"))?;
    let destination = LibraryPath::parse(payload.dst)
        .map_err(|_| ApiError::bad_request("invalid destination folder path"))?;
    let folder = library(&state)?
        .coordinator
        .rename_folder(source, destination)
        .await
        .map_err(map_folder_mutation_error)?;
    Ok(Json(folder_response(folder)))
}

#[utoipa::path(
    get,
    path = "/library/tracks",
    params(BatchQuery),
    responses(
        (status = 200, description = "Successful Response", body = [TrackResponse]),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library"
)]
async fn tracks_batch(
    State(state): State<HttpState>,
    headers: HeaderMap,
    query: Result<Query<BatchQuery>, QueryRejection>,
) -> Result<Json<Vec<TrackResponse>>, ApiError> {
    let _ = crate::auth::optional_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Query(query) = query.map_err(|_| ApiError::validation())?;
    let mut ids = Vec::new();
    for token in query
        .ids
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        let raw = token.parse::<i64>().map_err(|_| ApiError::validation())?;
        if let Ok(track_id) = TrackId::new(raw)
            && !ids.contains(&track_id)
        {
            ids.push(track_id);
        }
    }
    if ids.len() > 500 {
        return Err(ApiError::validation());
    }
    let tracks = library(&state)?
        .service
        .tracks_by_ids(&ids)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "library batch query failed");
            ApiError::internal()
        })?
        .into_iter()
        .map(TrackResponse::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(tracks))
}

#[utoipa::path(
    patch,
    path = "/library/tracks/bulk-metadata",
    request_body = BulkMetadataUpdateRequest,
    responses(
        (status = 200, description = "Successful Response", body = BulkMetadataResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library"
)]
async fn bulk_update_metadata(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<BulkMetadataUpdateRequest>, JsonRejection>,
) -> Result<Json<BulkMetadataResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if !(1..=1000).contains(&payload.track_ids.len()) {
        return Err(ApiError::validation());
    }
    let patch = payload.updates.into_patch()?;
    if patch.is_empty() {
        return Err(ApiError::bad_request("no fields to update"));
    }

    let supplied_ids = payload
        .track_ids
        .into_iter()
        .filter_map(|raw| TrackId::new(raw).ok())
        .collect::<Vec<_>>();
    let library = library(&state)?;
    let matched = library
        .service
        .tracks_by_ids(&supplied_ids)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "bulk metadata track lookup failed");
            ApiError::internal()
        })?;
    if matched.is_empty() {
        return Err(ApiError::plain_not_found(
            "no tracks matched the supplied ids",
        ));
    }
    let matched_ids = matched.into_iter().map(|track| track.id).collect();
    let results = library
        .coordinator
        .update_tracks_metadata(matched_ids, patch)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "bulk metadata coordinator failed");
            ApiError::internal()
        })?;

    let mut updated = Vec::new();
    let mut skipped = Vec::new();
    for item in results {
        if let Some(track) = item.track {
            updated.push(track.try_into()?);
        }
        if let Some(error) = item.error {
            skipped.push(BulkMetadataSkipResponse {
                track_id: item.track_id.get(),
                reason: bulk_metadata_reason(&error),
            });
        }
    }
    Ok(Json(BulkMetadataResponse { updated, skipped }))
}

#[utoipa::path(
    post,
    path = "/library/tracks/bulk-move",
    request_body = BulkMoveRequest,
    responses(
        (status = 200, description = "Successful Response", body = BulkMoveResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library"
)]
async fn bulk_move_tracks(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<BulkMoveRequest>, JsonRejection>,
) -> Result<Json<BulkMoveResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if !(1..=1000).contains(&payload.track_ids.len()) {
        return Err(ApiError::validation());
    }
    if !payload.destination.is_empty() {
        LibraryPath::parse(payload.destination.clone())
            .map_err(|_| ApiError::bad_request("invalid destination folder"))?;
    }
    let library = library(&state)?;
    let mut requests = Vec::with_capacity(payload.track_ids.len());
    let mut skipped = Vec::new();
    for raw_track_id in payload.track_ids {
        let Ok(track_id) = TrackId::new(raw_track_id) else {
            skipped.push(BulkActionSkipResponse {
                track_id: raw_track_id,
                reason: "not found".to_owned(),
            });
            continue;
        };
        let Some(track) = library.service.track(track_id).await.map_err(|error| {
            tracing::error!(error = %error, "bulk move track lookup failed");
            ApiError::internal()
        })?
        else {
            skipped.push(BulkActionSkipResponse {
                track_id: raw_track_id,
                reason: "not found".to_owned(),
            });
            continue;
        };
        match track_destination(&track, &payload.destination, None) {
            Ok(destination) => requests.push((track_id, destination)),
            Err(_) => skipped.push(BulkActionSkipResponse {
                track_id: raw_track_id,
                reason: "invalid destination path".to_owned(),
            }),
        }
    }
    let results = library
        .coordinator
        .move_tracks(requests)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "bulk track move coordinator failed");
            ApiError::internal()
        })?;
    let mut moved = Vec::new();
    for (track_id, result) in results {
        match result {
            Ok(track) => moved.push(track.try_into()?),
            Err(error) => skipped.push(BulkActionSkipResponse {
                track_id: track_id.get(),
                reason: bulk_mutation_reason(&error),
            }),
        }
    }
    Ok(Json(BulkMoveResponse { moved, skipped }))
}

#[utoipa::path(
    post,
    path = "/library/tracks/bulk-delete",
    request_body = BulkDeleteRequest,
    responses(
        (status = 200, description = "Successful Response", body = BulkDeleteResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library"
)]
async fn bulk_delete_tracks(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<BulkDeleteRequest>, JsonRejection>,
) -> Result<Json<BulkDeleteResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if !(1..=1000).contains(&payload.track_ids.len()) {
        return Err(ApiError::validation());
    }
    let mut track_ids = Vec::with_capacity(payload.track_ids.len());
    let mut skipped = Vec::new();
    for raw_track_id in payload.track_ids {
        match TrackId::new(raw_track_id) {
            Ok(track_id) => track_ids.push(track_id),
            Err(_) => skipped.push(BulkActionSkipResponse {
                track_id: raw_track_id,
                reason: "not found".to_owned(),
            }),
        }
    }
    let results = library(&state)?
        .coordinator
        .delete_tracks(track_ids)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "bulk track deletion coordinator failed");
            ApiError::internal()
        })?;
    let mut deleted_ids = Vec::new();
    for (track_id, result) in results {
        match result {
            Ok(()) => deleted_ids.push(track_id.get()),
            Err(error) => skipped.push(BulkActionSkipResponse {
                track_id: track_id.get(),
                reason: bulk_mutation_reason(&error),
            }),
        }
    }
    Ok(Json(BulkDeleteResponse {
        deleted_ids,
        skipped,
    }))
}

#[utoipa::path(
    get,
    path = "/library/tracks/{track_id}",
    params(("track_id" = i128, Path, description = "Track identifier")),
    responses(
        (status = 200, description = "Successful Response", body = TrackResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library"
)]
async fn track(
    State(state): State<HttpState>,
    headers: HeaderMap,
    track_id: Result<Path<i64>, PathRejection>,
) -> Result<Json<TrackResponse>, ApiError> {
    let _ = crate::auth::optional_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Path(raw_track_id) = track_id.map_err(|_| ApiError::validation())?;
    let track = indexed_track(&state, raw_track_id).await?;
    Ok(Json(track.try_into()?))
}

#[utoipa::path(
    patch,
    path = "/library/tracks/{track_id}/metadata",
    params(("track_id" = i128, Path, description = "Track identifier")),
    request_body = TrackMetadataUpdateRequest,
    responses(
        (status = 200, description = "Successful Response", body = TrackResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library"
)]
async fn update_metadata(
    State(state): State<HttpState>,
    headers: HeaderMap,
    track_id: Result<Path<i64>, PathRejection>,
    payload: Result<Json<TrackMetadataUpdateRequest>, JsonRejection>,
) -> Result<Json<TrackResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Path(raw_track_id) = track_id.map_err(|_| ApiError::validation())?;
    let track_id =
        TrackId::new(raw_track_id).map_err(|_| ApiError::plain_not_found("track not found"))?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let track = library(&state)?
        .coordinator
        .update_track_metadata(track_id, payload.into_patch()?)
        .await
        .map_err(map_track_metadata_error)?;
    Ok(Json(track.try_into()?))
}

#[utoipa::path(
    post,
    path = "/library/tracks/{track_id}/move",
    params(("track_id" = i128, Path, description = "Track identifier")),
    request_body = TrackMoveRequest,
    responses(
        (status = 200, description = "Successful Response", body = TrackResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library"
)]
async fn move_track(
    State(state): State<HttpState>,
    headers: HeaderMap,
    track_id: Result<Path<i64>, PathRejection>,
    payload: Result<Json<TrackMoveRequest>, JsonRejection>,
) -> Result<Json<TrackResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Path(raw_track_id) = track_id.map_err(|_| ApiError::validation())?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let current = indexed_track(&state, raw_track_id).await?;
    let destination = track_destination(
        &current,
        &payload.destination,
        payload.new_filename.as_deref(),
    )?;
    let moved = library(&state)?
        .coordinator
        .move_track(current.id, destination)
        .await
        .map_err(map_track_move_error)?;
    Ok(Json(moved.try_into()?))
}

#[utoipa::path(
    delete,
    path = "/library/tracks/{track_id}",
    params(("track_id" = i128, Path, description = "Track identifier")),
    responses(
        (status = 204, description = "Successful Response"),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library"
)]
async fn delete_track(
    State(state): State<HttpState>,
    headers: HeaderMap,
    track_id: Result<Path<i64>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Path(raw_track_id) = track_id.map_err(|_| ApiError::validation())?;
    let track_id =
        TrackId::new(raw_track_id).map_err(|_| ApiError::plain_not_found("track not found"))?;
    library(&state)?
        .coordinator
        .delete_track(track_id)
        .await
        .map_err(map_track_delete_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/library/tracks/{track_id}/stream",
    params(("track_id" = i128, Path, description = "Track identifier")),
    responses(
        (status = 200, description = "Successful Response", body = serde_json::Value),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library"
)]
async fn stream(
    State(state): State<HttpState>,
    track_id: Result<Path<i64>, PathRejection>,
    request: Request,
) -> Result<Response, ApiError> {
    let _ = crate::auth::optional_session(&state, request.headers(), SessionTouch::UpdateLastSeen)
        .await?;
    let Path(raw_track_id) = track_id.map_err(|_| ApiError::validation())?;
    let track = indexed_track(&state, raw_track_id).await?;
    let library = library(&state)?;
    let root = library.root.clone();
    let path = track.path;
    let absolute = tokio::task::spawn_blocking(move || resolve_library_media_file(&root, &path))
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "media path worker failed");
            ApiError::internal()
        })?
        .map_err(|error| map_stream_delivery_error(raw_track_id, &error))?;

    let response = ServeFile::new(absolute)
        .oneshot(request)
        .await
        .map_err(|never| match never {})?;
    if response.status() == StatusCode::NOT_FOUND {
        return Err(ApiError::gone("track file missing"));
    }
    if response.status() == StatusCode::INTERNAL_SERVER_ERROR {
        tracing::error!(
            track_id = raw_track_id,
            "media stream failed after path validation"
        );
        return Err(ApiError::internal());
    }
    let mut response = response.map(Body::new);
    response
        .headers_mut()
        .insert(CONTENT_DISPOSITION, HeaderValue::from_static("inline"));
    Ok(response)
}

#[utoipa::path(
    get,
    path = "/library/tracks/{track_id}/cover",
    params(("track_id" = i128, Path, description = "Track identifier")),
    responses(
        (status = 200, description = "Successful Response", body = serde_json::Value),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library"
)]
async fn cover(
    State(state): State<HttpState>,
    headers: HeaderMap,
    track_id: Result<Path<i64>, PathRejection>,
) -> Result<Response, ApiError> {
    let _ = crate::auth::optional_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Path(raw_track_id) = track_id.map_err(|_| ApiError::validation())?;
    let track = indexed_track(&state, raw_track_id).await?;
    let library = library(&state)?;
    let root = library.root.clone();
    let metadata = library.metadata.clone();
    let path = track.path;
    let artwork =
        tokio::task::spawn_blocking(move || read_library_cover_art(&root, &path, &metadata))
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "cover art worker failed");
                ApiError::internal()
            })?
            .map_err(|error| {
                if error.is_unavailable() {
                    ApiError::plain_not_found("no cover art")
                } else {
                    tracing::error!(
                        track_id = raw_track_id,
                        error_code = error.code(),
                        "cover art extraction failed"
                    );
                    ApiError::internal()
                }
            })?
            .ok_or_else(|| ApiError::plain_not_found("no cover art"))?;

    let mut response = artwork.bytes.into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static(safe_cover_mime(&artwork.mime_type)),
    );
    response
        .headers_mut()
        .insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response.headers_mut().insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    Ok(response)
}

#[utoipa::path(
    post,
    path = "/library/upload",
    params(UploadQuery),
    request_body(content = LibraryUploadMultipartBody, content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "Successful Response", body = UploadResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library"
)]
async fn upload(
    State(state): State<HttpState>,
    headers: HeaderMap,
    query: Result<Query<UploadQuery>, QueryRejection>,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<(StatusCode, Json<UploadResponse>), ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Query(query) = query.map_err(|_| ApiError::validation())?;
    let mut multipart = multipart.map_err(|_| ApiError::validation())?;
    let library = library(&state)?;
    let directory = upload_directory(&query.dest)?;
    if let Some(directory) = directory.clone() {
        library
            .coordinator
            .create_folder(directory)
            .await
            .map_err(map_folder_mutation_error)?;
    }

    let mut staged = Vec::new();
    while let Some(field) = multipart.next_field().await.map_err(map_multipart_error)? {
        if field.name() != Some("files") {
            continue;
        }
        let Some(file_name) = field.file_name().map(str::to_owned) else {
            continue;
        };
        if file_name.is_empty() {
            continue;
        }
        if staged.len() >= library.max_upload_files {
            return Err(ApiError::payload_too_large(
                "too many files in one upload request",
            ));
        }
        let requested = upload_destination(directory.as_ref(), &file_name)?;
        staged.push(
            stage_upload_field(
                &library.root,
                requested,
                field,
                library.max_upload_file_bytes,
            )
            .await?,
        );
    }
    if staged.is_empty() {
        return Err(ApiError::bad_request("no files provided"));
    }

    let uploads = staged
        .into_iter()
        .map(StagedUpload::transfer)
        .collect::<Vec<_>>();
    let results = library
        .coordinator
        .publish_uploads(uploads, query.conflict.into())
        .await
        .map_err(map_upload_error)?;
    let mut saved = Vec::new();
    let mut skipped = Vec::new();
    for result in results {
        match result {
            LibraryUploadBatchItem::Published {
                track: Some(track), ..
            } => saved.push((*track).try_into()?),
            LibraryUploadBatchItem::Published { track: None, .. } => {}
            LibraryUploadBatchItem::Skipped { requested } => {
                skipped.push(requested.file_name().to_owned());
            }
        }
    }
    Ok((
        StatusCode::CREATED,
        Json(UploadResponse {
            saved,
            destination: directory.map_or_else(String::new, LibraryPath::into_string),
            skipped,
        }),
    ))
}

#[utoipa::path(
    post,
    path = "/library/upload/check",
    request_body = UploadCheckRequest,
    responses(
        (status = 200, description = "Successful Response", body = UploadCheckResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library"
)]
async fn upload_check(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<UploadCheckRequest>, JsonRejection>,
) -> Result<Json<UploadCheckResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let root = library(&state)?.root.clone();
    let collisions = tokio::task::spawn_blocking(move || {
        payload
            .items
            .into_iter()
            .filter(|item| {
                let Ok(directory) = upload_directory(&item.dest) else {
                    return false;
                };
                let Ok(path) = upload_destination(directory.as_ref(), &item.name) else {
                    return false;
                };
                library_upload_target_exists(&root, &path).unwrap_or(false)
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "upload collision worker failed");
        ApiError::internal()
    })?;
    Ok(Json(UploadCheckResponse { collisions }))
}

#[utoipa::path(
    post,
    path = "/library/rescan",
    responses((status = 200, description = "Successful Response", body = RescanResponse)),
    tag = "library"
)]
async fn rescan(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<RescanResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let summary = library(&state)?
        .coordinator
        .reconcile()
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "library reconciliation request failed");
            ApiError::internal()
        })?;
    Ok(Json(RescanResponse {
        added: summary.added,
        updated: summary.updated,
        removed: summary.removed,
        unchanged: summary.unchanged,
    }))
}

fn folder_response(folder: FolderMutationResult) -> FolderResponse {
    FolderResponse {
        name: folder.path.file_name().to_owned(),
        path: folder.path.into_string(),
        track_count: 0,
        has_children: folder.has_children,
    }
}

struct StagedUpload {
    requested: LibraryPath,
    staged: LibraryPath,
    absolute: Option<PathBuf>,
}

impl StagedUpload {
    fn transfer(mut self) -> StagedLibraryUpload {
        self.absolute = None;
        StagedLibraryUpload {
            staged: self.staged.clone(),
            requested: self.requested.clone(),
        }
    }
}

impl Drop for StagedUpload {
    fn drop(&mut self) {
        if let Some(path) = self.absolute.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn upload_directory(value: &str) -> Result<Option<LibraryPath>, ApiError> {
    if value.is_empty() {
        Ok(None)
    } else {
        LibraryPath::parse(value.to_owned())
            .map(Some)
            .map_err(|_| ApiError::bad_request("invalid upload destination"))
    }
}

fn upload_destination(
    directory: Option<&LibraryPath>,
    file_name: &str,
) -> Result<LibraryPath, ApiError> {
    if file_name.contains(['/', '\\']) {
        return Err(ApiError::bad_request("invalid upload filename"));
    }
    directory.map_or_else(
        || {
            LibraryPath::parse(file_name.to_owned())
                .map_err(|_| ApiError::bad_request("invalid upload filename"))
        },
        |directory| {
            directory
                .join(file_name)
                .map_err(|_| ApiError::bad_request("invalid upload filename"))
        },
    )
}

fn upload_stage_path(requested: &LibraryPath) -> Result<LibraryPath, ApiError> {
    let name = format!(".upload-{}.partial", Uuid::new_v4().simple());
    requested.parent().map_or_else(
        || LibraryPath::parse(&name).map_err(|_| ApiError::bad_request("upload path is too long")),
        |parent| {
            parent
                .join(&name)
                .map_err(|_| ApiError::bad_request("upload path is too long"))
        },
    )
}

async fn stage_upload_field(
    root: &LibraryRoot,
    requested: LibraryPath,
    mut field: Field<'_>,
    max_bytes: u64,
) -> Result<StagedUpload, ApiError> {
    let staged = upload_stage_path(&requested)?;
    let absolute = root.resolve_for_creation(&staged).map_err(|error| {
        tracing::error!(error = %error, "upload staging path resolution failed");
        ApiError::internal()
    })?;
    let guard = StagedUpload {
        requested,
        staged,
        absolute: Some(absolute.clone()),
    };
    let mut output = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&absolute)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "upload staging file creation failed");
            ApiError::internal()
        })?;
    let mut written = 0_u64;
    while let Some(chunk) = field.chunk().await.map_err(map_multipart_error)? {
        written = written
            .checked_add(u64::try_from(chunk.len()).map_err(|_| {
                ApiError::payload_too_large("upload file exceeds the configured size limit")
            })?)
            .ok_or_else(|| {
                ApiError::payload_too_large("upload file exceeds the configured size limit")
            })?;
        if written > max_bytes {
            return Err(ApiError::payload_too_large(
                "upload file exceeds the configured size limit",
            ));
        }
        output.write_all(&chunk).await.map_err(|error| {
            tracing::error!(error = %error, "upload staging write failed");
            ApiError::internal()
        })?;
    }
    output.flush().await.map_err(|error| {
        tracing::error!(error = %error, "upload staging flush failed");
        ApiError::internal()
    })?;
    output.sync_all().await.map_err(|error| {
        tracing::error!(error = %error, "upload staging synchronization failed");
        ApiError::internal()
    })?;
    drop(output);
    Ok(guard)
}

fn map_multipart_error(error: MultipartError) -> ApiError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::payload_too_large("upload request exceeds the configured size limit")
    } else {
        ApiError::validation()
    }
}

fn map_upload_error(error: LibraryCoordinatorError) -> ApiError {
    match error {
        LibraryCoordinatorError::Mutation(failure) => match failure.kind() {
            LibraryMutationFailureKind::NotFound => {
                tracing::error!(error = %failure, "staged library upload disappeared");
                ApiError::internal()
            }
            LibraryMutationFailureKind::Conflict | LibraryMutationFailureKind::NotEmpty => {
                ApiError::conflict("upload destination changed during publication")
            }
            LibraryMutationFailureKind::Invalid => {
                ApiError::bad_request("invalid upload destination")
            }
            LibraryMutationFailureKind::Io => {
                tracing::error!(error = %failure, "library upload publication failed");
                ApiError::internal()
            }
        },
        other => {
            tracing::error!(error = %other, "library upload coordinator failed");
            ApiError::internal()
        }
    }
}

fn track_destination(
    track: &IndexedTrack,
    directory: &str,
    new_filename: Option<&str>,
) -> Result<LibraryPath, ApiError> {
    let file_name = new_filename.unwrap_or_else(|| track.path.file_name());
    let leaf = LibraryPath::parse(file_name.to_owned())
        .map_err(|_| ApiError::bad_request("invalid track filename"))?;
    if leaf.parent().is_some() {
        return Err(ApiError::bad_request("invalid track filename"));
    }
    if directory.is_empty() {
        return Ok(leaf);
    }
    let directory = LibraryPath::parse(directory.to_owned())
        .map_err(|_| ApiError::bad_request("invalid destination folder"))?;
    directory
        .join(file_name)
        .map_err(|_| ApiError::bad_request("invalid destination path"))
}

fn map_track_move_error(error: LibraryCoordinatorError) -> ApiError {
    match error {
        LibraryCoordinatorError::TrackNotFound { .. } => {
            ApiError::plain_not_found("track not found")
        }
        LibraryCoordinatorError::Mutation(failure) => match failure.kind() {
            LibraryMutationFailureKind::NotFound => ApiError::gone("source file missing"),
            LibraryMutationFailureKind::Conflict => {
                ApiError::conflict("a file already exists at the destination")
            }
            LibraryMutationFailureKind::Invalid | LibraryMutationFailureKind::NotEmpty => {
                ApiError::bad_request("invalid track destination")
            }
            LibraryMutationFailureKind::Io => {
                tracing::error!(error = %failure, "library track move failed");
                ApiError::internal()
            }
        },
        other => {
            tracing::error!(error = %other, "library track move coordinator failed");
            ApiError::internal()
        }
    }
}

fn map_track_delete_error(error: LibraryCoordinatorError) -> ApiError {
    match error {
        LibraryCoordinatorError::TrackNotFound { .. } => {
            ApiError::plain_not_found("track not found")
        }
        LibraryCoordinatorError::Mutation(failure) => {
            tracing::error!(error = %failure, "library track deletion failed");
            ApiError::internal()
        }
        other => {
            tracing::error!(error = %other, "library track deletion coordinator failed");
            ApiError::internal()
        }
    }
}

fn map_track_metadata_error(error: LibraryCoordinatorError) -> ApiError {
    match error {
        LibraryCoordinatorError::TrackNotFound { .. } => {
            ApiError::plain_not_found("track not found")
        }
        LibraryCoordinatorError::Mutation(failure) => match failure.kind() {
            LibraryMutationFailureKind::NotFound => ApiError::gone("track file missing"),
            LibraryMutationFailureKind::Invalid => {
                ApiError::bad_request("unsupported metadata format")
            }
            LibraryMutationFailureKind::Conflict | LibraryMutationFailureKind::NotEmpty => {
                ApiError::conflict("metadata update conflicts with another file operation")
            }
            LibraryMutationFailureKind::Io => {
                tracing::error!(error = %failure, "library metadata update failed");
                ApiError::internal()
            }
        },
        other => {
            tracing::error!(error = %other, "library metadata coordinator failed");
            ApiError::internal()
        }
    }
}

fn bulk_metadata_reason(error: &LibraryCoordinatorError) -> String {
    match error {
        LibraryCoordinatorError::TrackNotFound { .. } => "not found",
        LibraryCoordinatorError::Mutation(failure) => match failure.kind() {
            LibraryMutationFailureKind::NotFound => "file missing on disk",
            LibraryMutationFailureKind::Invalid => "unsupported format",
            LibraryMutationFailureKind::Conflict => "metadata update conflict",
            LibraryMutationFailureKind::NotEmpty => "metadata target is not writable",
            LibraryMutationFailureKind::Io => "tag write failed",
        },
        LibraryCoordinatorError::RecoveryConflict => "batch stopped for recovery",
        _ => "metadata update failed",
    }
    .to_owned()
}

fn bulk_mutation_reason(error: &LibraryCoordinatorError) -> String {
    match error {
        LibraryCoordinatorError::TrackNotFound { .. } => "not found",
        LibraryCoordinatorError::Mutation(failure) => match failure.kind() {
            LibraryMutationFailureKind::NotFound => "source file missing",
            LibraryMutationFailureKind::Conflict => "destination already exists",
            LibraryMutationFailureKind::Invalid => "invalid media path",
            LibraryMutationFailureKind::NotEmpty => "target is not empty",
            LibraryMutationFailureKind::Io => "filesystem operation failed",
        },
        LibraryCoordinatorError::RecoveryConflict => "batch stopped for recovery",
        _ => "mutation failed",
    }
    .to_owned()
}

fn map_folder_mutation_error(error: LibraryCoordinatorError) -> ApiError {
    match error {
        LibraryCoordinatorError::Mutation(failure) => match failure.kind() {
            LibraryMutationFailureKind::NotFound => ApiError::plain_not_found("folder not found"),
            LibraryMutationFailureKind::Conflict => {
                ApiError::conflict("destination folder already exists")
            }
            LibraryMutationFailureKind::NotEmpty => {
                ApiError::bad_request("folder is not empty (pass recursive=true to force)")
            }
            LibraryMutationFailureKind::Invalid => ApiError::bad_request("invalid folder path"),
            LibraryMutationFailureKind::Io => {
                tracing::error!(error = %failure, "library folder mutation failed");
                ApiError::internal()
            }
        },
        other => {
            tracing::error!(error = %other, "library folder coordinator failed");
            ApiError::internal()
        }
    }
}

pub(crate) fn library(state: &HttpState) -> Result<&RuntimeLibrary, ApiError> {
    state
        .library
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)
}

async fn indexed_track(state: &HttpState, raw_track_id: i64) -> Result<IndexedTrack, ApiError> {
    let track_id =
        TrackId::new(raw_track_id).map_err(|_| ApiError::plain_not_found("track not found"))?;
    library(state)?
        .service
        .track(track_id)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "library track query failed");
            ApiError::internal()
        })?
        .ok_or_else(|| ApiError::plain_not_found("track not found"))
}

fn map_stream_delivery_error(track_id: i64, error: &MediaDeliveryError) -> ApiError {
    if error.is_unavailable() {
        ApiError::gone("track file missing")
    } else {
        tracing::error!(
            track_id,
            error_code = error.code(),
            "media stream path validation failed"
        );
        ApiError::internal()
    }
}

fn safe_cover_mime(candidate: &str) -> &'static str {
    match candidate {
        "image/jpeg" => "image/jpeg",
        "image/png" => "image/png",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        "image/avif" => "image/avif",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::safe_cover_mime;

    #[test]
    fn hostile_cover_mime_is_never_rendered_as_active_content() {
        assert_eq!(safe_cover_mime("image/png"), "image/png");
        assert_eq!(safe_cover_mime("text/html"), "application/octet-stream");
        assert_eq!(safe_cover_mime("image/svg+xml"), "application/octet-stream");
    }
}
