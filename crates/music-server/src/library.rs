use std::sync::Arc;

use axum::Json;
use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use music_application::auth::{SessionTouch, UnixSeconds};
use music_application::library::{
    LibraryCoordinatorHandle, LibrarySearch, LibraryService, LibrarySortKey, SortOrder,
};
use music_domain::{IndexedTrack, LibraryPath, TrackId};
use music_media::{LibraryRoot, list_library_directories};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::error::{ApiError, HttpValidationErrorBody};
use crate::http::HttpState;

#[derive(Debug, Clone)]
pub(crate) struct RuntimeLibrary {
    pub(crate) service: Arc<LibraryService>,
    pub(crate) coordinator: LibraryCoordinatorHandle,
    pub(crate) root: LibraryRoot,
}

pub(crate) fn library_router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default()
        .routes(routes!(search))
        .routes(routes!(tree))
        .routes(routes!(folders))
        .routes(routes!(tracks_batch))
        .routes(routes!(track))
        .routes(routes!(rescan))
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = TrackOut)]
struct TrackResponse {
    id: i64,
    path: String,
    title: String,
    artist: String,
    album_artist: String,
    album: String,
    track_no: Option<u32>,
    disc_no: Option<u32>,
    year: Option<u32>,
    genre: String,
    length_s: f64,
    bpm: Option<u32>,
    size_bytes: u64,
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
    total: u64,
    limit: u16,
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
    track_count: u64,
    has_children: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = FoldersResponse)]
struct FoldersResponse {
    folders: Vec<FolderResponse>,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[schema(as = RescanResult)]
struct RescanResponse {
    added: u64,
    updated: u64,
    removed: u64,
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
    q: String,
    #[serde(default = "default_search_limit")]
    #[param(minimum = 1, maximum = 500)]
    limit: u16,
    #[serde(default)]
    offset: u64,
    #[serde(default)]
    sort: SearchSort,
    #[serde(default)]
    order: SearchOrder,
}

const fn default_search_limit() -> u16 {
    100
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct TreeQuery {
    path: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct BatchQuery {
    ids: String,
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
    get,
    path = "/library/tracks/{track_id}",
    params(("track_id" = i64, Path, description = "Track identifier")),
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
    let track_id = match TrackId::new(raw_track_id) {
        Ok(track_id) => track_id,
        Err(_) => return Err(ApiError::plain_not_found("track not found")),
    };
    let track = library(&state)?
        .service
        .track(track_id)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "library track query failed");
            ApiError::internal()
        })?
        .ok_or_else(|| ApiError::plain_not_found("track not found"))?;
    Ok(Json(track.try_into()?))
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

fn library(state: &HttpState) -> Result<&RuntimeLibrary, ApiError> {
    state
        .library
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)
}
