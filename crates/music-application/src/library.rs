use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use music_domain::{IndexedTrack, LibraryGeneration, LibraryPath, TrackId, TrackMetadata};
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::cleanup::{
    CleanupApplyOperation, CleanupApplyResult, CleanupBatchAppend, CleanupInputValue,
    CleanupMutationRepository, CleanupMutationValidationError, CleanupOperationKind,
    CleanupRevertMutation, CleanupRevertResult, CleanupSkip, MAX_CLEANUP_APPLY_OPERATIONS,
    MAX_CLEANUP_REVERT_ITEMS, MAX_CLEANUP_SCOPE_LABEL_CHARS,
};
use crate::playback::PlaybackActorHandle;
use crate::recovery::{
    RecoveryDomain, RecoveryJournalDraft, RecoveryJournalEntry, RecoveryJournalRepository,
    RecoveryOperation, RecoveryState, RecoveryTransition,
};

mod metadata_patch;
mod mutation;

pub use metadata_patch::{
    TrackMetadataField, TrackMetadataPatch, TrackMetadataPatchError, TrackMetadataPatchValue,
};
pub use mutation::{
    FolderDeletionResult, FolderMutationResult, LibraryFileMutation, LibraryFileMutationOutcome,
    LibraryFileTagReadFuture, LibraryFileTagValue, LibraryIndexMutationCommit,
    LibraryMutationEffects, LibraryMutationFailure, LibraryMutationFailureKind,
    LibraryMutationFuture, LibraryMutationValidationError, LibraryTrackMutationCommit,
    LibraryUploadDiscardFuture, LibraryUploadMutationCommit, LibraryUploadResolution,
    LibraryUploadResolutionFuture, UploadConflictPolicy,
};

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

    fn all_tracks(&self) -> LibraryFuture<'_, Vec<IndexedTrack>>;

    fn catalog_track_ids(&self) -> LibraryFuture<'_, Vec<TrackId>>;

    fn track(&self, track_id: TrackId) -> LibraryFuture<'_, Option<IndexedTrack>>;

    fn track_by_path<'a>(
        &'a self,
        path: &'a LibraryPath,
    ) -> LibraryFuture<'a, Option<IndexedTrack>>;

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

pub trait LibraryMutationRepository: LibraryRepository + RecoveryJournalRepository {
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

    fn commit_folder_rename<'a>(
        &'a self,
        journal_id: &'a crate::recovery::RecoveryJournalId,
        source: &'a LibraryPath,
        destination: &'a LibraryPath,
    ) -> LibraryFuture<'a, LibraryIndexMutationCommit>;

    fn commit_folder_delete<'a>(
        &'a self,
        journal_id: &'a crate::recovery::RecoveryJournalId,
        path: &'a LibraryPath,
    ) -> LibraryFuture<'a, LibraryIndexMutationCommit>;

    fn commit_track_move<'a>(
        &'a self,
        journal_id: &'a crate::recovery::RecoveryJournalId,
        track_id: TrackId,
        source: &'a LibraryPath,
        discovered: &'a DiscoveredTrack,
    ) -> LibraryFuture<'a, LibraryTrackMutationCommit>;

    fn commit_track_delete<'a>(
        &'a self,
        journal_id: &'a crate::recovery::RecoveryJournalId,
        track_id: TrackId,
        path: &'a LibraryPath,
    ) -> LibraryFuture<'a, LibraryIndexMutationCommit>;

    fn commit_track_metadata<'a>(
        &'a self,
        journal_id: &'a crate::recovery::RecoveryJournalId,
        track_id: TrackId,
        path: &'a LibraryPath,
        patch: &'a TrackMetadataPatch,
        discovered: Option<&'a DiscoveredTrack>,
    ) -> LibraryFuture<'a, LibraryTrackMutationCommit>;

    fn commit_upload<'a>(
        &'a self,
        journal_id: &'a crate::recovery::RecoveryJournalId,
        staged: &'a LibraryPath,
        destination: &'a LibraryPath,
        replace_existing: bool,
        discovered: Option<&'a DiscoveredTrack>,
    ) -> LibraryFuture<'a, LibraryUploadMutationCommit>;
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
    Mutation(LibraryMutationFailure),
    InvalidMutation(LibraryMutationValidationError),
    RecoveryConflict,
    InvalidMutationOutcome,
    TrackNotFound {
        track_id: TrackId,
    },
    InvalidCleanupBatchSize,
    InvalidCleanupRevertSize,
    InvalidCleanupScopeLabel,
    CleanupBatchNotFound,
    CleanupBatchReverted,
    InvalidCleanupMutation(CleanupMutationValidationError),
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
            Self::Mutation(failure) => Display::fmt(failure, formatter),
            Self::InvalidMutation(failure) => Display::fmt(failure, formatter),
            Self::RecoveryConflict => {
                formatter.write_str("library recovery journal changed unexpectedly")
            }
            Self::InvalidMutationOutcome => {
                formatter.write_str("library mutation returned an unexpected outcome")
            }
            Self::TrackNotFound { track_id } => {
                write!(formatter, "library track {} was not found", track_id.get())
            }
            Self::InvalidCleanupBatchSize => write!(
                formatter,
                "cleanup apply requires between 1 and {MAX_CLEANUP_APPLY_OPERATIONS} operations"
            ),
            Self::InvalidCleanupRevertSize => write!(
                formatter,
                "cleanup revert requires between 1 and {MAX_CLEANUP_REVERT_ITEMS} journal items"
            ),
            Self::InvalidCleanupScopeLabel => {
                formatter.write_str("cleanup scope label is too long")
            }
            Self::CleanupBatchNotFound => formatter.write_str("cleanup batch was not found"),
            Self::CleanupBatchReverted => formatter.write_str("cleanup batch was already reverted"),
            Self::InvalidCleanupMutation(error) => Display::fmt(error, formatter),
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
            Self::Mutation(failure) => Some(failure),
            Self::InvalidMutation(failure) => Some(failure),
            Self::InvalidCleanupMutation(failure) => Some(failure),
            Self::GenerationConflict { .. }
            | Self::RecoveryConflict
            | Self::InvalidMutationOutcome
            | Self::TrackNotFound { .. }
            | Self::InvalidCleanupBatchSize
            | Self::InvalidCleanupRevertSize
            | Self::InvalidCleanupScopeLabel
            | Self::CleanupBatchNotFound
            | Self::CleanupBatchReverted
            | Self::CommandQueueFull
            | Self::Unavailable => None,
        }
    }
}

pub type TrackMoveBatchResults = Vec<(TrackId, Result<IndexedTrack, LibraryCoordinatorError>)>;
pub type TrackDeleteBatchResults = Vec<(TrackId, Result<(), LibraryCoordinatorError>)>;

#[derive(Debug)]
pub struct TrackMetadataBatchItem {
    pub track_id: TrackId,
    pub track: Option<IndexedTrack>,
    pub error: Option<LibraryCoordinatorError>,
}

pub type TrackMetadataBatchResults = Vec<TrackMetadataBatchItem>;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StagedLibraryUpload {
    pub staged: LibraryPath,
    pub requested: LibraryPath,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LibraryUploadBatchItem {
    Published {
        destination: LibraryPath,
        track: Option<Box<IndexedTrack>>,
    },
    Skipped {
        requested: LibraryPath,
    },
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

    pub async fn create_folder(
        &self,
        path: LibraryPath,
    ) -> Result<FolderMutationResult, LibraryCoordinatorError> {
        let applied = self
            .mutate(LibraryFileMutation::CreateFolder { path })
            .await?;
        match applied.outcome {
            LibraryFileMutationOutcome::Folder { path, has_children } => {
                Ok(FolderMutationResult { path, has_children })
            }
            _ => Err(LibraryCoordinatorError::InvalidMutationOutcome),
        }
    }

    pub async fn rename_folder(
        &self,
        source: LibraryPath,
        destination: LibraryPath,
    ) -> Result<FolderMutationResult, LibraryCoordinatorError> {
        let applied = self
            .mutate(LibraryFileMutation::RenameFolder {
                source,
                destination,
            })
            .await?;
        match applied.outcome {
            LibraryFileMutationOutcome::Folder { path, has_children } => {
                Ok(FolderMutationResult { path, has_children })
            }
            _ => Err(LibraryCoordinatorError::InvalidMutationOutcome),
        }
    }

    pub async fn delete_folder(
        &self,
        path: LibraryPath,
        recursive: bool,
    ) -> Result<FolderDeletionResult, LibraryCoordinatorError> {
        let applied = self
            .mutate(LibraryFileMutation::DeleteFolder { path, recursive })
            .await?;
        match applied.outcome {
            LibraryFileMutationOutcome::Deleted => Ok(FolderDeletionResult {
                removed_tracks: applied.affected_tracks,
            }),
            _ => Err(LibraryCoordinatorError::InvalidMutationOutcome),
        }
    }

    pub async fn move_track(
        &self,
        track_id: TrackId,
        destination: LibraryPath,
    ) -> Result<IndexedTrack, LibraryCoordinatorError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(LibraryCommand::MoveTrack {
                track_id,
                destination,
                reply,
            })
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?;
        response
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?
    }

    pub async fn delete_track(&self, track_id: TrackId) -> Result<(), LibraryCoordinatorError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(LibraryCommand::DeleteTrack { track_id, reply })
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?;
        response
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?
    }

    pub async fn move_tracks(
        &self,
        requests: Vec<(TrackId, LibraryPath)>,
    ) -> Result<TrackMoveBatchResults, LibraryCoordinatorError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(LibraryCommand::MoveTracks { requests, reply })
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?;
        response
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?
    }

    pub async fn delete_tracks(
        &self,
        track_ids: Vec<TrackId>,
    ) -> Result<TrackDeleteBatchResults, LibraryCoordinatorError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(LibraryCommand::DeleteTracks { track_ids, reply })
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?;
        response
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?
    }

    pub async fn update_track_metadata(
        &self,
        track_id: TrackId,
        patch: TrackMetadataPatch,
    ) -> Result<IndexedTrack, LibraryCoordinatorError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(LibraryCommand::UpdateTrackMetadata {
                track_id,
                patch,
                reply,
            })
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?;
        response
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?
    }

    pub async fn update_tracks_metadata(
        &self,
        track_ids: Vec<TrackId>,
        patch: TrackMetadataPatch,
    ) -> Result<TrackMetadataBatchResults, LibraryCoordinatorError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(LibraryCommand::UpdateTracksMetadata {
                track_ids,
                patch,
                reply,
            })
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?;
        response
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?
    }

    pub async fn publish_uploads(
        &self,
        uploads: Vec<StagedLibraryUpload>,
        policy: UploadConflictPolicy,
    ) -> Result<Vec<LibraryUploadBatchItem>, LibraryCoordinatorError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(LibraryCommand::PublishUploads {
                uploads,
                policy,
                reply,
            })
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?;
        response
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?
    }

    pub async fn apply_cleanup(
        &self,
        batch_id: Option<i64>,
        scope_label: String,
        operations: Vec<CleanupApplyOperation>,
    ) -> Result<CleanupApplyResult, LibraryCoordinatorError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(LibraryCommand::ApplyCleanup {
                batch_id,
                scope_label,
                operations,
                reply,
            })
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?;
        response
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?
    }

    pub async fn revert_cleanup_batch(
        &self,
        batch_id: i64,
    ) -> Result<CleanupRevertResult, LibraryCoordinatorError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(LibraryCommand::RevertCleanupBatch { batch_id, reply })
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?;
        response
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?
    }

    pub async fn revert_cleanup_journal(
        &self,
        items: Vec<Map<String, Value>>,
    ) -> Result<CleanupRevertResult, LibraryCoordinatorError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(LibraryCommand::RevertCleanupJournal { items, reply })
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?;
        response
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?
    }

    async fn mutate(
        &self,
        mutation: LibraryFileMutation,
    ) -> Result<AppliedLibraryMutation, LibraryCoordinatorError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(LibraryCommand::Mutate { mutation, reply })
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?;
        response
            .await
            .map_err(|_| LibraryCoordinatorError::Unavailable)?
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
    repository: Arc<dyn CleanupMutationRepository>,
    discovery: Arc<dyn LibraryDiscovery>,
    catalog: Arc<dyn LibraryCatalogSink>,
    effects: Arc<dyn LibraryMutationEffects>,
) -> Result<SpawnedLibraryCoordinator, LibraryCoordinatorError> {
    recover_library_mutations(repository.as_ref(), effects.as_ref()).await?;
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
    let (status_sender, status_receiver) = watch::channel(status.clone());
    let (commands, receiver) = mpsc::channel(LIBRARY_COMMAND_CAPACITY);
    let cancellation = CancellationToken::new();
    let actor = LibraryCoordinator {
        repository: repository.clone(),
        discovery,
        catalog: catalog.clone(),
        effects: effects.clone(),
        status: status_sender,
    };
    actor.recover_cleanup_mutations().await?;
    status = repository
        .status()
        .await
        .map_err(|source| dependency("reload recovered library status", source))?;
    actor.status.send_replace(status.clone());
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
    Mutate {
        mutation: LibraryFileMutation,
        reply: oneshot::Sender<Result<AppliedLibraryMutation, LibraryCoordinatorError>>,
    },
    MoveTrack {
        track_id: TrackId,
        destination: LibraryPath,
        reply: oneshot::Sender<Result<IndexedTrack, LibraryCoordinatorError>>,
    },
    DeleteTrack {
        track_id: TrackId,
        reply: oneshot::Sender<Result<(), LibraryCoordinatorError>>,
    },
    MoveTracks {
        requests: Vec<(TrackId, LibraryPath)>,
        reply: oneshot::Sender<Result<TrackMoveBatchResults, LibraryCoordinatorError>>,
    },
    DeleteTracks {
        track_ids: Vec<TrackId>,
        reply: oneshot::Sender<Result<TrackDeleteBatchResults, LibraryCoordinatorError>>,
    },
    UpdateTrackMetadata {
        track_id: TrackId,
        patch: TrackMetadataPatch,
        reply: oneshot::Sender<Result<IndexedTrack, LibraryCoordinatorError>>,
    },
    UpdateTracksMetadata {
        track_ids: Vec<TrackId>,
        patch: TrackMetadataPatch,
        reply: oneshot::Sender<Result<TrackMetadataBatchResults, LibraryCoordinatorError>>,
    },
    PublishUploads {
        uploads: Vec<StagedLibraryUpload>,
        policy: UploadConflictPolicy,
        reply: oneshot::Sender<Result<Vec<LibraryUploadBatchItem>, LibraryCoordinatorError>>,
    },
    ApplyCleanup {
        batch_id: Option<i64>,
        scope_label: String,
        operations: Vec<CleanupApplyOperation>,
        reply: oneshot::Sender<Result<CleanupApplyResult, LibraryCoordinatorError>>,
    },
    RevertCleanupBatch {
        batch_id: i64,
        reply: oneshot::Sender<Result<CleanupRevertResult, LibraryCoordinatorError>>,
    },
    RevertCleanupJournal {
        items: Vec<Map<String, Value>>,
        reply: oneshot::Sender<Result<CleanupRevertResult, LibraryCoordinatorError>>,
    },
}

#[derive(Debug)]
struct AppliedLibraryMutation {
    outcome: LibraryFileMutationOutcome,
    affected_tracks: u64,
    track: Option<IndexedTrack>,
}

#[derive(Debug)]
struct PreparedCleanupMutation {
    track_id: i64,
    kind: PreparedCleanupKind,
    mutation: LibraryFileMutation,
    append: CleanupBatchAppend,
}

#[derive(Debug)]
enum PreparedCleanupKind {
    Rename { target_name: String },
    Tag,
    FolderRename { target_name: String },
}

#[derive(Debug)]
struct AppliedCleanupMutation {
    affected_tracks: u64,
    batch_id: i64,
}

#[derive(Debug)]
struct PreparedCleanupRevert {
    track_id: i64,
    kind: PreparedCleanupRevertKind,
    mutation: LibraryFileMutation,
    revert: CleanupRevertMutation,
}

#[derive(Debug)]
enum PreparedCleanupRevertKind {
    Rename { original_name: String },
    Tag,
    FolderRename { original_name: String },
}

#[derive(Debug)]
enum CleanupPreparationError {
    Skip(CleanupSkip),
    Fatal(LibraryCoordinatorError),
}

impl From<CleanupSkip> for CleanupPreparationError {
    fn from(skip: CleanupSkip) -> Self {
        Self::Skip(skip)
    }
}

#[derive(Debug)]
struct LibraryCoordinator {
    repository: Arc<dyn CleanupMutationRepository>,
    discovery: Arc<dyn LibraryDiscovery>,
    catalog: Arc<dyn LibraryCatalogSink>,
    effects: Arc<dyn LibraryMutationEffects>,
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
                        LibraryCommand::Mutate { mutation, reply } => {
                            let reconcile_metadata = mutation.needs_metadata_reconciliation();
                            let result = self.apply_mutation(mutation, true).await;
                            let applied = result.is_ok();
                            let _ = reply.send(result);
                            if applied && reconcile_metadata {
                                let _ = self.reconcile_once(cancellation.clone()).await;
                            }
                        }
                        LibraryCommand::MoveTrack {
                            track_id,
                            destination,
                            reply,
                        } => {
                            let result = self.move_track_once(track_id, destination, true).await;
                            let _ = reply.send(result);
                        }
                        LibraryCommand::DeleteTrack { track_id, reply } => {
                            let result = self.delete_track_once(track_id, true).await;
                            let _ = reply.send(result);
                        }
                        LibraryCommand::MoveTracks { requests, reply } => {
                            let result = self.move_tracks_once(requests).await;
                            let _ = reply.send(result);
                        }
                        LibraryCommand::DeleteTracks { track_ids, reply } => {
                            let result = self.delete_tracks_once(track_ids).await;
                            let _ = reply.send(result);
                        }
                        LibraryCommand::UpdateTrackMetadata {
                            track_id,
                            patch,
                            reply,
                        } => {
                            let result = self
                                .update_track_metadata_once(track_id, patch, true)
                                .await;
                            let _ = reply.send(result);
                        }
                        LibraryCommand::UpdateTracksMetadata {
                            track_ids,
                            patch,
                            reply,
                        } => {
                            let result = self.update_tracks_metadata_once(track_ids, patch).await;
                            let _ = reply.send(result);
                        }
                        LibraryCommand::PublishUploads {
                            uploads,
                            policy,
                            reply,
                        } => {
                            let result = self.publish_uploads_once(uploads, policy).await;
                            let _ = reply.send(result);
                        }
                        LibraryCommand::ApplyCleanup {
                            batch_id,
                            scope_label,
                            operations,
                            reply,
                        } => {
                            let result = self
                                .apply_cleanup_once(batch_id, scope_label, operations)
                                .await;
                            let _ = reply.send(result);
                        }
                        LibraryCommand::RevertCleanupBatch { batch_id, reply } => {
                            let result = self.revert_cleanup_batch_once(batch_id).await;
                            let _ = reply.send(result);
                        }
                        LibraryCommand::RevertCleanupJournal { items, reply } => {
                            let result = self.revert_cleanup_journal_once(items).await;
                            let _ = reply.send(result);
                        }
                    }
                }
            }
        }
    }

    async fn apply_mutation(
        &self,
        mutation: LibraryFileMutation,
        publish_catalog: bool,
    ) -> Result<AppliedLibraryMutation, LibraryCoordinatorError> {
        let operation = mutation
            .operation()
            .map_err(LibraryCoordinatorError::InvalidMutation)?;
        let draft = RecoveryJournalDraft::new(RecoveryDomain::Library, operation, mutation.plan())
            .map_err(|source| {
                dependency("validate a library mutation journal", Box::new(source))
            })?;
        let planned = self
            .repository
            .create_recovery_journal(draft)
            .await
            .map_err(|source| dependency("create a library mutation journal", source))?;
        let applying = transition_applied(
            self.repository
                .transition_recovery_journal(
                    &planned.id,
                    RecoveryState::Planned,
                    RecoveryState::Applying,
                    json!({}),
                )
                .await
                .map_err(|source| dependency("start a library mutation journal", source))?,
        )?;
        let outcome = match self
            .effects
            .apply(&applying.id, mutation.clone(), false)
            .await
        {
            Ok(outcome) => outcome,
            Err(failure) => {
                if !failure.requires_recovery() {
                    transition_applied(
                        self.repository
                            .transition_recovery_journal(
                                &applying.id,
                                RecoveryState::Applying,
                                RecoveryState::Failed,
                                json!({"error_code": failure.code()}),
                            )
                            .await
                            .map_err(|source| {
                                dependency("record a failed library mutation", source)
                            })?,
                    )?;
                }
                return Err(LibraryCoordinatorError::Mutation(failure));
            }
        };
        let (applied, status) = commit_applied_library_mutation(
            self.repository.as_ref(),
            &applying,
            &mutation,
            outcome,
        )
        .await?;
        if let Some(status) = status {
            let generation = status.generation;
            self.status.send_replace(status);
            if publish_catalog {
                let track_ids = self
                    .repository
                    .catalog_track_ids()
                    .await
                    .map_err(|source| dependency("load the mutated track catalog", source))?
                    .into_iter()
                    .collect();
                self.catalog
                    .publish(generation, track_ids)
                    .await
                    .map_err(|source| dependency("publish the mutated track catalog", source))?;
            }
        }
        Ok(applied)
    }

    async fn apply_cleanup_once(
        &self,
        batch_id: Option<i64>,
        scope_label: String,
        operations: Vec<CleanupApplyOperation>,
    ) -> Result<CleanupApplyResult, LibraryCoordinatorError> {
        if !(1..=MAX_CLEANUP_APPLY_OPERATIONS).contains(&operations.len()) {
            return Err(LibraryCoordinatorError::InvalidCleanupBatchSize);
        }
        if scope_label.chars().count() > MAX_CLEANUP_SCOPE_LABEL_CHARS {
            return Err(LibraryCoordinatorError::InvalidCleanupScopeLabel);
        }
        if let Some(batch_id) = batch_id {
            if batch_id <= 0 {
                return Err(LibraryCoordinatorError::CleanupBatchNotFound);
            }
            let batch = self
                .repository
                .cleanup_batch(batch_id)
                .await
                .map_err(|source| dependency("load the cleanup append target", source))?
                .ok_or(LibraryCoordinatorError::CleanupBatchNotFound)?;
            if batch.reverted_at_unix_seconds.is_some() {
                return Err(LibraryCoordinatorError::CleanupBatchReverted);
            }
        }

        let (mut regular, mut folders): (Vec<_>, Vec<_>) = operations
            .into_iter()
            .partition(|operation| operation.kind != CleanupOperationKind::FolderRename);
        folders.sort_by(|left, right| {
            cleanup_path_depth(&right.path).cmp(&cleanup_path_depth(&left.path))
        });
        regular.extend(folders);

        let mut current_batch_id = batch_id;
        let mut applied = 0_usize;
        let mut catalog_changed = false;
        let mut skipped = Vec::new();
        for operation in regular {
            let prepared = match self
                .prepare_cleanup_mutation(current_batch_id, &scope_label, operation)
                .await
            {
                Ok(prepared) => prepared,
                Err(CleanupPreparationError::Skip(skip)) => {
                    skipped.push(skip);
                    continue;
                }
                Err(CleanupPreparationError::Fatal(error)) => return Err(error),
            };
            match self.apply_cleanup_mutation(&prepared).await {
                Ok(commit) => {
                    current_batch_id = Some(commit.batch_id);
                    applied += 1;
                    catalog_changed |= commit.affected_tracks > 0;
                }
                Err(LibraryCoordinatorError::Mutation(failure)) if !failure.requires_recovery() => {
                    skipped.push(CleanupSkip {
                        track_id: prepared.track_id,
                        reason: cleanup_mutation_failure_reason(&prepared.kind, &failure),
                    });
                }
                Err(error) => return Err(error),
            }
        }
        if catalog_changed {
            self.publish_current_catalog().await?;
        }
        Ok(CleanupApplyResult {
            batch_id: current_batch_id,
            applied,
            skipped,
        })
    }

    async fn revert_cleanup_batch_once(
        &self,
        batch_id: i64,
    ) -> Result<CleanupRevertResult, LibraryCoordinatorError> {
        if batch_id <= 0 {
            return Err(LibraryCoordinatorError::CleanupBatchNotFound);
        }
        let batch = self
            .repository
            .cleanup_batch(batch_id)
            .await
            .map_err(|source| dependency("load the cleanup batch for reverting", source))?
            .ok_or(LibraryCoordinatorError::CleanupBatchNotFound)?;
        if batch.reverted_at_unix_seconds.is_some() {
            return Err(LibraryCoordinatorError::CleanupBatchReverted);
        }

        let operation = RecoveryOperation::parse("revert_batch").map_err(|source| {
            dependency("validate a cleanup batch revert journal", Box::new(source))
        })?;
        let draft = RecoveryJournalDraft::new(
            RecoveryDomain::Cleanup,
            operation,
            json!({"batch_id": batch_id}),
        )
        .map_err(|source| {
            dependency("validate a cleanup batch revert journal", Box::new(source))
        })?;
        let planned = self
            .repository
            .create_recovery_journal(draft)
            .await
            .map_err(|source| dependency("create a cleanup batch revert journal", source))?;
        let applying = transition_applied(
            self.repository
                .transition_recovery_journal(
                    &planned.id,
                    RecoveryState::Planned,
                    RecoveryState::Applying,
                    json!({}),
                )
                .await
                .map_err(|source| dependency("start a cleanup batch revert journal", source))?,
        )?;
        let result = self
            .revert_cleanup_items(Some(batch_id), batch.items)
            .await?;
        self.repository
            .finish_cleanup_batch_revert(
                &applying.id,
                batch_id,
                result.reverted,
                result.skipped.len(),
            )
            .await
            .map_err(|source| dependency("finish a cleanup batch revert", source))?;
        Ok(result)
    }

    async fn revert_cleanup_journal_once(
        &self,
        items: Vec<Map<String, Value>>,
    ) -> Result<CleanupRevertResult, LibraryCoordinatorError> {
        if !(1..=MAX_CLEANUP_REVERT_ITEMS).contains(&items.len()) {
            return Err(LibraryCoordinatorError::InvalidCleanupRevertSize);
        }
        self.revert_cleanup_items(None, items).await
    }

    async fn revert_cleanup_items(
        &self,
        batch_id: Option<i64>,
        items: Vec<Map<String, Value>>,
    ) -> Result<CleanupRevertResult, LibraryCoordinatorError> {
        let mut reverted = 0_usize;
        let mut catalog_changed = false;
        let mut skipped = Vec::new();
        for (item_index, item) in items.into_iter().enumerate().rev() {
            let prepared = match self
                .prepare_cleanup_revert(batch_id, item_index, &item)
                .await
            {
                Ok(prepared) => prepared,
                Err(CleanupPreparationError::Skip(skip)) => {
                    skipped.push(skip);
                    continue;
                }
                Err(CleanupPreparationError::Fatal(error)) => return Err(error),
            };
            match self.apply_cleanup_revert_mutation(&prepared).await {
                Ok(affected_tracks) => {
                    reverted += 1;
                    catalog_changed |= affected_tracks > 0;
                }
                Err(LibraryCoordinatorError::Mutation(failure)) if !failure.requires_recovery() => {
                    skipped.push(CleanupSkip {
                        track_id: prepared.track_id,
                        reason: cleanup_revert_failure_reason(&prepared.kind, &failure),
                    });
                }
                Err(error) => return Err(error),
            }
        }
        if catalog_changed {
            self.publish_current_catalog().await?;
        }
        Ok(CleanupRevertResult { reverted, skipped })
    }

    async fn prepare_cleanup_revert(
        &self,
        batch_id: Option<i64>,
        item_index: usize,
        item: &Map<String, Value>,
    ) -> Result<PreparedCleanupRevert, CleanupPreparationError> {
        let track_id = item.get("track_id").and_then(Value::as_i64).unwrap_or(0);
        match item.get("kind").and_then(Value::as_str) {
            Some("rename") => {
                self.prepare_cleanup_rename_revert(batch_id, item_index, track_id, item)
                    .await
            }
            Some("tag") => {
                self.prepare_cleanup_tag_revert(batch_id, item_index, track_id, item)
                    .await
            }
            Some("folder_rename") => {
                self.prepare_cleanup_folder_revert(batch_id, item_index, track_id, item)
            }
            kind => {
                Err(cleanup_skip(track_id, format!("unknown journal item kind: {kind:?}")).into())
            }
        }
    }

    async fn prepare_cleanup_rename_revert(
        &self,
        batch_id: Option<i64>,
        item_index: usize,
        recorded_track_id: i64,
        item: &Map<String, Value>,
    ) -> Result<PreparedCleanupRevert, CleanupPreparationError> {
        let (path_before, path_after) = cleanup_revert_paths(item)
            .map_err(|()| cleanup_skip(recorded_track_id, "malformed journal item"))?;
        let track = self
            .resolve_cleanup_revert_track(recorded_track_id, &path_after)
            .await?
            .ok_or_else(|| {
                cleanup_skip(
                    recorded_track_id,
                    "no track at the recorded path (renamed or removed since)",
                )
            })?;
        let original_name = path_before.file_name().to_owned();
        let revert = CleanupRevertMutation::new(batch_id, item_index).map_err(|error| {
            CleanupPreparationError::Fatal(LibraryCoordinatorError::InvalidCleanupMutation(error))
        })?;
        Ok(PreparedCleanupRevert {
            track_id: recorded_track_id,
            kind: PreparedCleanupRevertKind::Rename { original_name },
            mutation: LibraryFileMutation::MoveTrack {
                track_id: track.id,
                source: path_after,
                destination: path_before,
            },
            revert,
        })
    }

    async fn prepare_cleanup_tag_revert(
        &self,
        batch_id: Option<i64>,
        item_index: usize,
        recorded_track_id: i64,
        item: &Map<String, Value>,
    ) -> Result<PreparedCleanupRevert, CleanupPreparationError> {
        let field = item
            .get("field")
            .and_then(Value::as_str)
            .and_then(cleanup_tag_field)
            .ok_or_else(|| cleanup_skip(recorded_track_id, "malformed journal item"))?;
        let path = item
            .get("path")
            .and_then(Value::as_str)
            .ok_or(())
            .and_then(|path| LibraryPath::parse(path).map_err(|_| ()))
            .map_err(|()| cleanup_skip(recorded_track_id, "malformed journal item"))?;
        let track = self
            .resolve_cleanup_revert_track(recorded_track_id, &path)
            .await?
            .ok_or_else(|| {
                cleanup_skip(
                    recorded_track_id,
                    "no track at the recorded path (moved or removed since)",
                )
            })?;
        let expected = cleanup_input_from_json(item.get("new"))
            .map_err(|()| cleanup_skip(recorded_track_id, "malformed journal item"))?;
        let current = cleanup_track_value(&track, field);
        if !cleanup_values_match(current.as_ref(), expected.as_ref()) {
            return Err(cleanup_skip(
                recorded_track_id,
                format!("{} changed since this batch was applied", field.as_str()),
            )
            .into());
        }
        let restore_value = if item.contains_key("file_old") {
            item.get("file_old")
        } else {
            item.get("old")
        };
        let restore = cleanup_input_from_json(restore_value)
            .map_err(|()| cleanup_skip(recorded_track_id, "malformed journal item"))?;
        let patch = cleanup_tag_patch(field, restore.as_ref())
            .map_err(|_| cleanup_skip(recorded_track_id, "malformed journal item"))?;
        let revert = CleanupRevertMutation::new(batch_id, item_index).map_err(|error| {
            CleanupPreparationError::Fatal(LibraryCoordinatorError::InvalidCleanupMutation(error))
        })?;
        Ok(PreparedCleanupRevert {
            track_id: recorded_track_id,
            kind: PreparedCleanupRevertKind::Tag,
            mutation: LibraryFileMutation::UpdateTrackMetadata {
                track_id: track.id,
                path,
                patch,
            },
            revert,
        })
    }

    fn prepare_cleanup_folder_revert(
        &self,
        batch_id: Option<i64>,
        item_index: usize,
        recorded_track_id: i64,
        item: &Map<String, Value>,
    ) -> Result<PreparedCleanupRevert, CleanupPreparationError> {
        let (path_before, path_after) = cleanup_revert_paths(item)
            .map_err(|()| cleanup_skip(recorded_track_id, "malformed journal item"))?;
        let original_name = path_before.file_name().to_owned();
        let revert = CleanupRevertMutation::new(batch_id, item_index).map_err(|error| {
            CleanupPreparationError::Fatal(LibraryCoordinatorError::InvalidCleanupMutation(error))
        })?;
        Ok(PreparedCleanupRevert {
            track_id: recorded_track_id,
            kind: PreparedCleanupRevertKind::FolderRename { original_name },
            mutation: LibraryFileMutation::RenameFolder {
                source: path_after,
                destination: path_before,
            },
            revert,
        })
    }

    async fn resolve_cleanup_revert_track(
        &self,
        recorded_track_id: i64,
        path: &LibraryPath,
    ) -> Result<Option<IndexedTrack>, CleanupPreparationError> {
        if let Ok(track_id) = TrackId::new(recorded_track_id) {
            let track = self.repository.track(track_id).await.map_err(|source| {
                CleanupPreparationError::Fatal(dependency(
                    "load a cleanup revert track by id",
                    source,
                ))
            })?;
            if track.as_ref().is_some_and(|track| &track.path == path) {
                return Ok(track);
            }
        }
        self.repository.track_by_path(path).await.map_err(|source| {
            CleanupPreparationError::Fatal(dependency(
                "load a cleanup revert track by path",
                source,
            ))
        })
    }

    async fn prepare_cleanup_mutation(
        &self,
        batch_id: Option<i64>,
        scope_label: &str,
        operation: CleanupApplyOperation,
    ) -> Result<PreparedCleanupMutation, CleanupPreparationError> {
        match operation.kind {
            CleanupOperationKind::Rename => {
                self.prepare_cleanup_track_rename(batch_id, scope_label, operation)
                    .await
            }
            CleanupOperationKind::Tag => {
                self.prepare_cleanup_tag(batch_id, scope_label, operation)
                    .await
            }
            CleanupOperationKind::FolderRename => {
                self.prepare_cleanup_folder_rename(batch_id, scope_label, operation)
            }
        }
    }

    async fn prepare_cleanup_track_rename(
        &self,
        batch_id: Option<i64>,
        scope_label: &str,
        operation: CleanupApplyOperation,
    ) -> Result<PreparedCleanupMutation, CleanupPreparationError> {
        let track_id = TrackId::new(operation.track_id)
            .map_err(|_| cleanup_skip(operation.track_id, "track not found"))?;
        let track = self
            .repository
            .track(track_id)
            .await
            .map_err(|source| {
                CleanupPreparationError::Fatal(dependency(
                    "load a track for cleanup renaming",
                    source,
                ))
            })?
            .ok_or_else(|| cleanup_skip(operation.track_id, "track not found"))?;
        let current_stem = Path::new(track.path.file_name())
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if !cleanup_expected_text_matches(current_stem, operation.old.as_ref()) {
            return Err(cleanup_skip(operation.track_id, "filename changed since analysis").into());
        }
        let new_stem = cleanup_input_text(operation.new.as_ref());
        if !valid_cleanup_leaf(&new_stem, 255) {
            return Err(cleanup_skip(
                operation.track_id,
                format!("invalid target name: {new_stem:?}"),
            )
            .into());
        }
        let extension = Path::new(track.path.file_name())
            .extension()
            .and_then(|value| value.to_str());
        let file_name = extension.map_or_else(
            || new_stem.clone(),
            |extension| format!("{new_stem}.{extension}"),
        );
        let destination = track.path.parent().map_or_else(
            || LibraryPath::parse(&file_name),
            |parent| parent.join(&file_name),
        );
        let destination = destination.map_err(|error| {
            cleanup_skip(operation.track_id, format!("invalid target name: {error}"))
        })?;
        if destination == track.path {
            return Err(cleanup_skip(operation.track_id, "no change").into());
        }
        let item = serde_json::Map::from_iter([
            ("kind".to_owned(), json!("rename")),
            ("track_id".to_owned(), json!(operation.track_id)),
            ("path_before".to_owned(), json!(track.path.as_str())),
            ("path_after".to_owned(), json!(destination.as_str())),
        ]);
        let mutation = LibraryFileMutation::MoveTrack {
            track_id,
            source: track.path,
            destination,
        };
        let append =
            CleanupBatchAppend::new(batch_id, scope_label.to_owned(), item).map_err(|error| {
                CleanupPreparationError::Fatal(LibraryCoordinatorError::InvalidCleanupMutation(
                    error,
                ))
            })?;
        Ok(PreparedCleanupMutation {
            track_id: operation.track_id,
            kind: PreparedCleanupKind::Rename {
                target_name: file_name,
            },
            mutation,
            append,
        })
    }

    async fn prepare_cleanup_tag(
        &self,
        batch_id: Option<i64>,
        scope_label: &str,
        operation: CleanupApplyOperation,
    ) -> Result<PreparedCleanupMutation, CleanupPreparationError> {
        let track_id = TrackId::new(operation.track_id)
            .map_err(|_| cleanup_skip(operation.track_id, "track not found"))?;
        let track = self
            .repository
            .track(track_id)
            .await
            .map_err(|source| {
                CleanupPreparationError::Fatal(dependency(
                    "load a track for cleanup tagging",
                    source,
                ))
            })?
            .ok_or_else(|| cleanup_skip(operation.track_id, "track not found"))?;
        let field_name = operation.field.as_deref().unwrap_or_default();
        let field = cleanup_tag_field(field_name).ok_or_else(|| {
            cleanup_skip(
                operation.track_id,
                format!("unsupported tag field: {field_name:?}"),
            )
        })?;
        let current = cleanup_track_value(&track, field);
        if !cleanup_values_match(current.as_ref(), operation.old.as_ref()) {
            return Err(cleanup_skip(
                operation.track_id,
                format!("{} changed since analysis", field.as_str()),
            )
            .into());
        }
        let patch = cleanup_tag_patch(field, operation.new.as_ref()).map_err(|reason| {
            cleanup_skip(
                operation.track_id,
                format!("invalid {} value: {reason}", field.as_str()),
            )
        })?;
        let file_old = self
            .effects
            .read_file_tag(&track.path, field)
            .await
            .map_err(|failure| {
                let reason = if failure.kind() == LibraryMutationFailureKind::NotFound {
                    "source file missing on disk".to_owned()
                } else {
                    format!("tag read failed: {}", failure.code())
                };
                cleanup_skip(operation.track_id, reason)
            })?;
        let item = serde_json::Map::from_iter([
            ("kind".to_owned(), json!("tag")),
            ("track_id".to_owned(), json!(operation.track_id)),
            ("field".to_owned(), json!(field.as_str())),
            ("old".to_owned(), cleanup_input_json(operation.old.as_ref())),
            (
                "file_old".to_owned(),
                library_file_tag_json(file_old.as_ref()),
            ),
            ("new".to_owned(), cleanup_input_json(operation.new.as_ref())),
            ("path".to_owned(), json!(track.path.as_str())),
        ]);
        let mutation = LibraryFileMutation::UpdateTrackMetadata {
            track_id,
            path: track.path,
            patch,
        };
        let append =
            CleanupBatchAppend::new(batch_id, scope_label.to_owned(), item).map_err(|error| {
                CleanupPreparationError::Fatal(LibraryCoordinatorError::InvalidCleanupMutation(
                    error,
                ))
            })?;
        Ok(PreparedCleanupMutation {
            track_id: operation.track_id,
            kind: PreparedCleanupKind::Tag,
            mutation,
            append,
        })
    }

    fn prepare_cleanup_folder_rename(
        &self,
        batch_id: Option<i64>,
        scope_label: &str,
        operation: CleanupApplyOperation,
    ) -> Result<PreparedCleanupMutation, CleanupPreparationError> {
        let normalized = operation.path.trim_matches('/').replace('\\', "/");
        let normalized = normalized.trim_matches('/');
        if normalized.is_empty() {
            return Err(
                cleanup_skip(operation.track_id, "refusing to rename the music root").into(),
            );
        }
        let source = LibraryPath::parse(normalized).map_err(|error| {
            cleanup_skip(operation.track_id, format!("invalid folder name: {error}"))
        })?;
        if !cleanup_expected_text_matches(source.file_name(), operation.old.as_ref()) {
            return Err(cleanup_skip(operation.track_id, "folder changed since analysis").into());
        }
        let new_leaf = cleanup_input_text(operation.new.as_ref());
        if !valid_cleanup_leaf(&new_leaf, 200) {
            return Err(cleanup_skip(
                operation.track_id,
                format!("invalid folder name: {new_leaf:?}"),
            )
            .into());
        }
        let destination = source.parent().map_or_else(
            || LibraryPath::parse(&new_leaf),
            |parent| parent.join(&new_leaf),
        );
        let destination = destination.map_err(|error| {
            cleanup_skip(operation.track_id, format!("invalid folder name: {error}"))
        })?;
        if destination == source {
            return Err(cleanup_skip(operation.track_id, "no change").into());
        }
        let item = serde_json::Map::from_iter([
            ("kind".to_owned(), json!("folder_rename")),
            ("path_before".to_owned(), json!(source.as_str())),
            ("path_after".to_owned(), json!(destination.as_str())),
        ]);
        let mutation = LibraryFileMutation::RenameFolder {
            source,
            destination,
        };
        let append =
            CleanupBatchAppend::new(batch_id, scope_label.to_owned(), item).map_err(|error| {
                CleanupPreparationError::Fatal(LibraryCoordinatorError::InvalidCleanupMutation(
                    error,
                ))
            })?;
        Ok(PreparedCleanupMutation {
            track_id: operation.track_id,
            kind: PreparedCleanupKind::FolderRename {
                target_name: new_leaf,
            },
            mutation,
            append,
        })
    }

    async fn apply_cleanup_mutation(
        &self,
        prepared: &PreparedCleanupMutation,
    ) -> Result<AppliedCleanupMutation, LibraryCoordinatorError> {
        let operation = prepared
            .mutation
            .operation()
            .map_err(LibraryCoordinatorError::InvalidMutation)?;
        let plan = prepared
            .append
            .journal_plan(&prepared.mutation)
            .map_err(LibraryCoordinatorError::InvalidCleanupMutation)?;
        let draft = RecoveryJournalDraft::new(RecoveryDomain::Cleanup, operation, plan).map_err(
            |source| dependency("validate a cleanup mutation journal", Box::new(source)),
        )?;
        let planned = self
            .repository
            .create_recovery_journal(draft)
            .await
            .map_err(|source| dependency("create a cleanup mutation journal", source))?;
        let applying = transition_applied(
            self.repository
                .transition_recovery_journal(
                    &planned.id,
                    RecoveryState::Planned,
                    RecoveryState::Applying,
                    json!({}),
                )
                .await
                .map_err(|source| dependency("start a cleanup mutation journal", source))?,
        )?;
        let outcome = match self
            .effects
            .apply(&applying.id, prepared.mutation.clone(), false)
            .await
        {
            Ok(outcome) => outcome,
            Err(failure) => {
                if !failure.requires_recovery() {
                    transition_applied(
                        self.repository
                            .transition_recovery_journal(
                                &applying.id,
                                RecoveryState::Applying,
                                RecoveryState::Failed,
                                json!({"error_code": failure.code()}),
                            )
                            .await
                            .map_err(|source| {
                                dependency("record a failed cleanup mutation", source)
                            })?,
                    )?;
                }
                return Err(LibraryCoordinatorError::Mutation(failure));
            }
        };
        let commit = self
            .repository
            .commit_cleanup_mutation(&applying.id, &prepared.mutation, outcome, &prepared.append)
            .await
            .map_err(|source| dependency("commit a cleanup mutation", source))?;
        self.status.send_replace(commit.status);
        Ok(AppliedCleanupMutation {
            affected_tracks: commit.affected_tracks,
            batch_id: commit.batch_id,
        })
    }

    async fn apply_cleanup_revert_mutation(
        &self,
        prepared: &PreparedCleanupRevert,
    ) -> Result<u64, LibraryCoordinatorError> {
        let operation = prepared
            .mutation
            .operation()
            .map_err(LibraryCoordinatorError::InvalidMutation)?;
        let plan = prepared
            .revert
            .journal_plan(&prepared.mutation)
            .map_err(LibraryCoordinatorError::InvalidCleanupMutation)?;
        let draft = RecoveryJournalDraft::new(RecoveryDomain::Cleanup, operation, plan).map_err(
            |source| {
                dependency(
                    "validate a cleanup revert mutation journal",
                    Box::new(source),
                )
            },
        )?;
        let planned = self
            .repository
            .create_recovery_journal(draft)
            .await
            .map_err(|source| dependency("create a cleanup revert mutation journal", source))?;
        let applying = transition_applied(
            self.repository
                .transition_recovery_journal(
                    &planned.id,
                    RecoveryState::Planned,
                    RecoveryState::Applying,
                    json!({}),
                )
                .await
                .map_err(|source| dependency("start a cleanup revert mutation journal", source))?,
        )?;
        let outcome = match self
            .effects
            .apply(&applying.id, prepared.mutation.clone(), false)
            .await
        {
            Ok(outcome) => outcome,
            Err(failure) => {
                if !failure.requires_recovery() {
                    transition_applied(
                        self.repository
                            .transition_recovery_journal(
                                &applying.id,
                                RecoveryState::Applying,
                                RecoveryState::Failed,
                                json!({"error_code": failure.code()}),
                            )
                            .await
                            .map_err(|source| {
                                dependency("record a failed cleanup revert mutation", source)
                            })?,
                    )?;
                }
                return Err(LibraryCoordinatorError::Mutation(failure));
            }
        };
        let commit = self
            .repository
            .commit_cleanup_revert_mutation(
                &applying.id,
                &prepared.mutation,
                outcome,
                &prepared.revert,
            )
            .await
            .map_err(|source| dependency("commit a cleanup revert mutation", source))?;
        self.status.send_replace(commit.status);
        Ok(commit.affected_tracks)
    }

    async fn recover_cleanup_mutations(&self) -> Result<(), LibraryCoordinatorError> {
        let entries = self
            .repository
            .unfinished_recovery_journals(RecoveryDomain::Cleanup)
            .await
            .map_err(|source| dependency("load unfinished cleanup mutations", source))?;
        let mut apply_mutations = Vec::new();
        let mut revert_mutations = Vec::new();
        let mut batch_reverts = Vec::new();
        for entry in entries {
            if entry.operation.as_str() == "revert_batch" {
                if entry.plan.get("cleanup_batch").is_some()
                    || entry.plan.get("cleanup_revert").is_some()
                {
                    return Err(LibraryCoordinatorError::RecoveryConflict);
                }
                batch_reverts.push(entry);
            } else if entry.plan.get("cleanup_revert").is_some() {
                revert_mutations.push(entry);
            } else if entry.plan.get("cleanup_batch").is_some() {
                apply_mutations.push(entry);
            } else {
                return Err(LibraryCoordinatorError::RecoveryConflict);
            }
        }
        for entry in apply_mutations {
            self.recover_cleanup_apply_mutation(entry).await?;
        }
        for entry in revert_mutations {
            self.recover_cleanup_revert_mutation(entry).await?;
        }
        for entry in batch_reverts {
            self.recover_cleanup_batch_revert(entry).await?;
        }
        Ok(())
    }

    async fn recover_cleanup_apply_mutation(
        &self,
        entry: RecoveryJournalEntry,
    ) -> Result<(), LibraryCoordinatorError> {
        let mutation = LibraryFileMutation::from_journal(&entry)
            .map_err(LibraryCoordinatorError::InvalidMutation)?;
        let append = CleanupBatchAppend::from_journal(&entry)
            .map_err(LibraryCoordinatorError::InvalidCleanupMutation)?;
        let applying = self
            .resume_cleanup_mutation(entry, "resume a planned cleanup mutation")
            .await?;
        let Some(outcome) = self
            .recover_cleanup_file_effect(&applying, mutation.clone())
            .await?
        else {
            return Ok(());
        };
        let commit = self
            .repository
            .commit_cleanup_mutation(&applying.id, &mutation, outcome, &append)
            .await
            .map_err(|source| dependency("commit a recovered cleanup mutation", source))?;
        self.status.send_replace(commit.status);
        Ok(())
    }

    async fn recover_cleanup_revert_mutation(
        &self,
        entry: RecoveryJournalEntry,
    ) -> Result<(), LibraryCoordinatorError> {
        let mutation = LibraryFileMutation::from_journal(&entry)
            .map_err(LibraryCoordinatorError::InvalidMutation)?;
        let revert = CleanupRevertMutation::from_journal(&entry)
            .map_err(LibraryCoordinatorError::InvalidCleanupMutation)?;
        let applying = self
            .resume_cleanup_mutation(entry, "resume a planned cleanup revert mutation")
            .await?;
        let Some(outcome) = self
            .recover_cleanup_file_effect(&applying, mutation.clone())
            .await?
        else {
            return Ok(());
        };
        let commit = self
            .repository
            .commit_cleanup_revert_mutation(&applying.id, &mutation, outcome, &revert)
            .await
            .map_err(|source| dependency("commit a recovered cleanup revert mutation", source))?;
        self.status.send_replace(commit.status);
        Ok(())
    }

    async fn recover_cleanup_batch_revert(
        &self,
        entry: RecoveryJournalEntry,
    ) -> Result<(), LibraryCoordinatorError> {
        let plan = entry
            .plan
            .as_object()
            .filter(|plan| plan.len() == 1)
            .ok_or(LibraryCoordinatorError::RecoveryConflict)?;
        let batch_id = plan
            .get("batch_id")
            .and_then(Value::as_i64)
            .filter(|batch_id| *batch_id > 0)
            .ok_or(LibraryCoordinatorError::RecoveryConflict)?;
        let batch = self
            .repository
            .cleanup_batch(batch_id)
            .await
            .map_err(|source| dependency("load a recovering cleanup batch revert", source))?
            .ok_or(LibraryCoordinatorError::RecoveryConflict)?;
        if batch.reverted_at_unix_seconds.is_some() {
            return Err(LibraryCoordinatorError::RecoveryConflict);
        }
        let applying = self
            .resume_cleanup_mutation(entry, "resume a planned cleanup batch revert")
            .await?;
        let result = self
            .revert_cleanup_items(Some(batch_id), batch.items)
            .await?;
        self.repository
            .finish_cleanup_batch_revert(
                &applying.id,
                batch_id,
                result.reverted,
                result.skipped.len(),
            )
            .await
            .map_err(|source| dependency("finish a recovered cleanup batch revert", source))
    }

    async fn resume_cleanup_mutation(
        &self,
        entry: RecoveryJournalEntry,
        operation: &'static str,
    ) -> Result<RecoveryJournalEntry, LibraryCoordinatorError> {
        match entry.state {
            RecoveryState::Planned => transition_applied(
                self.repository
                    .transition_recovery_journal(
                        &entry.id,
                        RecoveryState::Planned,
                        RecoveryState::Applying,
                        json!({"recovered": true}),
                    )
                    .await
                    .map_err(|source| dependency(operation, source))?,
            ),
            RecoveryState::Applying => Ok(entry),
            RecoveryState::Committed
            | RecoveryState::RollingBack
            | RecoveryState::RolledBack
            | RecoveryState::Failed => Err(LibraryCoordinatorError::RecoveryConflict),
        }
    }

    async fn recover_cleanup_file_effect(
        &self,
        applying: &RecoveryJournalEntry,
        mutation: LibraryFileMutation,
    ) -> Result<Option<LibraryFileMutationOutcome>, LibraryCoordinatorError> {
        match self.effects.apply(&applying.id, mutation, true).await {
            Ok(outcome) => Ok(Some(outcome)),
            Err(failure) if !failure.requires_recovery() => {
                transition_applied(
                    self.repository
                        .transition_recovery_journal(
                            &applying.id,
                            RecoveryState::Applying,
                            RecoveryState::Failed,
                            json!({"error_code": failure.code(), "recovered": true}),
                        )
                        .await
                        .map_err(|source| {
                            dependency("record a failed recovered cleanup mutation", source)
                        })?,
                )?;
                Ok(None)
            }
            Err(failure) => Err(LibraryCoordinatorError::Mutation(failure)),
        }
    }

    async fn move_track_once(
        &self,
        track_id: TrackId,
        destination: LibraryPath,
        publish_catalog: bool,
    ) -> Result<IndexedTrack, LibraryCoordinatorError> {
        let track = self
            .repository
            .track(track_id)
            .await
            .map_err(|source| dependency("load a track for moving", source))?
            .ok_or(LibraryCoordinatorError::TrackNotFound { track_id })?;
        if track.path == destination {
            return Ok(track);
        }
        let mutation = LibraryFileMutation::MoveTrack {
            track_id,
            source: track.path,
            destination,
        };
        let applied = self.apply_mutation(mutation, publish_catalog).await?;
        if !matches!(
            applied.outcome,
            LibraryFileMutationOutcome::TrackMoved {
                track_id: moved_id,
                ..
            } if moved_id == track_id
        ) {
            return Err(LibraryCoordinatorError::InvalidMutationOutcome);
        }
        applied
            .track
            .ok_or(LibraryCoordinatorError::InvalidMutationOutcome)
    }

    async fn delete_track_once(
        &self,
        track_id: TrackId,
        publish_catalog: bool,
    ) -> Result<(), LibraryCoordinatorError> {
        let track = self
            .repository
            .track(track_id)
            .await
            .map_err(|source| dependency("load a track for deletion", source))?
            .ok_or(LibraryCoordinatorError::TrackNotFound { track_id })?;
        let mutation = LibraryFileMutation::DeleteTrack {
            track_id,
            path: track.path,
        };
        let applied = self.apply_mutation(mutation, publish_catalog).await?;
        if matches!(
            applied.outcome,
            LibraryFileMutationOutcome::TrackDeleted {
                track_id: deleted_id
            } if deleted_id == track_id
        ) {
            Ok(())
        } else {
            Err(LibraryCoordinatorError::InvalidMutationOutcome)
        }
    }

    async fn move_tracks_once(
        &self,
        requests: Vec<(TrackId, LibraryPath)>,
    ) -> Result<TrackMoveBatchResults, LibraryCoordinatorError> {
        let mut results = Vec::with_capacity(requests.len());
        let mut succeeded = false;
        let mut requests = requests.into_iter();
        while let Some((track_id, destination)) = requests.next() {
            let result = self.move_track_once(track_id, destination, false).await;
            succeeded |= result.is_ok();
            let stop = result.as_ref().is_err_and(mutation_error_requires_recovery);
            results.push((track_id, result));
            if stop {
                results.extend(requests.map(|(track_id, _)| {
                    (track_id, Err(LibraryCoordinatorError::RecoveryConflict))
                }));
                break;
            }
        }
        if succeeded {
            self.publish_current_catalog().await?;
        }
        Ok(results)
    }

    async fn delete_tracks_once(
        &self,
        track_ids: Vec<TrackId>,
    ) -> Result<TrackDeleteBatchResults, LibraryCoordinatorError> {
        let mut results = Vec::with_capacity(track_ids.len());
        let mut succeeded = false;
        let mut track_ids = track_ids.into_iter();
        while let Some(track_id) = track_ids.next() {
            let result = self.delete_track_once(track_id, false).await;
            succeeded |= result.is_ok();
            let stop = result.as_ref().is_err_and(mutation_error_requires_recovery);
            results.push((track_id, result));
            if stop {
                results
                    .extend(track_ids.map(|track_id| {
                        (track_id, Err(LibraryCoordinatorError::RecoveryConflict))
                    }));
                break;
            }
        }
        if succeeded {
            self.publish_current_catalog().await?;
        }
        Ok(results)
    }

    async fn update_track_metadata_once(
        &self,
        track_id: TrackId,
        patch: TrackMetadataPatch,
        publish_catalog: bool,
    ) -> Result<IndexedTrack, LibraryCoordinatorError> {
        let track = self
            .repository
            .track(track_id)
            .await
            .map_err(|source| dependency("load a track for metadata editing", source))?
            .ok_or(LibraryCoordinatorError::TrackNotFound { track_id })?;
        if patch.is_empty() {
            return Ok(track);
        }
        let mutation = LibraryFileMutation::UpdateTrackMetadata {
            track_id,
            path: track.path,
            patch,
        };
        let applied = self.apply_mutation(mutation, publish_catalog).await?;
        if !matches!(
            applied.outcome,
            LibraryFileMutationOutcome::TrackMetadataUpdated {
                track_id: updated_id,
                ..
            } if updated_id == track_id
        ) {
            return Err(LibraryCoordinatorError::InvalidMutationOutcome);
        }
        applied
            .track
            .ok_or(LibraryCoordinatorError::InvalidMutationOutcome)
    }

    async fn update_tracks_metadata_once(
        &self,
        track_ids: Vec<TrackId>,
        patch: TrackMetadataPatch,
    ) -> Result<TrackMetadataBatchResults, LibraryCoordinatorError> {
        let database_only = patch.database_only();
        let mut results = Vec::with_capacity(track_ids.len());
        let mut mutated = false;
        let mut track_ids = track_ids.into_iter();
        while let Some(track_id) = track_ids.next() {
            match self
                .update_track_metadata_once(track_id, patch.clone(), false)
                .await
            {
                Ok(track) => {
                    mutated |= !patch.is_empty();
                    results.push(TrackMetadataBatchItem {
                        track_id,
                        track: Some(track),
                        error: None,
                    });
                }
                Err(error) if mutation_error_requires_recovery(&error) => {
                    results.push(TrackMetadataBatchItem {
                        track_id,
                        track: None,
                        error: Some(error),
                    });
                    results.extend(track_ids.map(|track_id| TrackMetadataBatchItem {
                        track_id,
                        track: None,
                        error: Some(LibraryCoordinatorError::RecoveryConflict),
                    }));
                    break;
                }
                Err(tag_error) if !database_only.is_empty() && patch.has_tag_changes() => {
                    match self
                        .update_track_metadata_once(track_id, database_only.clone(), false)
                        .await
                    {
                        Ok(track) => {
                            mutated = true;
                            results.push(TrackMetadataBatchItem {
                                track_id,
                                track: Some(track),
                                error: Some(tag_error),
                            });
                        }
                        Err(error) => {
                            let stop = mutation_error_requires_recovery(&error);
                            results.push(TrackMetadataBatchItem {
                                track_id,
                                track: None,
                                error: Some(error),
                            });
                            if stop {
                                results.extend(track_ids.map(|track_id| TrackMetadataBatchItem {
                                    track_id,
                                    track: None,
                                    error: Some(LibraryCoordinatorError::RecoveryConflict),
                                }));
                                break;
                            }
                        }
                    }
                }
                Err(error) => results.push(TrackMetadataBatchItem {
                    track_id,
                    track: None,
                    error: Some(error),
                }),
            }
        }
        if mutated {
            self.publish_current_catalog().await?;
        }
        Ok(results)
    }

    async fn publish_uploads_once(
        &self,
        uploads: Vec<StagedLibraryUpload>,
        policy: UploadConflictPolicy,
    ) -> Result<Vec<LibraryUploadBatchItem>, LibraryCoordinatorError> {
        let mut results = Vec::with_capacity(uploads.len());
        let mut catalog_changed = false;
        let mut uploads = uploads.into_iter();
        while let Some(upload) = uploads.next() {
            let resolution = match self.effects.resolve_upload(&upload.requested, policy).await {
                Ok(resolution) => resolution,
                Err(failure) => {
                    let _ = self.effects.discard_upload(&upload.staged).await;
                    self.discard_staged_uploads(uploads).await;
                    return Err(LibraryCoordinatorError::Mutation(failure));
                }
            };
            let LibraryUploadResolution::Publish {
                destination,
                replace_existing,
            } = resolution
            else {
                if let Err(failure) = self.effects.discard_upload(&upload.staged).await {
                    self.discard_staged_uploads(uploads).await;
                    return Err(LibraryCoordinatorError::Mutation(failure));
                }
                results.push(LibraryUploadBatchItem::Skipped {
                    requested: upload.requested,
                });
                continue;
            };
            let mutation = LibraryFileMutation::PublishUpload {
                staged: upload.staged.clone(),
                destination: destination.clone(),
                replace_existing,
            };
            let applied = match self.apply_mutation(mutation, false).await {
                Ok(applied) => applied,
                Err(error) => {
                    if !mutation_error_requires_recovery(&error) {
                        let _ = self.effects.discard_upload(&upload.staged).await;
                    }
                    self.discard_staged_uploads(uploads).await;
                    if catalog_changed {
                        self.publish_current_catalog().await?;
                    }
                    return Err(error);
                }
            };
            catalog_changed |= applied.affected_tracks > 0;
            let LibraryFileMutationOutcome::UploadPublished {
                destination: published,
                ..
            } = applied.outcome
            else {
                self.discard_staged_uploads(uploads).await;
                if catalog_changed {
                    self.publish_current_catalog().await?;
                }
                return Err(LibraryCoordinatorError::InvalidMutationOutcome);
            };
            if published != destination {
                self.discard_staged_uploads(uploads).await;
                if catalog_changed {
                    self.publish_current_catalog().await?;
                }
                return Err(LibraryCoordinatorError::InvalidMutationOutcome);
            }
            results.push(LibraryUploadBatchItem::Published {
                destination: published,
                track: applied.track.map(Box::new),
            });
        }
        if catalog_changed {
            self.publish_current_catalog().await?;
        }
        Ok(results)
    }

    async fn discard_staged_uploads(&self, uploads: impl IntoIterator<Item = StagedLibraryUpload>) {
        for upload in uploads {
            let _ = self.effects.discard_upload(&upload.staged).await;
        }
    }

    async fn publish_current_catalog(&self) -> Result<(), LibraryCoordinatorError> {
        let status = self
            .repository
            .status()
            .await
            .map_err(|source| dependency("load the mutated library status", source))?;
        let track_ids = self
            .repository
            .catalog_track_ids()
            .await
            .map_err(|source| dependency("load the mutated track catalog", source))?
            .into_iter()
            .collect();
        let generation = status.generation;
        self.status.send_replace(status);
        self.catalog
            .publish(generation, track_ids)
            .await
            .map_err(|source| dependency("publish the mutated track catalog", source))
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

fn mutation_error_requires_recovery(error: &LibraryCoordinatorError) -> bool {
    match error {
        LibraryCoordinatorError::Dependency { .. }
        | LibraryCoordinatorError::RecoveryConflict
        | LibraryCoordinatorError::InvalidMutationOutcome => true,
        LibraryCoordinatorError::Mutation(failure) => failure.requires_recovery(),
        _ => false,
    }
}

fn cleanup_path_depth(path: &str) -> usize {
    path.bytes()
        .filter(|byte| matches!(*byte, b'/' | b'\\'))
        .count()
}

fn cleanup_skip(track_id: i64, reason: impl Into<String>) -> CleanupSkip {
    CleanupSkip {
        track_id,
        reason: reason.into(),
    }
}

fn cleanup_expected_text_matches(current: &str, expected: Option<&CleanupInputValue>) -> bool {
    match expected {
        Some(CleanupInputValue::Text(expected)) => current == expected,
        None => current.is_empty(),
        Some(CleanupInputValue::Integer(_)) => false,
    }
}

fn cleanup_input_text(value: Option<&CleanupInputValue>) -> String {
    match value {
        Some(CleanupInputValue::Text(value)) => value.clone(),
        Some(CleanupInputValue::Integer(value)) => value.to_string(),
        None => String::new(),
    }
}

fn valid_cleanup_leaf(name: &str, maximum: usize) -> bool {
    !name.is_empty()
        && name.trim() == name
        && !name.contains(['/', '\\'])
        && !name.starts_with('.')
        && name.chars().count() <= maximum
}

const fn cleanup_tag_field(field: &str) -> Option<TrackMetadataField> {
    match field.as_bytes() {
        b"title" => Some(TrackMetadataField::Title),
        b"artist" => Some(TrackMetadataField::Artist),
        b"album_artist" => Some(TrackMetadataField::AlbumArtist),
        b"album" => Some(TrackMetadataField::Album),
        b"track_no" => Some(TrackMetadataField::TrackNumber),
        b"disc_no" => Some(TrackMetadataField::DiscNumber),
        b"year" => Some(TrackMetadataField::Year),
        _ => None,
    }
}

fn cleanup_track_value(
    track: &IndexedTrack,
    field: TrackMetadataField,
) -> Option<CleanupInputValue> {
    match field {
        TrackMetadataField::Title => Some(CleanupInputValue::Text(track.metadata.title.clone())),
        TrackMetadataField::Artist => Some(CleanupInputValue::Text(track.metadata.artist.clone())),
        TrackMetadataField::AlbumArtist => {
            Some(CleanupInputValue::Text(track.metadata.album_artist.clone()))
        }
        TrackMetadataField::Album => Some(CleanupInputValue::Text(track.metadata.album.clone())),
        TrackMetadataField::TrackNumber => track
            .metadata
            .track_no
            .map(|value| CleanupInputValue::Integer(i64::from(value))),
        TrackMetadataField::DiscNumber => track
            .metadata
            .disc_no
            .map(|value| CleanupInputValue::Integer(i64::from(value))),
        TrackMetadataField::Year => track
            .metadata
            .year
            .map(|value| CleanupInputValue::Integer(i64::from(value))),
        TrackMetadataField::Genre
        | TrackMetadataField::Bpm
        | TrackMetadataField::DisplayTitle
        | TrackMetadataField::Origin => None,
    }
}

fn cleanup_values_match(
    current: Option<&CleanupInputValue>,
    expected: Option<&CleanupInputValue>,
) -> bool {
    current == expected || (cleanup_value_absent(current) && cleanup_value_absent(expected))
}

fn cleanup_value_absent(value: Option<&CleanupInputValue>) -> bool {
    value.is_none_or(|value| matches!(value, CleanupInputValue::Text(text) if text.is_empty()))
}

fn cleanup_tag_patch(
    field: TrackMetadataField,
    value: Option<&CleanupInputValue>,
) -> Result<TrackMetadataPatch, TrackMetadataPatchError> {
    let mut patch = TrackMetadataPatch::new();
    if field.is_numeric() {
        let value = match value {
            None => None,
            Some(CleanupInputValue::Text(value)) if value.is_empty() => None,
            Some(CleanupInputValue::Integer(value)) => Some(
                u32::try_from(*value)
                    .map_err(|_| TrackMetadataPatchError::NumberOutOfRange { field })?,
            ),
            Some(CleanupInputValue::Text(value)) => Some(
                value
                    .parse::<u32>()
                    .map_err(|_| TrackMetadataPatchError::WrongValueType { field })?,
            ),
        };
        patch.insert_number(field, value)?;
    } else {
        let value = cleanup_input_text(value);
        patch.insert_text(field, (!value.is_empty()).then_some(value))?;
    }
    Ok(patch)
}

fn cleanup_input_json(value: Option<&CleanupInputValue>) -> serde_json::Value {
    value.map_or(serde_json::Value::Null, CleanupInputValue::to_json)
}

fn cleanup_input_from_json(value: Option<&Value>) -> Result<Option<CleanupInputValue>, ()> {
    match value {
        Some(Value::Null) | None => Ok(None),
        Some(Value::String(value)) => Ok(Some(CleanupInputValue::Text(value.clone()))),
        Some(Value::Number(value)) => value
            .as_i64()
            .map(CleanupInputValue::Integer)
            .map(Some)
            .ok_or(()),
        Some(_) => Err(()),
    }
}

fn cleanup_revert_paths(item: &Map<String, Value>) -> Result<(LibraryPath, LibraryPath), ()> {
    let before = item
        .get("path_before")
        .and_then(Value::as_str)
        .ok_or(())
        .and_then(|path| LibraryPath::parse(path).map_err(|_| ()))?;
    let after = item
        .get("path_after")
        .and_then(Value::as_str)
        .ok_or(())
        .and_then(|path| LibraryPath::parse(path).map_err(|_| ()))?;
    Ok((before, after))
}

fn library_file_tag_json(value: Option<&LibraryFileTagValue>) -> serde_json::Value {
    match value {
        Some(LibraryFileTagValue::Text(value)) => json!(value),
        Some(LibraryFileTagValue::Number(value)) => json!(value),
        None => serde_json::Value::Null,
    }
}

fn cleanup_mutation_failure_reason(
    kind: &PreparedCleanupKind,
    failure: &LibraryMutationFailure,
) -> String {
    match (kind, failure.kind()) {
        (PreparedCleanupKind::Rename { .. }, LibraryMutationFailureKind::NotFound)
        | (PreparedCleanupKind::Tag, LibraryMutationFailureKind::NotFound) => {
            "source file missing on disk".to_owned()
        }
        (PreparedCleanupKind::Rename { target_name }, LibraryMutationFailureKind::Conflict) => {
            format!("a file named {target_name} already exists")
        }
        (PreparedCleanupKind::FolderRename { .. }, LibraryMutationFailureKind::NotFound) => {
            "folder missing on disk".to_owned()
        }
        (
            PreparedCleanupKind::FolderRename { target_name },
            LibraryMutationFailureKind::Conflict,
        ) => format!("a folder named {target_name} already exists"),
        (PreparedCleanupKind::Tag, LibraryMutationFailureKind::Invalid) => {
            format!("unsupported format: {}", failure.code())
        }
        (PreparedCleanupKind::Tag, _) => format!("tag write failed: {}", failure.code()),
        (PreparedCleanupKind::Rename { .. }, _) => {
            format!("file rename failed: {}", failure.code())
        }
        (PreparedCleanupKind::FolderRename { .. }, _) => {
            format!("folder rename failed: {}", failure.code())
        }
    }
}

fn cleanup_revert_failure_reason(
    kind: &PreparedCleanupRevertKind,
    failure: &LibraryMutationFailure,
) -> String {
    match (kind, failure.kind()) {
        (PreparedCleanupRevertKind::Rename { .. }, LibraryMutationFailureKind::NotFound)
        | (PreparedCleanupRevertKind::Tag, LibraryMutationFailureKind::NotFound) => {
            "file missing on disk".to_owned()
        }
        (
            PreparedCleanupRevertKind::Rename { original_name },
            LibraryMutationFailureKind::Conflict,
        ) => format!("original name {original_name} is taken"),
        (PreparedCleanupRevertKind::FolderRename { .. }, LibraryMutationFailureKind::NotFound) => {
            "no folder at the recorded path (moved or removed since)".to_owned()
        }
        (
            PreparedCleanupRevertKind::FolderRename { original_name },
            LibraryMutationFailureKind::Conflict,
        ) => format!("original folder name {original_name} is taken"),
        (PreparedCleanupRevertKind::Tag, _) => {
            format!("tag write failed: {}", failure.code())
        }
        (PreparedCleanupRevertKind::Rename { .. }, _) => {
            format!("file rename failed: {}", failure.code())
        }
        (PreparedCleanupRevertKind::FolderRename { .. }, _) => {
            format!("folder rename failed: {}", failure.code())
        }
    }
}

async fn recover_library_mutations<R>(
    repository: &R,
    effects: &dyn LibraryMutationEffects,
) -> Result<(), LibraryCoordinatorError>
where
    R: LibraryMutationRepository + ?Sized,
{
    let entries = repository
        .unfinished_recovery_journals(RecoveryDomain::Library)
        .await
        .map_err(|source| dependency("load unfinished library mutations", source))?;
    for entry in entries {
        let mutation = LibraryFileMutation::from_journal(&entry)
            .map_err(LibraryCoordinatorError::InvalidMutation)?;
        let applying = match entry.state {
            RecoveryState::Planned => transition_applied(
                repository
                    .transition_recovery_journal(
                        &entry.id,
                        RecoveryState::Planned,
                        RecoveryState::Applying,
                        json!({"recovered": true}),
                    )
                    .await
                    .map_err(|source| dependency("resume a planned library mutation", source))?,
            )?,
            RecoveryState::Applying => entry,
            RecoveryState::Committed
            | RecoveryState::RollingBack
            | RecoveryState::RolledBack
            | RecoveryState::Failed => return Err(LibraryCoordinatorError::RecoveryConflict),
        };
        let outcome = match effects.apply(&applying.id, mutation.clone(), true).await {
            Ok(outcome) => outcome,
            Err(failure) if !failure.requires_recovery() => {
                transition_applied(
                    repository
                        .transition_recovery_journal(
                            &applying.id,
                            RecoveryState::Applying,
                            RecoveryState::Failed,
                            json!({"error_code": failure.code()}),
                        )
                        .await
                        .map_err(|source| {
                            dependency("record a failed recovered library mutation", source)
                        })?,
                )?;
                continue;
            }
            Err(failure) => return Err(LibraryCoordinatorError::Mutation(failure)),
        };
        commit_applied_library_mutation(repository, &applying, &mutation, outcome).await?;
    }
    Ok(())
}

async fn commit_applied_library_mutation<R>(
    repository: &R,
    journal: &RecoveryJournalEntry,
    mutation: &LibraryFileMutation,
    outcome: LibraryFileMutationOutcome,
) -> Result<(AppliedLibraryMutation, Option<LibraryStatus>), LibraryCoordinatorError>
where
    R: LibraryMutationRepository + ?Sized,
{
    match (mutation, outcome) {
        (
            LibraryFileMutation::CreateFolder { path: expected },
            LibraryFileMutationOutcome::Folder { path, has_children },
        ) if &path == expected => {
            transition_applied(
                repository
                    .transition_recovery_journal(
                        &journal.id,
                        RecoveryState::Applying,
                        RecoveryState::Committed,
                        json!({"created": true}),
                    )
                    .await
                    .map_err(|source| dependency("commit a folder creation journal", source))?,
            )?;
            Ok((
                AppliedLibraryMutation {
                    outcome: LibraryFileMutationOutcome::Folder { path, has_children },
                    affected_tracks: 0,
                    track: None,
                },
                None,
            ))
        }
        (
            LibraryFileMutation::RenameFolder {
                source,
                destination,
            },
            LibraryFileMutationOutcome::Folder { path, has_children },
        ) if &path == destination => {
            let commit = repository
                .commit_folder_rename(&journal.id, source, destination)
                .await
                .map_err(|source| dependency("commit a folder rename", source))?;
            let affected_tracks = commit.affected_tracks;
            Ok((
                AppliedLibraryMutation {
                    outcome: LibraryFileMutationOutcome::Folder { path, has_children },
                    affected_tracks,
                    track: None,
                },
                Some(commit.status),
            ))
        }
        (LibraryFileMutation::DeleteFolder { path, .. }, LibraryFileMutationOutcome::Deleted) => {
            let commit = repository
                .commit_folder_delete(&journal.id, path)
                .await
                .map_err(|source| dependency("commit a folder deletion", source))?;
            let affected_tracks = commit.affected_tracks;
            Ok((
                AppliedLibraryMutation {
                    outcome: LibraryFileMutationOutcome::Deleted,
                    affected_tracks,
                    track: None,
                },
                Some(commit.status),
            ))
        }
        (
            LibraryFileMutation::MoveTrack {
                track_id,
                source,
                destination,
            },
            LibraryFileMutationOutcome::TrackMoved {
                track_id: moved_id,
                track,
            },
        ) if track_id == &moved_id && destination == &track.path => {
            let commit = repository
                .commit_track_move(&journal.id, *track_id, source, &track)
                .await
                .map_err(|source| dependency("commit a track move", source))?;
            Ok((
                AppliedLibraryMutation {
                    outcome: LibraryFileMutationOutcome::TrackMoved {
                        track_id: moved_id,
                        track,
                    },
                    affected_tracks: 1,
                    track: Some(commit.track),
                },
                Some(commit.status),
            ))
        }
        (
            LibraryFileMutation::DeleteTrack { track_id, path },
            LibraryFileMutationOutcome::TrackDeleted {
                track_id: deleted_id,
            },
        ) if track_id == &deleted_id => {
            let commit = repository
                .commit_track_delete(&journal.id, *track_id, path)
                .await
                .map_err(|source| dependency("commit a track deletion", source))?;
            let affected_tracks = commit.affected_tracks;
            Ok((
                AppliedLibraryMutation {
                    outcome: LibraryFileMutationOutcome::TrackDeleted {
                        track_id: deleted_id,
                    },
                    affected_tracks,
                    track: None,
                },
                Some(commit.status),
            ))
        }
        (
            LibraryFileMutation::UpdateTrackMetadata {
                track_id,
                path,
                patch,
            },
            LibraryFileMutationOutcome::TrackMetadataUpdated {
                track_id: updated_id,
                discovered,
            },
        ) if track_id == &updated_id
            && discovered.as_ref().is_none_or(|track| &track.path == path) =>
        {
            let commit = repository
                .commit_track_metadata(&journal.id, *track_id, path, patch, discovered.as_ref())
                .await
                .map_err(|source| dependency("commit a track metadata update", source))?;
            Ok((
                AppliedLibraryMutation {
                    outcome: LibraryFileMutationOutcome::TrackMetadataUpdated {
                        track_id: updated_id,
                        discovered,
                    },
                    affected_tracks: 1,
                    track: Some(commit.track),
                },
                Some(commit.status),
            ))
        }
        (
            LibraryFileMutation::PublishUpload {
                staged,
                destination,
                replace_existing,
            },
            LibraryFileMutationOutcome::UploadPublished {
                destination: published,
                discovered,
            },
        ) if destination == &published
            && discovered
                .as_ref()
                .is_none_or(|track| track.path == published) =>
        {
            let commit = repository
                .commit_upload(
                    &journal.id,
                    staged,
                    destination,
                    *replace_existing,
                    discovered.as_ref(),
                )
                .await
                .map_err(|source| dependency("commit a library upload", source))?;
            let affected_tracks = commit.affected_tracks;
            Ok((
                AppliedLibraryMutation {
                    outcome: LibraryFileMutationOutcome::UploadPublished {
                        destination: published,
                        discovered,
                    },
                    affected_tracks,
                    track: commit.track,
                },
                Some(commit.status),
            ))
        }
        _ => Err(LibraryCoordinatorError::InvalidMutationOutcome),
    }
}

fn transition_applied(
    transition: RecoveryTransition,
) -> Result<RecoveryJournalEntry, LibraryCoordinatorError> {
    match transition {
        RecoveryTransition::Applied(entry) => Ok(entry),
        RecoveryTransition::Conflict(_) => Err(LibraryCoordinatorError::RecoveryConflict),
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

    pub async fn all_tracks(&self) -> Result<Vec<IndexedTrack>, LibraryDependencyError> {
        self.repository.all_tracks().await
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
