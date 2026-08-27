use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use music_domain::{IndexedTrack, LibraryGeneration, LibraryPath, TrackId};

pub type LibraryDependencyError = Box<dyn Error + Send + Sync>;
pub type LibraryFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, LibraryDependencyError>> + Send + 'a>>;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ReconciliationStatus {
    Pending,
    Reconciling,
    Current,
    Failed,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LibraryStatus {
    pub generation: LibraryGeneration,
    pub status: ReconciliationStatus,
    pub scan_started_at_unix_seconds: Option<i64>,
    pub last_scan_at_unix_seconds: Option<i64>,
    pub last_error_code: Option<String>,
    pub discovered_tracks: u64,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LibrarySortKey {
    Title,
    Artist,
    Album,
    AlbumArtist,
    Year,
    LengthSeconds,
    TrackNumber,
    AddedAt,
    Path,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SortOrder {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LibrarySearch {
    pub query: String,
    pub limit: u16,
    pub offset: u64,
    pub sort: LibrarySortKey,
    pub order: SortOrder,
}

impl LibrarySearch {
    pub fn new(
        query: impl Into<String>,
        limit: u16,
        offset: u64,
        sort: LibrarySortKey,
        order: SortOrder,
    ) -> Result<Self, LibraryQueryError> {
        if !(1..=500).contains(&limit) {
            return Err(LibraryQueryError::InvalidLimit);
        }
        Ok(Self {
            query: query.into(),
            limit,
            offset,
            sort,
            order,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LibrarySearchResult {
    pub tracks: Vec<IndexedTrack>,
    pub total: u64,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LibraryQueryError {
    InvalidLimit,
}

impl Display for LibraryQueryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidLimit => "library search limit must be between 1 and 500",
        })
    }
}

impl Error for LibraryQueryError {}

pub trait LibraryRepository: std::fmt::Debug + Send + Sync {
    fn status(&self) -> LibraryFuture<'_, LibraryStatus>;

    fn catalog_track_ids(&self) -> LibraryFuture<'_, Vec<TrackId>>;

    fn track(&self, track_id: TrackId) -> LibraryFuture<'_, Option<IndexedTrack>>;

    fn tracks_by_ids<'a>(
        &'a self,
        track_ids: &'a [TrackId],
    ) -> LibraryFuture<'a, Vec<IndexedTrack>>;

    fn search<'a>(&'a self, request: &'a LibrarySearch) -> LibraryFuture<'a, LibrarySearchResult>;

    fn tracks_in_directory<'a>(
        &'a self,
        directory: Option<&'a LibraryPath>,
    ) -> LibraryFuture<'a, Vec<IndexedTrack>>;
}

#[derive(Debug, Clone)]
pub struct LibraryService {
    repository: Arc<dyn LibraryRepository>,
}

impl LibraryService {
    #[must_use]
    pub fn new(repository: Arc<dyn LibraryRepository>) -> Self {
        Self { repository }
    }

    pub async fn status(&self) -> Result<LibraryStatus, LibraryDependencyError> {
        self.repository.status().await
    }

    pub async fn catalog_track_ids(&self) -> Result<Vec<TrackId>, LibraryDependencyError> {
        self.repository.catalog_track_ids().await
    }

    pub async fn track(
        &self,
        track_id: TrackId,
    ) -> Result<Option<IndexedTrack>, LibraryDependencyError> {
        self.repository.track(track_id).await
    }

    pub async fn tracks_by_ids(
        &self,
        track_ids: &[TrackId],
    ) -> Result<Vec<IndexedTrack>, LibraryDependencyError> {
        self.repository.tracks_by_ids(track_ids).await
    }

    pub async fn search(
        &self,
        request: &LibrarySearch,
    ) -> Result<LibrarySearchResult, LibraryDependencyError> {
        self.repository.search(request).await
    }

    pub async fn tracks_in_directory(
        &self,
        directory: Option<&LibraryPath>,
    ) -> Result<Vec<IndexedTrack>, LibraryDependencyError> {
        self.repository.tracks_in_directory(directory).await
    }
}

#[cfg(test)]
mod tests {
    use super::{LibraryQueryError, LibrarySearch, LibrarySortKey, SortOrder};

    #[test]
    fn search_bounds_match_the_public_contract() -> Result<(), LibraryQueryError> {
        let request = LibrarySearch::new(
            "underscore_% is literal",
            500,
            0,
            LibrarySortKey::Artist,
            SortOrder::Ascending,
        )?;
        assert_eq!(request.limit, 500);
        assert_eq!(
            LibrarySearch::new("", 0, 0, LibrarySortKey::Artist, SortOrder::Ascending,),
            Err(LibraryQueryError::InvalidLimit)
        );
        Ok(())
    }
}
