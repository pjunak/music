use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use music_domain::{IndexedTrack, LibraryGeneration, LibraryPath, TrackId, TrackMetadata};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::playback::PlaybackActorHandle;

const LIBRARY_COMMAND_CAPACITY: usize = 4;

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

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DiscoveredTrack {
    pub path: LibraryPath,
    pub metadata: TrackMetadata,
    pub duration: Duration,
    pub size_bytes: u64,
    pub mtime_unix_seconds: i64,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct ReconciliationSummary {
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
    pub unchanged: u64,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ReconciliationCommit {
    Applied {
        status: LibraryStatus,
        summary: ReconciliationSummary,
        track_ids: BTreeSet<TrackId>,
    },
    Conflict {
        current_generation: LibraryGeneration,
    },
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

    fn folder_track_counts(&self) -> LibraryFuture<'_, BTreeMap<LibraryPath, u64>>;
}

pub trait LibraryMutationRepository: LibraryRepository {
    fn begin_reconciliation(&self) -> LibraryFuture<'_, LibraryStatus>;

    fn commit_reconciliation(
        &self,
        expected_generation: LibraryGeneration,
        discovered: Vec<DiscoveredTrack>,
    ) -> LibraryFuture<'_, ReconciliationCommit>;

    fn fail_reconciliation<'a>(
        &'a self,
        expected_generation: LibraryGeneration,
        error_code: &'a str,
    ) -> LibraryFuture<'a, LibraryStatus>;
}

#[derive(Debug)]
pub struct LibraryDiscoveryFailure {
    pub code: &'static str,
    pub source: LibraryDependencyError,
}

impl LibraryDiscoveryFailure {
    #[must_use]
    pub fn new(code: &'static str, source: LibraryDependencyError) -> Self {
        Self { code, source }
    }
}

impl Display for LibraryDiscoveryFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "library discovery failed ({})", self.code)
    }
}

impl Error for LibraryDiscoveryFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

pub type LibraryDiscoveryFuture<'a> = Pin<
    Box<dyn Future<Output = Result<Vec<DiscoveredTrack>, LibraryDiscoveryFailure>> + Send + 'a>,
>;

pub trait LibraryDiscovery: std::fmt::Debug + Send + Sync {
    fn discover(&self, cancellation: CancellationToken) -> LibraryDiscoveryFuture<'_>;
}

pub trait LibraryCatalogSink: std::fmt::Debug + Send + Sync {
    fn publish(
        &self,
        generation: LibraryGeneration,
        track_ids: BTreeSet<TrackId>,
    ) -> LibraryFuture<'_, ()>;
}

impl LibraryCatalogSink for PlaybackActorHandle {
    fn publish(
        &self,
        generation: LibraryGeneration,
        track_ids: BTreeSet<TrackId>,
    ) -> LibraryFuture<'_, ()> {
        Box::pin(async move {
            self.replace_library_catalog(generation, track_ids)
                .await
                .map(|_| ())
                .map_err(|source| -> LibraryDependencyError { Box::new(source) })
        })
    }
}

#[derive(Debug)]
pub enum LibraryCoordinatorError {
    Dependency {
        operation: &'static str,
        source: LibraryDependencyError,
    },
    Discovery(LibraryDiscoveryFailure),
    GenerationConflict {
        expected: LibraryGeneration,
        current: LibraryGeneration,
    },
    CommandQueueFull,
    Unavailable,
}

impl Display for LibraryCoordinatorError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dependency { operation, .. } => {
                write!(formatter, "library coordinator could not {operation}")
            }
            Self::Discovery(failure) => Display::fmt(failure, formatter),
            Self::GenerationConflict { expected, current } => write!(
                formatter,
                "library generation changed during reconciliation (expected {}, current {})",
                expected.get(),
                current.get()
            ),
            Self::CommandQueueFull => formatter.write_str("library command queue is full"),
            Self::Unavailable => formatter.write_str("library coordinator is unavailable"),
        }
    }
}

impl Error for LibraryCoordinatorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Dependency { source, .. } => Some(source.as_ref()),
            Self::Discovery(failure) => Some(failure),
            Self::GenerationConflict { .. } | Self::CommandQueueFull | Self::Unavailable => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LibraryCoordinatorHandle {
    commands: mpsc::Sender<LibraryCommand>,
    cancellation: CancellationToken,
    status: watch::Receiver<LibraryStatus>,
}

impl LibraryCoordinatorHandle {
    #[must_use]
    pub fn status(&self) -> LibraryStatus {
        self.status.borrow().clone()
    }

    #[must_use]
    pub fn subscribe_status(&self) -> watch::Receiver<LibraryStatus> {
        self.status.clone()
    }

    pub async fn reconcile(&self) -> Result<ReconciliationSummary, LibraryCoordinatorError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(LibraryCommand::Reconcile { reply: Some(reply) })
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?;
        response
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?
    }

    pub fn request_reconciliation(&self) -> Result<(), LibraryCoordinatorError> {
        self.commands
            .try_send(LibraryCommand::Reconcile { reply: None })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => LibraryCoordinatorError::CommandQueueFull,
                mpsc::error::TrySendError::Closed(_) => LibraryCoordinatorError::Unavailable,
            })
    }

    pub fn shutdown(&self) {
        self.cancellation.cancel();
    }
}

#[derive(Debug)]
pub struct SpawnedLibraryCoordinator {
    pub handle: LibraryCoordinatorHandle,
    pub task: JoinHandle<Result<(), LibraryCoordinatorError>>,
}

pub async fn start_library_coordinator(
    repository: Arc<dyn LibraryMutationRepository>,
    discovery: Arc<dyn LibraryDiscovery>,
    catalog: Arc<dyn LibraryCatalogSink>,
) -> Result<SpawnedLibraryCoordinator, LibraryCoordinatorError> {
    let mut status = repository
        .status()
        .await
        .map_err(|source| dependency("load durable library status", source))?;
    if status.status == ReconciliationStatus::Reconciling {
        status = repository
            .fail_reconciliation(status.generation, "scan_interrupted")
            .await
            .map_err(|source| dependency("recover an interrupted reconciliation", source))?;
    }
    let track_ids = repository
        .catalog_track_ids()
        .await
        .map_err(|source| dependency("load the durable track catalog", source))?
        .into_iter()
        .collect();
    catalog
        .publish(status.generation, track_ids)
        .await
        .map_err(|source| dependency("publish the durable track catalog", source))?;

    let (status_sender, status_receiver) = watch::channel(status);
    let (commands, receiver) = mpsc::channel(LIBRARY_COMMAND_CAPACITY);
    let cancellation = CancellationToken::new();
    let actor = LibraryCoordinator {
        repository,
        discovery,
        catalog,
        status: status_sender,
    };
    let task_cancellation = cancellation.clone();
    let task = tokio::spawn(actor.run(receiver, task_cancellation));
    Ok(SpawnedLibraryCoordinator {
        handle: LibraryCoordinatorHandle {
            commands,
            cancellation,
            status: status_receiver,
        },
        task,
    })
}

#[derive(Debug)]
enum LibraryCommand {
    Reconcile {
        reply: Option<oneshot::Sender<Result<ReconciliationSummary, LibraryCoordinatorError>>>,
    },
}

#[derive(Debug)]
struct LibraryCoordinator {
    repository: Arc<dyn LibraryMutationRepository>,
    discovery: Arc<dyn LibraryDiscovery>,
    catalog: Arc<dyn LibraryCatalogSink>,
    status: watch::Sender<LibraryStatus>,
}

impl LibraryCoordinator {
    async fn run(
        self,
        mut commands: mpsc::Receiver<LibraryCommand>,
        cancellation: CancellationToken,
    ) -> Result<(), LibraryCoordinatorError> {
        loop {
            tokio::select! {
                () = cancellation.cancelled() => return Ok(()),
                command = commands.recv() => {
                    let Some(command) = command else {
                        return Ok(());
                    };
                    match command {
                        LibraryCommand::Reconcile { reply } => {
                            let result = self.reconcile_once(cancellation.clone()).await;
                            if let Some(reply) = reply {
                                let _ = reply.send(result);
                            }
                        }
                    }
                }
            }
        }
    }

    async fn reconcile_once(
        &self,
        cancellation: CancellationToken,
    ) -> Result<ReconciliationSummary, LibraryCoordinatorError> {
        let started = self
            .repository
            .begin_reconciliation()
            .await
            .map_err(|source| dependency("begin library reconciliation", source))?;
        self.status.send_replace(started.clone());
        let discovered = match self.discovery.discover(cancellation).await {
            Ok(discovered) => discovered,
            Err(failure) => {
                let failed = self
                    .repository
                    .fail_reconciliation(started.generation, failure.code)
                    .await
                    .map_err(|source| dependency("record library discovery failure", source))?;
                self.status.send_replace(failed);
                return Err(LibraryCoordinatorError::Discovery(failure));
            }
        };
        let committed = match self
            .repository
            .commit_reconciliation(started.generation, discovered)
            .await
        {
            Ok(committed) => committed,
            Err(source) => {
                let failed = self
                    .repository
                    .fail_reconciliation(started.generation, "scan_commit_failed")
                    .await
                    .map_err(|failure| dependency("record library commit failure", failure))?;
                self.status.send_replace(failed);
                return Err(dependency("commit library reconciliation", source));
            }
        };
        match committed {
            ReconciliationCommit::Applied {
                status,
                summary,
                track_ids,
            } => {
                self.status.send_replace(status.clone());
                self.catalog
                    .publish(status.generation, track_ids)
                    .await
                    .map_err(|source| dependency("publish reconciled track catalog", source))?;
                Ok(summary)
            }
            ReconciliationCommit::Conflict { current_generation } => {
                let current = self
                    .repository
                    .status()
                    .await
                    .map_err(|source| dependency("reload conflicted library status", source))?;
                self.status.send_replace(current);
                Err(LibraryCoordinatorError::GenerationConflict {
                    expected: started.generation,
                    current: current_generation,
                })
            }
        }
    }
}

fn dependency(operation: &'static str, source: LibraryDependencyError) -> LibraryCoordinatorError {
    LibraryCoordinatorError::Dependency { operation, source }
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

    pub async fn folder_track_counts(
        &self,
    ) -> Result<BTreeMap<LibraryPath, u64>, LibraryDependencyError> {
        self.repository.folder_track_counts().await
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
