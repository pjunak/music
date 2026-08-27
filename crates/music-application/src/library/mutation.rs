use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;

use music_domain::{IndexedTrack, LibraryPath, TrackId};
use serde_json::{Value, json};

use crate::recovery::{RecoveryJournalEntry, RecoveryOperation};

use super::{DiscoveredTrack, LibraryDependencyError, LibraryStatus};

pub type LibraryMutationFuture<'a> = Pin<
    Box<
        dyn Future<Output = Result<LibraryFileMutationOutcome, LibraryMutationFailure>> + Send + 'a,
    >,
>;

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum LibraryFileMutation {
    CreateFolder {
        path: LibraryPath,
    },
    RenameFolder {
        source: LibraryPath,
        destination: LibraryPath,
    },
    DeleteFolder {
        path: LibraryPath,
        recursive: bool,
    },
    MoveTrack {
        track_id: TrackId,
        source: LibraryPath,
        destination: LibraryPath,
    },
    DeleteTrack {
        track_id: TrackId,
        path: LibraryPath,
    },
}

impl LibraryFileMutation {
    pub fn operation(&self) -> Result<RecoveryOperation, LibraryMutationValidationError> {
        RecoveryOperation::parse(match self {
            Self::CreateFolder { .. } => "create_folder",
            Self::RenameFolder { .. } => "rename_folder",
            Self::DeleteFolder { .. } => "delete_folder",
            Self::MoveTrack { .. } => "move_track",
            Self::DeleteTrack { .. } => "delete_track",
        })
        .map_err(|_| LibraryMutationValidationError::InvalidJournalOperation)
    }

    #[must_use]
    pub fn plan(&self) -> Value {
        match self {
            Self::CreateFolder { path } => json!({"path": path.as_str()}),
            Self::RenameFolder {
                source,
                destination,
            } => json!({
                "source": source.as_str(),
                "destination": destination.as_str(),
            }),
            Self::DeleteFolder { path, recursive } => {
                json!({"path": path.as_str(), "recursive": recursive})
            }
            Self::MoveTrack {
                track_id,
                source,
                destination,
            } => json!({
                "track_id": track_id.get(),
                "source": source.as_str(),
                "destination": destination.as_str(),
            }),
            Self::DeleteTrack { track_id, path } => {
                json!({"track_id": track_id.get(), "path": path.as_str()})
            }
        }
    }

    pub fn from_journal(
        entry: &RecoveryJournalEntry,
    ) -> Result<Self, LibraryMutationValidationError> {
        match entry.operation.as_str() {
            "create_folder" => Ok(Self::CreateFolder {
                path: parse_path(&entry.plan, "path")?,
            }),
            "rename_folder" => Ok(Self::RenameFolder {
                source: parse_path(&entry.plan, "source")?,
                destination: parse_path(&entry.plan, "destination")?,
            }),
            "delete_folder" => Ok(Self::DeleteFolder {
                path: parse_path(&entry.plan, "path")?,
                recursive: entry
                    .plan
                    .get("recursive")
                    .and_then(Value::as_bool)
                    .ok_or(LibraryMutationValidationError::InvalidJournalPlan)?,
            }),
            "move_track" => Ok(Self::MoveTrack {
                track_id: parse_track_id(&entry.plan)?,
                source: parse_path(&entry.plan, "source")?,
                destination: parse_path(&entry.plan, "destination")?,
            }),
            "delete_track" => Ok(Self::DeleteTrack {
                track_id: parse_track_id(&entry.plan)?,
                path: parse_path(&entry.plan, "path")?,
            }),
            _ => Err(LibraryMutationValidationError::UnknownJournalOperation),
        }
    }

    #[must_use]
    pub const fn needs_metadata_reconciliation(&self) -> bool {
        matches!(*self, Self::RenameFolder { .. })
    }
}

fn parse_track_id(plan: &Value) -> Result<TrackId, LibraryMutationValidationError> {
    TrackId::new(
        plan.get("track_id")
            .and_then(Value::as_i64)
            .ok_or(LibraryMutationValidationError::InvalidJournalPlan)?,
    )
    .map_err(|_| LibraryMutationValidationError::InvalidJournalPlan)
}

fn parse_path(
    plan: &Value,
    name: &'static str,
) -> Result<LibraryPath, LibraryMutationValidationError> {
    LibraryPath::parse(
        plan.get(name)
            .and_then(Value::as_str)
            .ok_or(LibraryMutationValidationError::InvalidJournalPlan)?,
    )
    .map_err(|_| LibraryMutationValidationError::InvalidJournalPlan)
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum LibraryFileMutationOutcome {
    Folder {
        path: LibraryPath,
        has_children: bool,
    },
    TrackMoved {
        track_id: TrackId,
        track: DiscoveredTrack,
    },
    TrackDeleted {
        track_id: TrackId,
    },
    Deleted,
}

pub trait LibraryMutationEffects: std::fmt::Debug + Send + Sync {
    fn apply(&self, mutation: LibraryFileMutation, replay: bool) -> LibraryMutationFuture<'_>;
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LibraryMutationFailureKind {
    NotFound,
    Conflict,
    NotEmpty,
    Invalid,
    Io,
}

#[derive(Debug)]
pub struct LibraryMutationFailure {
    kind: LibraryMutationFailureKind,
    code: &'static str,
    source: LibraryDependencyError,
}

impl LibraryMutationFailure {
    #[must_use]
    pub fn new(
        kind: LibraryMutationFailureKind,
        code: &'static str,
        source: LibraryDependencyError,
    ) -> Self {
        Self { kind, code, source }
    }

    #[must_use]
    pub const fn kind(&self) -> LibraryMutationFailureKind {
        self.kind
    }

    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }
}

impl Display for LibraryMutationFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "library mutation failed ({})", self.code)
    }
}

impl Error for LibraryMutationFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LibraryIndexMutationCommit {
    pub status: LibraryStatus,
    pub track_ids: std::collections::BTreeSet<TrackId>,
    pub affected_tracks: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LibraryTrackMutationCommit {
    pub status: LibraryStatus,
    pub track_ids: std::collections::BTreeSet<TrackId>,
    pub track: IndexedTrack,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FolderMutationResult {
    pub path: LibraryPath,
    pub has_children: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct FolderDeletionResult {
    pub removed_tracks: u64,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LibraryMutationValidationError {
    InvalidJournalOperation,
    UnknownJournalOperation,
    InvalidJournalPlan,
}

impl Display for LibraryMutationValidationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidJournalOperation => "library journal operation is invalid",
            Self::UnknownJournalOperation => "library journal operation is unknown",
            Self::InvalidJournalPlan => "library journal plan is invalid",
        })
    }
}

impl Error for LibraryMutationValidationError {}

#[cfg(test)]
mod tests {
    use music_domain::{LibraryPath, TrackId};

    use super::LibraryFileMutation;
    use crate::recovery::{
        RecoveryDomain, RecoveryJournalDraft, RecoveryJournalEntry, RecoveryState,
    };

    #[test]
    fn journal_plans_round_trip_through_validated_paths() -> Result<(), Box<dyn std::error::Error>>
    {
        let mutations = [
            LibraryFileMutation::RenameFolder {
                source: LibraryPath::parse("Old/Album")?,
                destination: LibraryPath::parse("New/Album")?,
            },
            LibraryFileMutation::MoveTrack {
                track_id: TrackId::new(17)?,
                source: LibraryPath::parse("Old/track.mp3")?,
                destination: LibraryPath::parse("New/track.mp3")?,
            },
            LibraryFileMutation::DeleteTrack {
                track_id: TrackId::new(17)?,
                path: LibraryPath::parse("New/track.mp3")?,
            },
        ];
        for mutation in mutations {
            let draft = RecoveryJournalDraft::new(
                RecoveryDomain::Library,
                mutation.operation()?,
                mutation.plan(),
            )?;
            let entry = RecoveryJournalEntry {
                id: draft.id,
                domain: draft.domain,
                operation: draft.operation,
                state: RecoveryState::Applying,
                plan: draft.plan,
                progress: draft.progress,
                created_at_unix_seconds: 1,
                updated_at_unix_seconds: 1,
                completed_at_unix_seconds: None,
            };
            assert_eq!(LibraryFileMutation::from_journal(&entry)?, mutation);
        }
        Ok(())
    }
}
