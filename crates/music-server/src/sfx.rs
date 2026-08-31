use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::multipart::{Field, MultipartError, MultipartRejection};
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Multipart, Query, Request, State};
use axum::http::header::{CONTENT_DISPOSITION, HeaderValue};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use music_application::auth::{SessionTouch, UnixSeconds};
use music_application::modes::ModeCatalog;
use music_application::sfx::{
    SfxCoordinator, SfxCoordinatorError, SfxFileRecord, SfxFolderRecord, SfxMutationFailureKind,
    SfxUploadBatchItem, SfxUploadConflictPolicy, StagedSfxUpload,
};
use music_domain::SfxPath;
use music_media::{RootedPathError, SfxRoot, sfx_upload_target_exists};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{ArrayBuilder, ObjectBuilder, Schema, Type};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::auth::format_rfc3339;
use crate::blocking::{BlockingMediaError, BlockingMediaExecutor};
use crate::error::{
    ApiError, HttpValidationErrorBody, openapi_datetime, openapi_integer, openapi_nullable_string,
};
use crate::http::HttpState;

#[derive(Debug)]
pub(crate) struct RuntimeSfx {
    pub(crate) coordinator: Arc<SfxCoordinator>,
    pub(crate) root: SfxRoot,
    pub(crate) max_upload_files: usize,
    pub(crate) max_upload_file_bytes: u64,
    pub(crate) media_executor: BlockingMediaExecutor,
}

pub(crate) fn sfx_router() -> OpenApiRouter<HttpState> {
    let upload = OpenApiRouter::default()
        .routes(routes!(upload))
        .layer(DefaultBodyLimit::disable());
    OpenApiRouter::default()
        .routes(routes!(get_sfx_file))
        .routes(routes!(list_all_files))
        .routes(routes!(delete_file))
        .routes(routes!(list_all_folders))
        .routes(routes!(create_folder))
        .routes(routes!(delete_folder))
        .routes(routes!(rename_folder))
        .routes(routes!(move_file))
        .routes(routes!(list_tree))
        .merge(upload)
        .routes(routes!(upload_check))
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = SfxFileOut)]
struct SfxFileResponse {
    name: String,
    path: String,
    #[schema(schema_with = openapi_integer)]
    size_bytes: u64,
    #[schema(schema_with = openapi_datetime)]
    modified_at: String,
    /// True iff some loaded soundboard has an item with this file path.
    referenced: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = SfxFolderOut)]
struct SfxFolderResponse {
    name: String,
    path: String,
    #[schema(schema_with = openapi_integer)]
    file_count: u64,
    has_children: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = SfxTreeResponse)]
struct SfxTreeResponse {
    path: String,
    files: Vec<SfxFileResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = SfxFoldersResponse)]
struct SfxFoldersResponse {
    folders: Vec<SfxFolderResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = SfxUploadResult)]
struct SfxUploadResponse {
    saved: Vec<SfxFileResponse>,
    destination: String,
    #[schema(required = false)]
    skipped: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[schema(as = UploadCheckItem)]
struct UploadCheckItem {
    dest: String,
    name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = UploadCheckRequest)]
struct UploadCheckRequest {
    items: Vec<UploadCheckItem>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = UploadCheckResponse)]
struct UploadCheckResponse {
    collisions: Vec<UploadCheckItem>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = SfxMoveRequest)]
struct SfxMoveRequest {
    /// Source path relative to SFX_LIBRARY_DIR.
    #[schema(min_length = 1)]
    src: String,
    /// Destination folder; '' for root.
    dst_folder: String,
    #[serde(default)]
    #[schema(required = false, schema_with = openapi_nullable_string)]
    new_filename: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = app__api__sfx__FolderCreateRequest)]
struct FolderCreateRequest {
    /// Folder path relative to SFX_LIBRARY_DIR.
    #[schema(min_length = 1)]
    path: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = app__api__sfx__FolderRenameRequest)]
struct FolderRenameRequest {
    #[schema(min_length = 1)]
    src: String,
    #[schema(min_length = 1)]
    dst: String,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[schema(as = app__api__sfx__FolderDeleteResult)]
struct FolderDeleteResponse {
    deleted: bool,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct FileQuery {
    /// Path relative to SFX_LIBRARY_DIR.
    path: String,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct TreeQuery {
    /// Folder path relative to SFX_LIBRARY_DIR.
    #[serde(default)]
    #[param(default = "", required = false)]
    path: String,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct DeleteFolderQuery {
    /// Folder path relative to SFX_LIBRARY_DIR.
    path: String,
    #[serde(default)]
    #[param(default = false, required = false)]
    recursive: bool,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum UploadConflictQuery {
    #[default]
    Rename,
    Overwrite,
    Skip,
}

impl From<UploadConflictQuery> for SfxUploadConflictPolicy {
    fn from(value: UploadConflictQuery) -> Self {
        match value {
            UploadConflictQuery::Rename => Self::Rename,
            UploadConflictQuery::Overwrite => Self::Overwrite,
            UploadConflictQuery::Skip => Self::Skip,
        }
    }
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct UploadQuery {
    /// Destination folder under SFX_LIBRARY_DIR.
    #[serde(default)]
    #[param(default = "", required = false)]
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

#[derive(Debug, ToSchema)]
#[schema(as = Body_upload_api_sfx_upload_post)]
#[allow(dead_code)]
struct SfxUploadMultipartBody {
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

#[utoipa::path(
    get,
    path = "/sfx/file",
    operation_id = "get_sfx_file_api_sfx_file_get",
    params(FileQuery),
    responses(
        (status = 200, description = "Successful Response", body = serde_json::Value),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "sfx"
)]
/// Stream a single SFX asset.
///
/// 1. Path must normalise without traversal segments.
/// 2. Path must resolve inside SFX_LIBRARY_DIR.
/// 3. Path must be referenced by at least one loaded mode's soundboards.
async fn get_sfx_file(
    State(state): State<HttpState>,
    query: Result<Query<FileQuery>, QueryRejection>,
    request: Request,
) -> Result<Response, ApiError> {
    let _ = crate::auth::optional_session(&state, request.headers(), SessionTouch::UpdateLastSeen)
        .await?;
    let Query(query) = query.map_err(|_| ApiError::validation())?;
    let path = parse_required_path(&query.path, "invalid path")?;
    let catalog = mode_catalog(&state);
    if !catalog
        .as_deref()
        .is_some_and(|catalog| catalog.references_sfx_path(path.as_str()))
    {
        return Err(ApiError::plain_not_found(
            "sfx path not referenced by any soundboard",
        ));
    }
    let root = sfx(&state)?.root.clone();
    let path_for_worker = path.clone();
    let absolute = sfx(&state)?
        .media_executor
        .execute(move || {
            let absolute = root.resolve_existing(&path_for_worker)?;
            Ok::<_, RootedPathError>(absolute.is_file().then_some(absolute))
        })
        .await
        .map_err(map_media_worker_error)?
        .map_err(map_sfx_delivery_error)?
        .ok_or_else(|| ApiError::gone("sfx file missing on disk"))?;

    let response = ServeFile::new(absolute)
        .oneshot(request)
        .await
        .map_err(|never| match never {})?;
    if response.status() == StatusCode::NOT_FOUND {
        return Err(ApiError::gone("sfx file missing on disk"));
    }
    if response.status() == StatusCode::INTERNAL_SERVER_ERROR {
        tracing::error!(path = %path, "SFX stream failed after path validation");
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
    path = "/sfx/files",
    operation_id = "list_all_files_api_sfx_files_get",
    responses((status = 200, description = "Successful Response", body = [SfxFileResponse])),
    tag = "sfx"
)]
/// Flat list of every SFX file, recursive. Used by the soundboard editor
/// to populate the file picker without round-tripping per folder.
async fn list_all_files(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<SfxFileResponse>>, ApiError> {
    authorize(&state, &headers).await?;
    let catalog = mode_catalog(&state);
    let files = sfx(&state)?
        .coordinator
        .list_files()
        .await
        .map_err(map_read_error)?;
    Ok(Json(file_responses(files, catalog.as_deref())?))
}

#[utoipa::path(
    delete,
    path = "/sfx/files",
    operation_id = "delete_file_api_sfx_files_delete",
    params(FileQuery),
    responses(
        (status = 204, description = "Successful Response"),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "sfx"
)]
async fn delete_file(
    State(state): State<HttpState>,
    headers: HeaderMap,
    query: Result<Query<FileQuery>, QueryRejection>,
) -> Result<StatusCode, ApiError> {
    authorize(&state, &headers).await?;
    let Query(query) = query.map_err(|_| ApiError::validation())?;
    let path = parse_required_path(&query.path, "invalid path")?;
    sfx(&state)?
        .coordinator
        .delete_file(path)
        .await
        .map_err(|error| {
            map_mutation_error(
                error,
                "file not found",
                "target already exists",
                "invalid path",
                "folder is not empty",
            )
        })?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/sfx/folders",
    operation_id = "list_all_folders_api_sfx_folders_get",
    responses((status = 200, description = "Successful Response", body = SfxFoldersResponse)),
    tag = "sfx"
)]
/// Whole folder hierarchy in one response — same contract as the music
/// library's GET /api/library/folders, for the client-side tree.
async fn list_all_folders(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<SfxFoldersResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let folders = sfx(&state)?
        .coordinator
        .list_folders()
        .await
        .map_err(map_read_error)?;
    Ok(Json(SfxFoldersResponse {
        folders: folders.into_iter().map(folder_response).collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/sfx/folders",
    operation_id = "create_folder_api_sfx_folders_post",
    request_body = FolderCreateRequest,
    responses(
        (status = 201, description = "Successful Response", body = SfxFolderResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "sfx"
)]
async fn create_folder(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<FolderCreateRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<SfxFolderResponse>), ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if payload.path.is_empty() {
        return Err(ApiError::validation());
    }
    let path = parse_required_folder(&payload.path)?;
    let folder = sfx(&state)?
        .coordinator
        .create_folder(path)
        .await
        .map_err(|error| {
            map_mutation_error(
                error,
                "folder not found",
                "destination already exists",
                "invalid folder path",
                "folder is not empty",
            )
        })?;
    Ok((
        StatusCode::CREATED,
        Json(SfxFolderResponse {
            name: folder.path.file_name().to_owned(),
            path: folder.path.into_string(),
            file_count: 0,
            has_children: false,
        }),
    ))
}

#[utoipa::path(
    delete,
    path = "/sfx/folders",
    operation_id = "delete_folder_api_sfx_folders_delete",
    params(DeleteFolderQuery),
    responses(
        (status = 200, description = "Successful Response", body = FolderDeleteResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "sfx"
)]
async fn delete_folder(
    State(state): State<HttpState>,
    headers: HeaderMap,
    query: Result<Query<DeleteFolderQuery>, QueryRejection>,
) -> Result<Json<FolderDeleteResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Query(query) = query.map_err(|_| ApiError::validation())?;
    let path = parse_required_folder(&query.path)?;
    sfx(&state)?
        .coordinator
        .delete_folder(path, query.recursive)
        .await
        .map_err(|error| {
            map_mutation_error(
                error,
                "folder not found",
                "destination already exists",
                "invalid folder path",
                "folder is not empty (pass recursive=true to force)",
            )
        })?;
    Ok(Json(FolderDeleteResponse { deleted: true }))
}

#[utoipa::path(
    post,
    path = "/sfx/folders/rename",
    operation_id = "rename_folder_api_sfx_folders_rename_post",
    request_body = FolderRenameRequest,
    responses(
        (status = 200, description = "Successful Response", body = SfxFolderResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "sfx"
)]
async fn rename_folder(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<FolderRenameRequest>, JsonRejection>,
) -> Result<Json<SfxFolderResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if payload.src.is_empty() || payload.dst.is_empty() {
        return Err(ApiError::validation());
    }
    let source = parse_required_folder(&payload.src)?;
    let destination = parse_required_folder(&payload.dst)?;
    let folder = sfx(&state)?
        .coordinator
        .rename_folder(source, destination)
        .await
        .map_err(|error| {
            map_mutation_error(
                error,
                "source folder not found",
                "destination already exists",
                "invalid folder path",
                "folder is not empty",
            )
        })?;
    Ok(Json(folder_response(folder)))
}

#[utoipa::path(
    post,
    path = "/sfx/move",
    operation_id = "move_file_api_sfx_move_post",
    request_body = SfxMoveRequest,
    responses(
        (status = 200, description = "Successful Response", body = SfxFileResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "sfx"
)]
async fn move_file(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<SfxMoveRequest>, JsonRejection>,
) -> Result<Json<SfxFileResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if payload.src.is_empty() {
        return Err(ApiError::validation());
    }
    let source = parse_required_path(&payload.src, "invalid path")?;
    let directory = parse_optional_folder(&payload.dst_folder)?;
    let file_name = payload
        .new_filename
        .as_deref()
        .unwrap_or_else(|| source.file_name());
    let destination = destination_path(directory.as_ref(), file_name)?;
    let file = sfx(&state)?
        .coordinator
        .move_file(source, destination)
        .await
        .map_err(|error| {
            map_mutation_error(
                error,
                "source file not found",
                "target already exists",
                "invalid path",
                "folder is not empty",
            )
        })?;
    let catalog = mode_catalog(&state);
    Ok(Json(file_response(file, catalog.as_deref())?))
}

#[utoipa::path(
    get,
    path = "/sfx/tree",
    operation_id = "list_tree_api_sfx_tree_get",
    params(TreeQuery),
    responses(
        (status = 200, description = "Successful Response", body = SfxTreeResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "sfx"
)]
/// The SFX files immediately in this folder. The folder hierarchy comes
/// from `/folders` (one whole-tree response) — the client builds the tree
/// from that, so this endpoint returns only the file list.
async fn list_tree(
    State(state): State<HttpState>,
    headers: HeaderMap,
    query: Result<Query<TreeQuery>, QueryRejection>,
) -> Result<Json<SfxTreeResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Query(query) = query.map_err(|_| ApiError::validation())?;
    let directory = parse_optional_folder(&query.path)?;
    let catalog = mode_catalog(&state);
    let files = sfx(&state)?
        .coordinator
        .list_directory(directory.as_ref())
        .await
        .map_err(map_read_error)?;
    Ok(Json(SfxTreeResponse {
        path: directory.map_or_else(String::new, SfxPath::into_string),
        files: file_responses(files, catalog.as_deref())?,
    }))
}

#[utoipa::path(
    post,
    path = "/sfx/upload",
    operation_id = "upload_api_sfx_upload_post",
    params(UploadQuery),
    request_body(content = SfxUploadMultipartBody, content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "Successful Response", body = SfxUploadResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "sfx"
)]
async fn upload(
    State(state): State<HttpState>,
    headers: HeaderMap,
    query: Result<Query<UploadQuery>, QueryRejection>,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<(StatusCode, Json<SfxUploadResponse>), ApiError> {
    authorize(&state, &headers).await?;
    let Query(query) = query.map_err(|_| ApiError::validation())?;
    let mut multipart = multipart.map_err(|_| ApiError::validation())?;
    let runtime = sfx(&state)?;
    let directory = parse_optional_folder(&query.dest)?;
    let mut staged = Vec::new();
    let mut destination_ready = directory.is_none();
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
        if staged.len() >= runtime.max_upload_files {
            return Err(ApiError::payload_too_large("too many files in one request"));
        }
        if !destination_ready {
            runtime
                .coordinator
                .create_folder(
                    directory
                        .clone()
                        .ok_or_else(|| ApiError::bad_request("invalid upload destination"))?,
                )
                .await
                .map_err(|error| {
                    map_mutation_error(
                        error,
                        "destination folder not found",
                        "destination already exists",
                        "invalid upload destination",
                        "folder is not empty",
                    )
                })?;
            destination_ready = true;
        }
        let requested = destination_path(directory.as_ref(), &file_name)?;
        staged.push(
            stage_upload_field(
                &runtime.root,
                requested,
                field,
                runtime.max_upload_file_bytes,
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
    let results = runtime
        .coordinator
        .publish_uploads(uploads, query.conflict.into())
        .await
        .map_err(map_upload_error)?;
    let catalog = mode_catalog(&state);
    let mut saved = Vec::new();
    let mut skipped = Vec::new();
    for result in results {
        match result {
            SfxUploadBatchItem::Published(file) => {
                saved.push(file_response(file, catalog.as_deref())?);
            }
            SfxUploadBatchItem::Skipped { requested } => {
                skipped.push(requested.file_name().to_owned());
            }
        }
    }
    Ok((
        StatusCode::CREATED,
        Json(SfxUploadResponse {
            saved,
            destination: directory.map_or_else(String::new, SfxPath::into_string),
            skipped,
        }),
    ))
}

#[utoipa::path(
    post,
    path = "/sfx/upload/check",
    operation_id = "upload_check_api_sfx_upload_check_post",
    request_body = UploadCheckRequest,
    responses(
        (status = 200, description = "Successful Response", body = UploadCheckResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "sfx"
)]
/// Report which proposed (dest, name) targets already exist under the SFX
/// root, so the client can ask about duplicates before sending bytes.
async fn upload_check(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<UploadCheckRequest>, JsonRejection>,
) -> Result<Json<UploadCheckResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let root = sfx(&state)?.root.clone();
    let collisions = sfx(&state)?
        .media_executor
        .execute(move || {
            payload
                .items
                .into_iter()
                .filter(|item| {
                    let Ok(directory) = parse_optional_folder(&item.dest) else {
                        return false;
                    };
                    let Ok(path) = destination_path(directory.as_ref(), &item.name) else {
                        return false;
                    };
                    sfx_upload_target_exists(&root, &path).unwrap_or(false)
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(map_media_worker_error)?;
    Ok(Json(UploadCheckResponse { collisions }))
}

fn map_media_worker_error(error: BlockingMediaError) -> ApiError {
    if error == BlockingMediaError::Busy {
        return ApiError::service_unavailable();
    }
    tracing::error!(error = %error, "SFX media worker failed");
    ApiError::internal()
}

async fn authorize(state: &HttpState, headers: &HeaderMap) -> Result<(), ApiError> {
    crate::auth::current_session(state, headers, SessionTouch::UpdateLastSeen)
        .await
        .map(|_| ())
}

fn sfx(state: &HttpState) -> Result<&RuntimeSfx, ApiError> {
    state
        .sfx
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)
}

fn mode_catalog(state: &HttpState) -> Option<Arc<ModeCatalog>> {
    state.modes.as_ref()?.snapshot()
}

fn file_responses(
    files: Vec<SfxFileRecord>,
    catalog: Option<&ModeCatalog>,
) -> Result<Vec<SfxFileResponse>, ApiError> {
    files
        .into_iter()
        .map(|file| file_response(file, catalog))
        .collect()
}

fn file_response(
    file: SfxFileRecord,
    catalog: Option<&ModeCatalog>,
) -> Result<SfxFileResponse, ApiError> {
    let referenced = catalog.is_some_and(|catalog| catalog.references_sfx_path(file.path.as_str()));
    Ok(SfxFileResponse {
        name: file.name,
        path: file.path.into_string(),
        size_bytes: file.size_bytes,
        modified_at: format_rfc3339(UnixSeconds::new(file.modified_at_unix_seconds))?,
        referenced,
    })
}

fn folder_response(folder: SfxFolderRecord) -> SfxFolderResponse {
    SfxFolderResponse {
        name: folder.name,
        path: folder.path.into_string(),
        file_count: folder.file_count,
        has_children: folder.has_children,
    }
}

fn parse_required_path(value: &str, detail: &'static str) -> Result<SfxPath, ApiError> {
    let normalized = value.replace('\\', "/");
    if normalized.is_empty() || normalized.starts_with('/') || normalized.ends_with('/') {
        return Err(ApiError::bad_request(detail));
    }
    SfxPath::parse(normalized).map_err(|_| ApiError::bad_request(detail))
}

fn parse_optional_folder(value: &str) -> Result<Option<SfxPath>, ApiError> {
    let normalized = value.trim_matches('/').replace('\\', "/");
    if normalized.is_empty() {
        Ok(None)
    } else {
        SfxPath::parse(normalized)
            .map(Some)
            .map_err(|_| ApiError::bad_request("invalid folder path"))
    }
}

fn parse_required_folder(value: &str) -> Result<SfxPath, ApiError> {
    parse_optional_folder(value)?.ok_or_else(|| ApiError::bad_request("invalid folder path"))
}

fn destination_path(directory: Option<&SfxPath>, file_name: &str) -> Result<SfxPath, ApiError> {
    if file_name.is_empty() || file_name.contains(['/', '\\']) {
        return Err(ApiError::bad_request("invalid filename"));
    }
    directory
        .map_or_else(
            || SfxPath::parse(file_name.to_owned()),
            |directory| directory.join(file_name),
        )
        .map_err(|_| ApiError::bad_request("invalid filename"))
}

struct StagedUpload {
    requested: SfxPath,
    staged: SfxPath,
    absolute: Option<PathBuf>,
}

impl StagedUpload {
    fn transfer(mut self) -> StagedSfxUpload {
        self.absolute = None;
        StagedSfxUpload {
            requested: self.requested.clone(),
            staged: self.staged.clone(),
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

fn upload_stage_path(requested: &SfxPath) -> Result<SfxPath, ApiError> {
    let name = format!(".sfx-upload-{}.partial", Uuid::new_v4().simple());
    requested.parent().map_or_else(
        || SfxPath::parse(&name).map_err(|_| ApiError::bad_request("upload path is too long")),
        |parent| {
            parent
                .join(&name)
                .map_err(|_| ApiError::bad_request("upload path is too long"))
        },
    )
}

async fn stage_upload_field(
    root: &SfxRoot,
    requested: SfxPath,
    mut field: Field<'_>,
    max_bytes: u64,
) -> Result<StagedUpload, ApiError> {
    let staged = upload_stage_path(&requested)?;
    let absolute = root.resolve_for_creation(&staged).map_err(|error| {
        tracing::error!(error = %error, "SFX upload staging path resolution failed");
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
            tracing::error!(error = %error, "SFX upload staging file creation failed");
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
            tracing::error!(error = %error, "SFX upload staging write failed");
            ApiError::internal()
        })?;
    }
    output.flush().await.map_err(|error| {
        tracing::error!(error = %error, "SFX upload staging flush failed");
        ApiError::internal()
    })?;
    output.sync_all().await.map_err(|error| {
        tracing::error!(error = %error, "SFX upload staging synchronization failed");
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

fn map_read_error(error: SfxCoordinatorError) -> ApiError {
    tracing::error!(error = %error, "SFX inventory operation failed");
    match error {
        SfxCoordinatorError::RecoveryConflict => ApiError::service_unavailable(),
        _ => ApiError::internal(),
    }
}

fn map_upload_error(error: SfxCoordinatorError) -> ApiError {
    map_mutation_error(
        error,
        "staged upload is missing",
        "upload destination changed during publication",
        "invalid upload destination",
        "folder is not empty",
    )
}

fn map_mutation_error(
    error: SfxCoordinatorError,
    not_found: &'static str,
    conflict: &'static str,
    invalid: &'static str,
    not_empty: &'static str,
) -> ApiError {
    match error {
        SfxCoordinatorError::Mutation(failure) => match failure.kind() {
            SfxMutationFailureKind::NotFound => ApiError::plain_not_found(not_found),
            SfxMutationFailureKind::Conflict => ApiError::conflict(conflict),
            SfxMutationFailureKind::NotEmpty => ApiError::bad_request(not_empty),
            SfxMutationFailureKind::Invalid => ApiError::bad_request(invalid),
            SfxMutationFailureKind::Capacity | SfxMutationFailureKind::Io => {
                tracing::error!(error = %failure, "SFX filesystem mutation failed");
                ApiError::internal()
            }
        },
        SfxCoordinatorError::RecoveryConflict => ApiError::service_unavailable(),
        other => {
            tracing::error!(error = %other, "SFX coordinator operation failed");
            ApiError::internal()
        }
    }
}

fn map_sfx_delivery_error(error: RootedPathError) -> ApiError {
    match &error {
        RootedPathError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
            ApiError::gone("sfx file missing on disk")
        }
        RootedPathError::EscapesRoot(_)
        | RootedPathError::SymbolicLinkTarget(_)
        | RootedPathError::ParentIsNotDirectory(_)
        | RootedPathError::TargetIsNotFile(_) => ApiError::bad_request("path escapes sfx root"),
        RootedPathError::Io { .. } | RootedPathError::RootIsNotDirectory(_) => {
            tracing::error!(error = %error, "SFX playback path resolution failed");
            ApiError::internal()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::error::Error;
    use std::fs;
    use std::path::Path;

    use axum::body::{Body, to_bytes};
    use axum::http::header::{CONTENT_DISPOSITION, COOKIE};
    use axum::http::{Request, StatusCode};
    use music_application::auth::{AuthRepository, UnixSeconds};
    use music_application::recovery::{RecoveryJournalDraft, RecoveryJournalRepository};
    use music_application::sfx::SfxMutation;
    use music_domain::SfxPath;
    use music_storage::{SqliteStorage, SqliteStorageOptions};
    use serde_json::Value;
    use tempfile::tempdir;
    use tower::ServiceExt;
    use uuid::Uuid;

    use crate::{AppConfig, AppRuntime, RuntimeError, TEST_PASSWORD_HASH};

    fn runtime_config(root: &Path) -> Result<AppConfig, RuntimeError> {
        AppConfig::from_values(&BTreeMap::from([
            (
                "DATABASE_URL".to_owned(),
                format!("sqlite:///{}", root.join("app.db").display()),
            ),
            (
                "MUSIC_DIR".to_owned(),
                root.join("music").display().to_string(),
            ),
            (
                "SFX_LIBRARY_DIR".to_owned(),
                root.join("sfx").display().to_string(),
            ),
            (
                "MODES_DIR".to_owned(),
                root.join("modes").display().to_string(),
            ),
            (
                "STATIC_DIR".to_owned(),
                root.join("missing-static").display().to_string(),
            ),
            (
                "DEVICES_FILE".to_owned(),
                root.join("devices.json").display().to_string(),
            ),
            ("SESSION_COOKIE_SECURE".to_owned(), "false".to_owned()),
            ("MAX_UPLOAD_FILES".to_owned(), "3".to_owned()),
            ("MAX_UPLOAD_FILE_BYTES".to_owned(), "1024".to_owned()),
        ]))
        .map_err(Into::into)
    }

    async fn seed_session(root: &Path, token: &str) -> Result<(), Box<dyn Error>> {
        let storage = SqliteStorage::open(SqliteStorageOptions::new(root.join("app.db"))).await?;
        let user_id = storage
            .create_user(
                "operator",
                TEST_PASSWORD_HASH,
                UnixSeconds::new(1_800_000_000),
            )
            .await?;
        AuthRepository::create_session(
            &storage,
            user_id,
            token,
            UnixSeconds::new(1_800_000_000),
            UnixSeconds::new(4_000_000_000),
        )
        .await
        .map_err(|error| -> Box<dyn Error> { error })?;
        storage.close().await;
        Ok(())
    }

    fn seed_mode(root: &Path) -> Result<(), Box<dyn Error>> {
        fs::create_dir_all(root.join("modes/table/soundboards"))?;
        fs::write(
            root.join("modes/table/manifest.yaml"),
            "id: table\nname: Table\npanels: [soundboard]\nplaylist_categories: []\ndefault_crossfade_ms: 0\ndefault_soundboard: main\n",
        )?;
        fs::write(
            root.join("modes/table/soundboards/main.yaml"),
            "id: main\nname: Main\ncategories:\n  - id: doors\n    name: Doors\n    items:\n      - file: dnd/door.ogg\n        name: Door\n      - file: dnd/sword.ogg\n        name: Sword\n",
        )?;
        Ok(())
    }

    fn multipart_upload(file_name: &str, bytes: &[u8]) -> (String, Vec<u8>) {
        let boundary = "music-rust-sfx-boundary";
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"{file_name}\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        (format!("multipart/form-data; boundary={boundary}"), body)
    }

    async fn json(response: axum::response::Response) -> Result<Value, Box<dyn Error>> {
        Ok(serde_json::from_slice(
            &to_bytes(response.into_body(), 1024 * 1024).await?,
        )?)
    }

    #[tokio::test]
    async fn playback_gate_and_management_routes_use_the_rust_sfx_owner()
    -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        seed_mode(directory.path())?;
        fs::create_dir_all(directory.path().join("sfx/dnd"))?;
        fs::write(directory.path().join("sfx/dnd/door.ogg"), b"door-bytes")?;
        let token = "sfx-test-session-token";
        seed_session(directory.path(), token).await?;
        let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
        let router = runtime.router()?;
        let cookie = format!("music_session={token}");

        let guest = router
            .clone()
            .oneshot(Request::get("/api/sfx/file?path=dnd%2Fdoor.ogg").body(Body::empty())?)
            .await?;
        assert_eq!(guest.status(), StatusCode::OK);
        assert_eq!(guest.headers()[CONTENT_DISPOSITION], "inline");
        assert_eq!(
            to_bytes(guest.into_body(), 1024).await?.as_ref(),
            b"door-bytes"
        );

        let unreferenced = router
            .clone()
            .oneshot(Request::get("/api/sfx/file?path=dnd%2Frandom.ogg").body(Body::empty())?)
            .await?;
        assert_eq!(unreferenced.status(), StatusCode::NOT_FOUND);
        let missing = router
            .clone()
            .oneshot(Request::get("/api/sfx/file?path=dnd%2Fsword.ogg").body(Body::empty())?)
            .await?;
        assert_eq!(missing.status(), StatusCode::GONE);
        let traversal = router
            .clone()
            .oneshot(Request::get("/api/sfx/file?path=..%2Fescape.ogg").body(Body::empty())?)
            .await?;
        assert_eq!(traversal.status(), StatusCode::BAD_REQUEST);

        let unauthenticated_tree = router
            .clone()
            .oneshot(Request::get("/api/sfx/tree").body(Body::empty())?)
            .await?;
        assert_eq!(unauthenticated_tree.status(), StatusCode::UNAUTHORIZED);
        let tree = router
            .clone()
            .oneshot(
                Request::get("/api/sfx/tree?path=dnd")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(tree.status(), StatusCode::OK);
        let tree = json(tree).await?;
        assert_eq!(tree["files"][0]["path"], "dnd/door.ogg");
        assert_eq!(tree["files"][0]["referenced"], true);

        let empty_boundary = "music-rust-empty-sfx-boundary";
        let empty_upload = router
            .clone()
            .oneshot(
                Request::post("/api/sfx/upload?dest=empty-request")
                    .header(COOKIE, &cookie)
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={empty_boundary}"),
                    )
                    .body(Body::from(format!("--{empty_boundary}--\r\n")))?,
            )
            .await?;
        assert_eq!(empty_upload.status(), StatusCode::BAD_REQUEST);
        assert!(!directory.path().join("sfx/empty-request").exists());

        let create = router
            .clone()
            .oneshot(
                Request::post("/api/sfx/folders")
                    .header(COOKIE, &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"path":"ambient/birds"}"#))?,
            )
            .await?;
        assert_eq!(create.status(), StatusCode::CREATED);

        let (content_type, body) = multipart_upload("ping.wav", b"first");
        let first = router
            .clone()
            .oneshot(
                Request::post("/api/sfx/upload?dest=ambient%2Fbirds")
                    .header(COOKIE, &cookie)
                    .header("content-type", content_type)
                    .body(Body::from(body))?,
            )
            .await?;
        assert_eq!(first.status(), StatusCode::CREATED);
        assert_eq!(
            json(first).await?["saved"][0]["path"],
            "ambient/birds/ping.wav"
        );

        let (content_type, body) = multipart_upload("ping.wav", b"second");
        let renamed = router
            .clone()
            .oneshot(
                Request::post("/api/sfx/upload?dest=ambient%2Fbirds")
                    .header(COOKIE, &cookie)
                    .header("content-type", content_type)
                    .body(Body::from(body))?,
            )
            .await?;
        assert_eq!(
            json(renamed).await?["saved"][0]["path"],
            "ambient/birds/ping-1.wav"
        );

        let (content_type, body) = multipart_upload("ping.wav", b"ignored");
        let skipped = router
            .clone()
            .oneshot(
                Request::post("/api/sfx/upload?dest=ambient%2Fbirds&conflict=skip")
                    .header(COOKIE, &cookie)
                    .header("content-type", content_type)
                    .body(Body::from(body))?,
            )
            .await?;
        assert_eq!(
            json(skipped).await?["skipped"],
            serde_json::json!(["ping.wav"])
        );

        let check = router
            .clone()
            .oneshot(
                Request::post("/api/sfx/upload/check")
                    .header(COOKIE, &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"items":[{"dest":"ambient/birds","name":"ping.wav"},{"dest":"ambient/birds","name":"new.wav"}]}"#,
                    ))?,
            )
            .await?;
        assert_eq!(
            json(check).await?["collisions"],
            serde_json::json!([{"dest":"ambient/birds","name":"ping.wav"}])
        );

        let moved = router
            .clone()
            .oneshot(
                Request::post("/api/sfx/move")
                    .header(COOKIE, &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"src":"ambient/birds/ping.wav","dst_folder":"moved","new_filename":"renamed.wav"}"#,
                    ))?,
            )
            .await?;
        assert_eq!(moved.status(), StatusCode::OK);
        assert_eq!(json(moved).await?["path"], "moved/renamed.wav");

        let renamed_folder = router
            .clone()
            .oneshot(
                Request::post("/api/sfx/folders/rename")
                    .header(COOKIE, &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"src":"moved","dst":"renamed-folder"}"#))?,
            )
            .await?;
        assert_eq!(renamed_folder.status(), StatusCode::OK);
        assert_eq!(json(renamed_folder).await?["path"], "renamed-folder");

        let deleted_file = router
            .clone()
            .oneshot(
                Request::delete("/api/sfx/files?path=renamed-folder%2Frenamed.wav")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(deleted_file.status(), StatusCode::NO_CONTENT);
        let deleted_folder = router
            .clone()
            .oneshot(
                Request::delete("/api/sfx/folders?path=renamed-folder")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(deleted_folder.status(), StatusCode::OK);
        assert!(!directory.path().join("sfx/renamed-folder").exists());

        runtime.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn startup_replays_a_planned_sfx_upload_before_serving_inventory()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        fs::create_dir_all(directory.path().join("sfx/uploads"))?;
        let staged = SfxPath::parse(format!(
            "uploads/.sfx-upload-{}.partial",
            Uuid::new_v4().simple()
        ))?;
        let destination = SfxPath::parse("uploads/recovered.wav")?;
        fs::write(
            directory.path().join("sfx").join(staged.as_str()),
            b"recovered",
        )?;

        let storage =
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?;
        let mutation = SfxMutation::PublishUpload {
            staged: staged.clone(),
            destination: destination.clone(),
            replace_existing: false,
        };
        RecoveryJournalRepository::create_recovery_journal(
            &storage,
            RecoveryJournalDraft::new(
                music_application::recovery::RecoveryDomain::Sfx,
                mutation.operation()?,
                mutation.plan(),
            )?,
        )
        .await?;
        storage.close().await;
        drop(storage);

        let runtime = AppRuntime::start(runtime_config(directory.path())?).await?;
        assert!(!directory.path().join("sfx").join(staged.as_str()).exists());
        assert_eq!(
            fs::read(directory.path().join("sfx").join(destination.as_str()))?,
            b"recovered"
        );
        runtime.shutdown().await?;
        Ok(())
    }
}
