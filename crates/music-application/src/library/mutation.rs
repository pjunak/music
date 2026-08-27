use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;

use music_domain::{LibraryPath, TrackId};
use serde_json::{Value, json};

use crate::recovery::{RecoveryJournalEntry, RecoveryOperation};

use super::{LibraryDependencyError, LibraryStatus};

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
}

impl LibraryFileMutation {
    pub fn operation(&self) -> Result<RecoveryOperation, LibraryMutationValidationError> {
        RecoveryOperation::parse(match self {
            Self::CreateFolder { .. } => "create_folder",
            Self::RenameFolder { .. } => "rename_folder",
            Self::DeleteFolder { .. } => "delete_folder",
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
            _ => Err(LibraryMutationValidationError::UnknownJournalOperation),
        }
    }

    #[must_use]
    pub const fn needs_metadata_reconciliation(&self) -> bool {
        matches!(*self, Self::RenameFolder { .. })
    }
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
    use music_domain::LibraryPath;

    use super::LibraryFileMutation;
    use crate::recovery::{
        RecoveryDomain, RecoveryJournalDraft, RecoveryJournalEntry, RecoveryState,
    };

    #[test]
    fn journal_plans_round_trip_through_validated_paths() -> Result<(), Box<dyn std::error::Error>>
    {
        let mutation = LibraryFileMutation::RenameFolder {
            source: LibraryPath::parse("Old/Album")?,
            destination: LibraryPath::parse("New/Album")?,
        };
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
        Ok(())
    }
}
