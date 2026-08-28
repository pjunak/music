use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use music_domain::SfxPath;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::recovery::{
    RecoveryDomain, RecoveryJournalDraft, RecoveryJournalEntry, RecoveryJournalId,
    RecoveryJournalRepository, RecoveryOperation, RecoveryState, RecoveryTransition,
};

pub type SfxDependencyError = Box<dyn Error + Send + Sync>;
pub type SfxFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, SfxDependencyError>> + Send + 'a>>;
pub type SfxMutationFuture<'a> =
    Pin<Box<dyn Future<Output = Result<SfxMutationOutcome, SfxMutationFailure>> + Send + 'a>>;
pub type SfxUploadResolutionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<SfxUploadResolution, SfxMutationFailure>> + Send + 'a>>;
pub type SfxUploadDiscardFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), SfxMutationFailure>> + Send + 'a>>;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SfxFileRecord {
    pub name: String,
    pub path: SfxPath,
    pub size_bytes: u64,
    pub modified_at_unix_seconds: i64,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SfxFolderRecord {
    pub name: String,
    pub path: SfxPath,
    pub file_count: u64,
    pub has_children: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SfxUploadConflictPolicy {
    Rename,
    Overwrite,
    Skip,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SfxUploadResolution {
    Publish {
        destination: SfxPath,
        replace_existing: bool,
    },
    Skip,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StagedSfxUpload {
    pub requested: SfxPath,
    pub staged: SfxPath,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SfxUploadBatchItem {
    Published(SfxFileRecord),
    Skipped { requested: SfxPath },
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SfxMutation {
    CreateFolder {
        path: SfxPath,
    },
    RenameFolder {
        source: SfxPath,
        destination: SfxPath,
    },
    DeleteFolder {
        path: SfxPath,
        recursive: bool,
    },
    MoveFile {
        source: SfxPath,
        destination: SfxPath,
    },
    DeleteFile {
        path: SfxPath,
    },
    PublishUpload {
        staged: SfxPath,
        destination: SfxPath,
        replace_existing: bool,
    },
}

impl SfxMutation {
    pub fn operation(&self) -> Result<RecoveryOperation, SfxMutationValidationError> {
        RecoveryOperation::parse(match self {
            Self::CreateFolder { .. } => "create_folder",
            Self::RenameFolder { .. } => "rename_folder",
            Self::DeleteFolder { .. } => "delete_folder",
            Self::MoveFile { .. } => "move_file",
            Self::DeleteFile { .. } => "delete_file",
            Self::PublishUpload { .. } => "publish_upload",
        })
        .map_err(|_| SfxMutationValidationError::InvalidJournalOperation)
    }

    #[must_use]
    pub fn plan(&self) -> Value {
        match self {
            Self::CreateFolder { path } | Self::DeleteFile { path } => {
                json!({"path": path.as_str()})
            }
            Self::DeleteFolder { path, recursive } => {
                json!({"path": path.as_str(), "recursive": recursive})
            }
            Self::RenameFolder {
                source,
                destination,
            }
            | Self::MoveFile {
                source,
                destination,
            } => json!({
                "source": source.as_str(),
                "destination": destination.as_str(),
            }),
            Self::PublishUpload {
                staged,
                destination,
                replace_existing,
            } => json!({
                "staged": staged.as_str(),
                "destination": destination.as_str(),
                "replace_existing": replace_existing,
            }),
        }
    }

    pub fn from_journal(entry: &RecoveryJournalEntry) -> Result<Self, SfxMutationValidationError> {
        if entry.domain != RecoveryDomain::Sfx {
            return Err(SfxMutationValidationError::InvalidJournalDomain);
        }
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
                    .ok_or(SfxMutationValidationError::InvalidJournalPlan)?,
            }),
            "move_file" => Ok(Self::MoveFile {
                source: parse_path(&entry.plan, "source")?,
                destination: parse_path(&entry.plan, "destination")?,
            }),
            "delete_file" => Ok(Self::DeleteFile {
                path: parse_path(&entry.plan, "path")?,
            }),
            "publish_upload" => Ok(Self::PublishUpload {
                staged: parse_path(&entry.plan, "staged")?,
                destination: parse_path(&entry.plan, "destination")?,
                replace_existing: entry
                    .plan
                    .get("replace_existing")
                    .and_then(Value::as_bool)
                    .ok_or(SfxMutationValidationError::InvalidJournalPlan)?,
            }),
            _ => Err(SfxMutationValidationError::UnknownJournalOperation),
        }
    }
}

fn parse_path(plan: &Value, name: &'static str) -> Result<SfxPath, SfxMutationValidationError> {
    SfxPath::parse(
        plan.get(name)
            .and_then(Value::as_str)
            .ok_or(SfxMutationValidationError::InvalidJournalPlan)?,
    )
    .map_err(|_| SfxMutationValidationError::InvalidJournalPlan)
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SfxMutationOutcome {
    Folder(SfxFolderRecord),
    File(SfxFileRecord),
    Deleted,
}

pub trait SfxEffects: std::fmt::Debug + Send + Sync {
    fn list_files(&self) -> SfxFuture<'_, Vec<SfxFileRecord>>;

    fn list_folders(&self) -> SfxFuture<'_, Vec<SfxFolderRecord>>;

    fn list_directory<'a>(&'a self, path: Option<&'a SfxPath>)
    -> SfxFuture<'a, Vec<SfxFileRecord>>;

    fn target_exists<'a>(&'a self, path: &'a SfxPath) -> SfxFuture<'a, bool>;

    fn resolve_upload<'a>(
        &'a self,
        requested: &'a SfxPath,
        policy: SfxUploadConflictPolicy,
    ) -> SfxUploadResolutionFuture<'a>;

    fn discard_upload<'a>(&'a self, staged: &'a SfxPath) -> SfxUploadDiscardFuture<'a>;

    fn cleanup_orphans(&self) -> SfxUploadDiscardFuture<'_>;

    fn apply<'a>(
        &'a self,
        journal_id: &'a RecoveryJournalId,
        mutation: SfxMutation,
        replay: bool,
    ) -> SfxMutationFuture<'a>;
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SfxMutationFailureKind {
    NotFound,
    Conflict,
    NotEmpty,
    Invalid,
    Capacity,
    Io,
}

#[derive(Debug)]
pub struct SfxMutationFailure {
    kind: SfxMutationFailureKind,
    code: &'static str,
    recovery_required: bool,
    source: SfxDependencyError,
}

impl SfxMutationFailure {
    #[must_use]
    pub fn new(
        kind: SfxMutationFailureKind,
        code: &'static str,
        source: SfxDependencyError,
    ) -> Self {
        Self {
            kind,
            code,
            recovery_required: true,
            source,
        }
    }

    #[must_use]
    pub fn without_recovery(
        kind: SfxMutationFailureKind,
        code: &'static str,
        source: SfxDependencyError,
    ) -> Self {
        Self {
            kind,
            code,
            recovery_required: false,
            source,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> SfxMutationFailureKind {
        self.kind
    }

    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    #[must_use]
    pub const fn requires_recovery(&self) -> bool {
        self.recovery_required
    }
}

impl Display for SfxMutationFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)
    }
}

impl Error for SfxMutationFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SfxMutationValidationError {
    InvalidJournalDomain,
    InvalidJournalOperation,
    InvalidJournalPlan,
    UnknownJournalOperation,
}

impl Display for SfxMutationValidationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidJournalDomain => "SFX journal has the wrong recovery domain",
            Self::InvalidJournalOperation => "SFX journal operation is invalid",
            Self::InvalidJournalPlan => "SFX journal plan is invalid",
            Self::UnknownJournalOperation => "SFX journal operation is unknown",
        })
    }
}

impl Error for SfxMutationValidationError {}

#[derive(Debug)]
pub enum SfxCoordinatorError {
    InvalidMutation(SfxMutationValidationError),
    Mutation(SfxMutationFailure),
    Dependency {
        operation: &'static str,
        source: SfxDependencyError,
    },
    RecoveryConflict,
}

impl Display for SfxCoordinatorError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMutation(error) => Display::fmt(error, formatter),
            Self::Mutation(error) => Display::fmt(error, formatter),
            Self::Dependency { operation, .. } => write!(formatter, "failed to {operation}"),
            Self::RecoveryConflict => {
                formatter.write_str("SFX mutation recovery is required before more writes")
            }
        }
    }
}

impl Error for SfxCoordinatorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidMutation(source) => Some(source),
            Self::Mutation(source) => Some(source),
            Self::Dependency { source, .. } => Some(source.as_ref()),
            Self::RecoveryConflict => None,
        }
    }
}

impl From<SfxMutationValidationError> for SfxCoordinatorError {
    fn from(error: SfxMutationValidationError) -> Self {
        Self::InvalidMutation(error)
    }
}

#[derive(Debug)]
pub struct SfxCoordinator {
    journal: Arc<dyn RecoveryJournalRepository>,
    effects: Arc<dyn SfxEffects>,
    gate: Mutex<()>,
    recovery_required: AtomicBool,
}

impl SfxCoordinator {
    pub async fn start(
        journal: Arc<dyn RecoveryJournalRepository>,
        effects: Arc<dyn SfxEffects>,
    ) -> Result<Self, SfxCoordinatorError> {
        recover_sfx_mutations(journal.as_ref(), effects.as_ref()).await?;
        Ok(Self {
            journal,
            effects,
            gate: Mutex::new(()),
            recovery_required: AtomicBool::new(false),
        })
    }

    pub async fn list_files(&self) -> Result<Vec<SfxFileRecord>, SfxCoordinatorError> {
        let _guard = self.begin_access().await?;
        self.effects
            .list_files()
            .await
            .map_err(|source| dependency("list SFX files", source))
    }

    pub async fn list_folders(&self) -> Result<Vec<SfxFolderRecord>, SfxCoordinatorError> {
        let _guard = self.begin_access().await?;
        self.effects
            .list_folders()
            .await
            .map_err(|source| dependency("list SFX folders", source))
    }

    pub async fn list_directory(
        &self,
        path: Option<&SfxPath>,
    ) -> Result<Vec<SfxFileRecord>, SfxCoordinatorError> {
        let _guard = self.begin_access().await?;
        self.effects
            .list_directory(path)
            .await
            .map_err(|source| dependency("list an SFX directory", source))
    }

    pub async fn target_exists(&self, path: &SfxPath) -> Result<bool, SfxCoordinatorError> {
        let _guard = self.begin_access().await?;
        self.effects
            .target_exists(path)
            .await
            .map_err(|source| dependency("inspect an SFX upload target", source))
    }

    pub async fn create_folder(
        &self,
        path: SfxPath,
    ) -> Result<SfxFolderRecord, SfxCoordinatorError> {
        let outcome = self.mutate(SfxMutation::CreateFolder { path }).await?;
        match outcome {
            SfxMutationOutcome::Folder(folder) => Ok(folder),
            _ => Err(SfxCoordinatorError::RecoveryConflict),
        }
    }

    pub async fn rename_folder(
        &self,
        source: SfxPath,
        destination: SfxPath,
    ) -> Result<SfxFolderRecord, SfxCoordinatorError> {
        let outcome = self
            .mutate(SfxMutation::RenameFolder {
                source,
                destination,
            })
            .await?;
        match outcome {
            SfxMutationOutcome::Folder(folder) => Ok(folder),
            _ => Err(SfxCoordinatorError::RecoveryConflict),
        }
    }

    pub async fn delete_folder(
        &self,
        path: SfxPath,
        recursive: bool,
    ) -> Result<(), SfxCoordinatorError> {
        let outcome = self
            .mutate(SfxMutation::DeleteFolder { path, recursive })
            .await?;
        if outcome == SfxMutationOutcome::Deleted {
            Ok(())
        } else {
            Err(SfxCoordinatorError::RecoveryConflict)
        }
    }

    pub async fn move_file(
        &self,
        source: SfxPath,
        destination: SfxPath,
    ) -> Result<SfxFileRecord, SfxCoordinatorError> {
        let outcome = self
            .mutate(SfxMutation::MoveFile {
                source,
                destination,
            })
            .await?;
        match outcome {
            SfxMutationOutcome::File(file) => Ok(file),
            _ => Err(SfxCoordinatorError::RecoveryConflict),
        }
    }

    pub async fn delete_file(&self, path: SfxPath) -> Result<(), SfxCoordinatorError> {
        let outcome = self.mutate(SfxMutation::DeleteFile { path }).await?;
        if outcome == SfxMutationOutcome::Deleted {
            Ok(())
        } else {
            Err(SfxCoordinatorError::RecoveryConflict)
        }
    }

    pub async fn publish_uploads(
        &self,
        uploads: Vec<StagedSfxUpload>,
        policy: SfxUploadConflictPolicy,
    ) -> Result<Vec<SfxUploadBatchItem>, SfxCoordinatorError> {
        let _guard = self.begin_write().await?;
        let mut results = Vec::with_capacity(uploads.len());
        let mut remaining = uploads.into_iter();
        while let Some(upload) = remaining.next() {
            let resolution = match self.effects.resolve_upload(&upload.requested, policy).await {
                Ok(resolution) => resolution,
                Err(error) => {
                    self.discard_uploads(std::iter::once(upload).chain(remaining))
                        .await;
                    return Err(SfxCoordinatorError::Mutation(error));
                }
            };
            let SfxUploadResolution::Publish {
                destination,
                replace_existing,
            } = resolution
            else {
                if let Err(error) = self.effects.discard_upload(&upload.staged).await {
                    self.discard_uploads(remaining).await;
                    return Err(SfxCoordinatorError::Mutation(error));
                }
                results.push(SfxUploadBatchItem::Skipped {
                    requested: upload.requested,
                });
                continue;
            };
            let current_stage = upload.staged.clone();
            let mutation = SfxMutation::PublishUpload {
                staged: upload.staged,
                destination,
                replace_existing,
            };
            match self.execute_mutation(mutation).await {
                Ok(SfxMutationOutcome::File(file)) => {
                    results.push(SfxUploadBatchItem::Published(file));
                }
                Ok(_) => {
                    self.recovery_required.store(true, Ordering::Release);
                    self.discard_uploads(remaining).await;
                    return Err(SfxCoordinatorError::RecoveryConflict);
                }
                Err(error) => {
                    if !self.recovery_required.load(Ordering::Acquire) {
                        let _ = self.effects.discard_upload(&current_stage).await;
                    }
                    self.discard_uploads(remaining).await;
                    return Err(error);
                }
            }
        }
        Ok(results)
    }

    async fn mutate(
        &self,
        mutation: SfxMutation,
    ) -> Result<SfxMutationOutcome, SfxCoordinatorError> {
        let _guard = self.begin_write().await?;
        self.execute_mutation(mutation).await
    }

    async fn begin_write(&self) -> Result<tokio::sync::MutexGuard<'_, ()>, SfxCoordinatorError> {
        self.begin_access().await
    }

    async fn begin_access(&self) -> Result<tokio::sync::MutexGuard<'_, ()>, SfxCoordinatorError> {
        if self.recovery_required.load(Ordering::Acquire) {
            return Err(SfxCoordinatorError::RecoveryConflict);
        }
        let guard = self.gate.lock().await;
        if self.recovery_required.load(Ordering::Acquire) {
            return Err(SfxCoordinatorError::RecoveryConflict);
        }
        Ok(guard)
    }

    async fn execute_mutation(
        &self,
        mutation: SfxMutation,
    ) -> Result<SfxMutationOutcome, SfxCoordinatorError> {
        let draft =
            RecoveryJournalDraft::new(RecoveryDomain::Sfx, mutation.operation()?, mutation.plan())
                .map_err(|_| {
                    SfxCoordinatorError::InvalidMutation(
                        SfxMutationValidationError::InvalidJournalPlan,
                    )
                })?;
        let planned = self
            .journal
            .create_recovery_journal(draft)
            .await
            .map_err(|source| dependency("create an SFX recovery journal", source))?;
        let applying = match transition(
            self.journal.as_ref(),
            &planned,
            RecoveryState::Applying,
            json!({"stage": "applying"}),
        )
        .await
        {
            Ok(entry) => entry,
            Err(error) => {
                self.recovery_required.store(true, Ordering::Release);
                return Err(error);
            }
        };
        let outcome = match self.effects.apply(&applying.id, mutation, false).await {
            Ok(outcome) => outcome,
            Err(failure) if !failure.requires_recovery() => {
                if transition(
                    self.journal.as_ref(),
                    &applying,
                    RecoveryState::Failed,
                    json!({"error_code": failure.code()}),
                )
                .await
                .is_err()
                {
                    self.recovery_required.store(true, Ordering::Release);
                    return Err(SfxCoordinatorError::RecoveryConflict);
                }
                return Err(SfxCoordinatorError::Mutation(failure));
            }
            Err(failure) => {
                self.recovery_required.store(true, Ordering::Release);
                return Err(SfxCoordinatorError::Mutation(failure));
            }
        };
        if let Err(error) = transition(
            self.journal.as_ref(),
            &applying,
            RecoveryState::Committed,
            json!({"stage": "committed"}),
        )
        .await
        {
            self.recovery_required.store(true, Ordering::Release);
            return Err(error);
        }
        Ok(outcome)
    }

    async fn discard_uploads(&self, uploads: impl IntoIterator<Item = StagedSfxUpload>) {
        for upload in uploads {
            let _ = self.effects.discard_upload(&upload.staged).await;
        }
    }
}

async fn recover_sfx_mutations(
    journal: &dyn RecoveryJournalRepository,
    effects: &dyn SfxEffects,
) -> Result<(), SfxCoordinatorError> {
    let entries = journal
        .unfinished_recovery_journals(RecoveryDomain::Sfx)
        .await
        .map_err(|source| dependency("load unfinished SFX mutations", source))?;
    for entry in entries {
        let mutation = SfxMutation::from_journal(&entry)?;
        let applying = match entry.state {
            RecoveryState::Planned => {
                transition(
                    journal,
                    &entry,
                    RecoveryState::Applying,
                    json!({"stage": "startup_replay"}),
                )
                .await?
            }
            RecoveryState::Applying => entry,
            RecoveryState::Committed
            | RecoveryState::RollingBack
            | RecoveryState::RolledBack
            | RecoveryState::Failed => return Err(SfxCoordinatorError::RecoveryConflict),
        };
        match effects.apply(&applying.id, mutation, true).await {
            Ok(_) => {
                transition(
                    journal,
                    &applying,
                    RecoveryState::Committed,
                    json!({"stage": "startup_committed"}),
                )
                .await?;
            }
            Err(failure) if !failure.requires_recovery() => {
                transition(
                    journal,
                    &applying,
                    RecoveryState::Failed,
                    json!({"error_code": failure.code()}),
                )
                .await?;
            }
            Err(failure) => return Err(SfxCoordinatorError::Mutation(failure)),
        }
    }
    effects
        .cleanup_orphans()
        .await
        .map_err(SfxCoordinatorError::Mutation)
}

async fn transition(
    journal: &dyn RecoveryJournalRepository,
    current: &RecoveryJournalEntry,
    next: RecoveryState,
    progress: Value,
) -> Result<RecoveryJournalEntry, SfxCoordinatorError> {
    match journal
        .transition_recovery_journal(&current.id, current.state, next, progress)
        .await
        .map_err(|source| dependency("transition an SFX recovery journal", source))?
    {
        RecoveryTransition::Applied(entry) => Ok(entry),
        RecoveryTransition::Conflict(_) => Err(SfxCoordinatorError::RecoveryConflict),
    }
}

fn dependency(operation: &'static str, source: SfxDependencyError) -> SfxCoordinatorError {
    SfxCoordinatorError::Dependency { operation, source }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use crate::recovery::{RecoveryOperation, RecoveryState};

    use super::*;

    #[test]
    fn mutation_plans_round_trip_through_typed_paths() -> Result<(), Box<dyn Error>> {
        let mutations = [
            SfxMutation::CreateFolder {
                path: SfxPath::parse("Scenes/Storm")?,
            },
            SfxMutation::RenameFolder {
                source: SfxPath::parse("Scenes/Storm")?,
                destination: SfxPath::parse("Scenes/Rain")?,
            },
            SfxMutation::DeleteFolder {
                path: SfxPath::parse("Old")?,
                recursive: true,
            },
            SfxMutation::MoveFile {
                source: SfxPath::parse("a/bell.wav")?,
                destination: SfxPath::parse("b/chime.wav")?,
            },
            SfxMutation::DeleteFile {
                path: SfxPath::parse("b/chime.wav")?,
            },
            SfxMutation::PublishUpload {
                staged: SfxPath::parse("b/.sfx-upload-123.partial")?,
                destination: SfxPath::parse("b/chime.wav")?,
                replace_existing: true,
            },
        ];
        for mutation in mutations {
            let entry = RecoveryJournalEntry {
                id: RecoveryJournalId::new(),
                domain: RecoveryDomain::Sfx,
                operation: RecoveryOperation::parse(mutation.operation()?.as_str())?,
                state: RecoveryState::Applying,
                plan: mutation.plan(),
                progress: json!({}),
                created_at_unix_seconds: 1,
                updated_at_unix_seconds: 1,
                completed_at_unix_seconds: None,
            };
            assert_eq!(SfxMutation::from_journal(&entry)?, mutation);
        }
        Ok(())
    }
}
