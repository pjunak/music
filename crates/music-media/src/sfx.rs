use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use music_application::recovery::RecoveryJournalId;
use music_application::sfx::{
    SfxEffects, SfxFileRecord, SfxFolderRecord, SfxFuture, SfxMutation, SfxMutationFailure,
    SfxMutationFailureKind, SfxMutationFuture, SfxMutationOutcome, SfxUploadConflictPolicy,
    SfxUploadDiscardFuture, SfxUploadResolution, SfxUploadResolutionFuture,
};
use music_domain::SfxPath;
use uuid::Uuid;

use crate::{RootedPathError, SfxRoot};

const MAX_SFX_ENTRIES: usize = 100_000;
const MAX_UPLOAD_RENAME_ATTEMPTS: u32 = 10_000;
const SFX_EXTENSIONS: [&str; 7] = ["aac", "flac", "m4a", "mp3", "ogg", "opus", "wav"];

#[derive(Debug, Clone)]
pub struct FilesystemSfxEffects {
    root: SfxRoot,
}

impl FilesystemSfxEffects {
    #[must_use]
    pub const fn new(root: SfxRoot) -> Self {
        Self { root }
    }

    fn apply_blocking(
        &self,
        journal_id: &RecoveryJournalId,
        mutation: SfxMutation,
        replay: bool,
    ) -> Result<SfxMutationOutcome, FilesystemSfxError> {
        match mutation {
            SfxMutation::CreateFolder { path } => {
                let absolute = self
                    .root
                    .ensure_directory(&path)
                    .map_err(FilesystemSfxError::RootedPath)?;
                Ok(SfxMutationOutcome::Folder(folder_record(&path, &absolute)?))
            }
            SfxMutation::RenameFolder {
                source,
                destination,
            } => self.rename_folder(journal_id, source, destination, replay),
            SfxMutation::DeleteFolder { path, recursive } => {
                self.delete_folder(&path, recursive, replay)?;
                Ok(SfxMutationOutcome::Deleted)
            }
            SfxMutation::MoveFile {
                source,
                destination,
            } => self.move_file(journal_id, source, destination, replay),
            SfxMutation::DeleteFile { path } => {
                self.delete_file(&path, replay)?;
                Ok(SfxMutationOutcome::Deleted)
            }
            SfxMutation::PublishUpload {
                staged,
                destination,
                replace_existing,
            } => self.publish_upload(journal_id, &staged, destination, replace_existing, replay),
        }
    }

    fn rename_folder(
        &self,
        journal_id: &RecoveryJournalId,
        source: SfxPath,
        destination: SfxPath,
        replay: bool,
    ) -> Result<SfxMutationOutcome, FilesystemSfxError> {
        let source_folded = source.as_str().to_lowercase();
        let destination_folded = destination.as_str().to_lowercase();
        if source == destination
            || destination
                .as_str()
                .starts_with(&format!("{}/", source.as_str()))
            || (cfg!(windows) && destination_folded.starts_with(&format!("{source_folded}/")))
        {
            return Err(FilesystemSfxError::InvalidMove);
        }
        let source_absolute = self
            .root
            .resolve_for_creation(&source)
            .map_err(FilesystemSfxError::RootedPath)?;
        match entry_state(&source_absolute, EntryKind::Directory)? {
            EntryState::Missing if replay => {
                let destination_absolute = self
                    .root
                    .resolve_existing_directory(&destination)
                    .map_err(FilesystemSfxError::RootedPath)?;
                return Ok(SfxMutationOutcome::Folder(folder_record(
                    &destination,
                    &destination_absolute,
                )?));
            }
            EntryState::Missing => return Err(FilesystemSfxError::NotFound),
            EntryState::Occupied => return Err(FilesystemSfxError::InvalidPath),
            EntryState::Present => {}
        }
        if let Some(parent) = destination.parent() {
            self.root
                .ensure_directory(&parent)
                .map_err(FilesystemSfxError::RootedPath)?;
        }
        let destination_absolute = self
            .root
            .resolve_for_creation(&destination)
            .map_err(FilesystemSfxError::RootedPath)?;
        let case_only = cfg!(windows)
            && source.as_str() != destination.as_str()
            && source_folded == destination_folded
            && source
                .parent()
                .map(|path| path.into_string().to_lowercase())
                == destination
                    .parent()
                    .map(|path| path.into_string().to_lowercase());

        if case_only {
            let temporary = rename_temporary_path(&source, journal_id, "folder")?;
            let temporary_absolute = self
                .root
                .resolve_for_creation(&temporary)
                .map_err(FilesystemSfxError::RootedPath)?;
            finish_case_only_rename(
                &source_absolute,
                &temporary_absolute,
                &destination_absolute,
                EntryKind::Directory,
                replay,
            )?;
        } else {
            finish_rename(
                &source_absolute,
                &destination_absolute,
                EntryKind::Directory,
                replay,
            )?;
        }
        Ok(SfxMutationOutcome::Folder(folder_record(
            &destination,
            &destination_absolute,
        )?))
    }

    fn delete_folder(
        &self,
        path: &SfxPath,
        recursive: bool,
        replay: bool,
    ) -> Result<(), FilesystemSfxError> {
        let absolute = match self.root.resolve_existing_directory(path) {
            Ok(path) => path,
            Err(error) if replay && rooted_path_is_missing(&error) => return Ok(()),
            Err(error) => return Err(FilesystemSfxError::RootedPath(error)),
        };
        if !recursive {
            let mut entries =
                std::fs::read_dir(&absolute).map_err(|source| FilesystemSfxError::Io {
                    operation: "read SFX folder before deletion",
                    source,
                })?;
            if entries
                .next()
                .transpose()
                .map_err(|source| FilesystemSfxError::Io {
                    operation: "inspect SFX folder before deletion",
                    source,
                })?
                .is_some()
            {
                return Err(FilesystemSfxError::DirectoryNotEmpty);
            }
            std::fs::remove_dir(&absolute).map_err(|source| FilesystemSfxError::Io {
                operation: "delete empty SFX folder",
                source,
            })?;
        } else {
            std::fs::remove_dir_all(&absolute).map_err(|source| FilesystemSfxError::Io {
                operation: "delete SFX folder recursively",
                source,
            })?;
        }
        Ok(())
    }

    fn move_file(
        &self,
        journal_id: &RecoveryJournalId,
        source: SfxPath,
        destination: SfxPath,
        replay: bool,
    ) -> Result<SfxMutationOutcome, FilesystemSfxError> {
        if source == destination {
            let absolute = self
                .root
                .resolve_existing_file_for_mutation(&source)
                .map_err(FilesystemSfxError::RootedPath)?;
            return Ok(SfxMutationOutcome::File(file_record(source, &absolute)?));
        }
        let source_absolute = self
            .root
            .resolve_for_creation(&source)
            .map_err(FilesystemSfxError::RootedPath)?;
        match entry_state(&source_absolute, EntryKind::File)? {
            EntryState::Missing if replay => {
                let destination_absolute = self
                    .root
                    .resolve_existing_file_for_mutation(&destination)
                    .map_err(FilesystemSfxError::RootedPath)?;
                return Ok(SfxMutationOutcome::File(file_record(
                    destination,
                    &destination_absolute,
                )?));
            }
            EntryState::Missing => return Err(FilesystemSfxError::NotFound),
            EntryState::Occupied => return Err(FilesystemSfxError::InvalidPath),
            EntryState::Present => {}
        }
        if let Some(parent) = destination.parent() {
            self.root
                .ensure_directory(&parent)
                .map_err(FilesystemSfxError::RootedPath)?;
        }
        let destination_absolute = self
            .root
            .resolve_for_creation(&destination)
            .map_err(FilesystemSfxError::RootedPath)?;
        let source_folded = source.as_str().to_lowercase();
        let destination_folded = destination.as_str().to_lowercase();
        let case_only = cfg!(windows)
            && source_folded == destination_folded
            && source
                .parent()
                .map(|path| path.into_string().to_lowercase())
                == destination
                    .parent()
                    .map(|path| path.into_string().to_lowercase());
        if case_only {
            let temporary = rename_temporary_path(&source, journal_id, "file")?;
            let temporary_absolute = self
                .root
                .resolve_for_creation(&temporary)
                .map_err(FilesystemSfxError::RootedPath)?;
            finish_case_only_rename(
                &source_absolute,
                &temporary_absolute,
                &destination_absolute,
                EntryKind::File,
                replay,
            )?;
        } else {
            finish_rename(
                &source_absolute,
                &destination_absolute,
                EntryKind::File,
                replay,
            )?;
        }
        Ok(SfxMutationOutcome::File(file_record(
            destination,
            &destination_absolute,
        )?))
    }

    fn delete_file(&self, path: &SfxPath, replay: bool) -> Result<(), FilesystemSfxError> {
        let absolute = match self.root.resolve_existing_file_for_mutation(path) {
            Ok(path) => path,
            Err(error) if replay && rooted_path_is_missing(&error) => return Ok(()),
            Err(error) => return Err(FilesystemSfxError::RootedPath(error)),
        };
        std::fs::remove_file(absolute).map_err(|source| FilesystemSfxError::Io {
            operation: "delete SFX file",
            source,
        })
    }

    fn publish_upload(
        &self,
        journal_id: &RecoveryJournalId,
        staged: &SfxPath,
        destination: SfxPath,
        replace_existing: bool,
        replay: bool,
    ) -> Result<SfxMutationOutcome, FilesystemSfxError> {
        let staged_absolute = self
            .root
            .resolve_for_creation(staged)
            .map_err(FilesystemSfxError::RootedPath)?;
        let destination_absolute = self
            .root
            .resolve_for_creation(&destination)
            .map_err(FilesystemSfxError::RootedPath)?;
        let backup = upload_backup_path(&destination, journal_id)?;
        let backup_absolute = self
            .root
            .resolve_for_creation(&backup)
            .map_err(FilesystemSfxError::RootedPath)?;
        let state = ReplacingUploadState {
            staged: mutation_artifact_state(&staged_absolute)?,
            target: upload_target_state(&destination_absolute)?,
            backup: mutation_artifact_state(&backup_absolute)?,
        };
        if replace_existing {
            publish_replacing_upload(
                &staged_absolute,
                &destination_absolute,
                &backup_absolute,
                state,
                replay,
            )?;
        } else {
            if state.backup != ArtifactState::Missing {
                return Err(FilesystemSfxError::RecoveryArtifactConflict);
            }
            publish_new_upload(
                &staged_absolute,
                &destination_absolute,
                state.staged,
                state.target,
                replay,
            )?;
        }
        Ok(SfxMutationOutcome::File(file_record(
            destination,
            &destination_absolute,
        )?))
    }
}

impl SfxEffects for FilesystemSfxEffects {
    fn list_files(&self) -> SfxFuture<'_, Vec<SfxFileRecord>> {
        let root = self.root.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || list_all_files(&root))
                .await
                .map_err(boxed)?
                .map_err(boxed)
        })
    }

    fn list_folders(&self) -> SfxFuture<'_, Vec<SfxFolderRecord>> {
        let root = self.root.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || list_all_folders(&root))
                .await
                .map_err(boxed)?
                .map_err(boxed)
        })
    }

    fn list_directory<'a>(
        &'a self,
        path: Option<&'a SfxPath>,
    ) -> SfxFuture<'a, Vec<SfxFileRecord>> {
        let root = self.root.clone();
        let path = path.cloned();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || list_directory(&root, path.as_ref()))
                .await
                .map_err(boxed)?
                .map_err(boxed)
        })
    }

    fn target_exists<'a>(&'a self, path: &'a SfxPath) -> SfxFuture<'a, bool> {
        let root = self.root.clone();
        let path = path.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || sfx_upload_target_exists(&root, &path))
                .await
                .map_err(boxed)?
                .map_err(boxed)
        })
    }

    fn resolve_upload<'a>(
        &'a self,
        requested: &'a SfxPath,
        policy: SfxUploadConflictPolicy,
    ) -> SfxUploadResolutionFuture<'a> {
        let root = self.root.clone();
        let requested = requested.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                resolve_upload_destination(&root, &requested, policy)
            })
            .await
            .map_err(|source| safe_failure("sfx_upload_resolution_worker_failed", source))?
            .map_err(map_failure)
        })
    }

    fn discard_upload<'a>(&'a self, staged: &'a SfxPath) -> SfxUploadDiscardFuture<'a> {
        let root = self.root.clone();
        let staged = staged.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let absolute = root
                    .resolve_for_creation(&staged)
                    .map_err(FilesystemSfxError::RootedPath)?;
                remove_regular_file_if_present(&absolute, "discard staged SFX upload")
            })
            .await
            .map_err(|source| safe_failure("sfx_upload_discard_worker_failed", source))?
            .map_err(map_safe_failure)
        })
    }

    fn cleanup_orphans(&self) -> SfxUploadDiscardFuture<'_> {
        let root = self.root.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || cleanup_upload_orphans(&root))
                .await
                .map_err(|source| safe_failure("sfx_cleanup_worker_failed", source))?
                .map_err(map_safe_failure)
        })
    }

    fn apply<'a>(
        &'a self,
        journal_id: &'a RecoveryJournalId,
        mutation: SfxMutation,
        replay: bool,
    ) -> SfxMutationFuture<'a> {
        let effects = self.clone();
        let journal_id = journal_id.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                effects.apply_blocking(&journal_id, mutation, replay)
            })
            .await
            .map_err(|source| {
                SfxMutationFailure::new(
                    SfxMutationFailureKind::Io,
                    "sfx_mutation_worker_failed",
                    Box::new(source),
                )
            })?
            .map_err(map_failure)
        })
    }
}

#[must_use]
pub fn is_supported_sfx_path(path: &SfxPath) -> bool {
    Path::new(path.file_name())
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            SFX_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

pub fn sfx_upload_target_exists(root: &SfxRoot, path: &SfxPath) -> Result<bool, RootedPathError> {
    let absolute = match root.resolve_for_creation(path) {
        Ok(absolute) => absolute,
        Err(RootedPathError::SymbolicLinkTarget(_)) => return Ok(true),
        Err(error) => return Err(error),
    };
    match std::fs::symlink_metadata(&absolute) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(RootedPathError::Io {
            operation: "inspect SFX upload target",
            path: absolute,
            source,
        }),
    }
}

fn list_all_files(root: &SfxRoot) -> Result<Vec<SfxFileRecord>, FilesystemSfxError> {
    let mut budget = TraversalBudget::default();
    let mut files = Vec::new();
    walk_files(root.canonical_path(), None, &mut budget, &mut files)?;
    Ok(files)
}

fn walk_files(
    directory: &Path,
    relative: Option<&SfxPath>,
    budget: &mut TraversalBudget,
    files: &mut Vec<SfxFileRecord>,
) -> Result<(), FilesystemSfxError> {
    for entry in sorted_entries(directory, budget)? {
        let file_type = entry
            .file_type()
            .map_err(|source| FilesystemSfxError::SafeIo {
                operation: "inspect SFX entry",
                source,
            })?;
        if file_type.is_symlink() {
            continue;
        }
        let Some(path) = child_path(relative, &entry) else {
            continue;
        };
        if file_type.is_dir() {
            walk_files(&entry.path(), Some(&path), budget, files)?;
        } else if file_type.is_file()
            && is_supported_sfx_path(&path)
            && !is_internal_artifact(&entry)
        {
            files.push(file_record(path, &entry.path())?);
        }
    }
    Ok(())
}

fn list_all_folders(root: &SfxRoot) -> Result<Vec<SfxFolderRecord>, FilesystemSfxError> {
    let mut budget = TraversalBudget::default();
    let mut folders = Vec::new();
    walk_folders(root.canonical_path(), None, &mut budget, &mut folders)?;
    Ok(folders)
}

fn walk_folders(
    directory: &Path,
    relative: Option<&SfxPath>,
    budget: &mut TraversalBudget,
    folders: &mut Vec<SfxFolderRecord>,
) -> Result<u64, FilesystemSfxError> {
    let mut total = 0_u64;
    for entry in sorted_entries(directory, budget)? {
        let file_type = entry
            .file_type()
            .map_err(|source| FilesystemSfxError::SafeIo {
                operation: "inspect SFX folder entry",
                source,
            })?;
        if file_type.is_symlink() {
            continue;
        }
        let Some(path) = child_path(relative, &entry) else {
            continue;
        };
        if file_type.is_dir() {
            let count = walk_folders(&entry.path(), Some(&path), budget, folders)?;
            folders.push(SfxFolderRecord {
                name: path.file_name().to_owned(),
                path,
                file_count: count,
                has_children: has_child_directories(&entry.path())?,
            });
            total = total
                .checked_add(count)
                .ok_or(FilesystemSfxError::CapacityExceeded)?;
        } else if file_type.is_file()
            && is_supported_sfx_path(&path)
            && !is_internal_artifact(&entry)
        {
            total = total
                .checked_add(1)
                .ok_or(FilesystemSfxError::CapacityExceeded)?;
        }
    }
    Ok(total)
}

fn list_directory(
    root: &SfxRoot,
    relative: Option<&SfxPath>,
) -> Result<Vec<SfxFileRecord>, FilesystemSfxError> {
    let directory = match relative {
        Some(path) => match root.resolve_existing_directory(path) {
            Ok(path) => path,
            Err(error) if rooted_path_is_missing(&error) => return Ok(Vec::new()),
            Err(error) => return Err(FilesystemSfxError::RootedPath(error)),
        },
        None => root.canonical_path().to_path_buf(),
    };
    let mut budget = TraversalBudget::default();
    let mut files = Vec::new();
    for entry in sorted_entries(&directory, &mut budget)? {
        let file_type = entry
            .file_type()
            .map_err(|source| FilesystemSfxError::SafeIo {
                operation: "inspect SFX directory entry",
                source,
            })?;
        let Some(path) = child_path(relative, &entry) else {
            continue;
        };
        if !file_type.is_symlink()
            && file_type.is_file()
            && is_supported_sfx_path(&path)
            && !is_internal_artifact(&entry)
        {
            files.push(file_record(path, &entry.path())?);
        }
    }
    Ok(files)
}

fn sorted_entries(
    directory: &Path,
    budget: &mut TraversalBudget,
) -> Result<Vec<std::fs::DirEntry>, FilesystemSfxError> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(directory).map_err(|source| FilesystemSfxError::SafeIo {
        operation: "read SFX directory",
        source,
    })? {
        budget.observe()?;
        entries.push(entry.map_err(|source| FilesystemSfxError::SafeIo {
            operation: "read SFX directory entry",
            source,
        })?);
    }
    entries.sort_by(|left, right| {
        let left = left.file_name().to_string_lossy().into_owned();
        let right = right.file_name().to_string_lossy().into_owned();
        left.to_lowercase()
            .cmp(&right.to_lowercase())
            .then_with(|| left.cmp(&right))
    });
    Ok(entries)
}

fn child_path(relative: Option<&SfxPath>, entry: &std::fs::DirEntry) -> Option<SfxPath> {
    let name = entry.file_name();
    let name = name.to_str()?;
    relative
        .map_or_else(|| SfxPath::parse(name), |parent| parent.join(name))
        .ok()
}

fn file_record(path: SfxPath, absolute: &Path) -> Result<SfxFileRecord, FilesystemSfxError> {
    let metadata = std::fs::metadata(absolute).map_err(|source| FilesystemSfxError::SafeIo {
        operation: "inspect SFX file",
        source,
    })?;
    if !metadata.is_file() {
        return Err(FilesystemSfxError::NotAFile);
    }
    let modified = metadata
        .modified()
        .map_err(|source| FilesystemSfxError::SafeIo {
            operation: "read SFX file modification time",
            source,
        })?;
    Ok(SfxFileRecord {
        name: path.file_name().to_owned(),
        path,
        size_bytes: metadata.len(),
        modified_at_unix_seconds: system_time_seconds(modified),
    })
}

fn folder_record(path: &SfxPath, absolute: &Path) -> Result<SfxFolderRecord, FilesystemSfxError> {
    let mut budget = TraversalBudget::default();
    let count = count_supported_files(absolute, &mut budget)?;
    Ok(SfxFolderRecord {
        name: path.file_name().to_owned(),
        path: path.clone(),
        file_count: count,
        has_children: has_child_directories(absolute)?,
    })
}

fn count_supported_files(
    directory: &Path,
    budget: &mut TraversalBudget,
) -> Result<u64, FilesystemSfxError> {
    let mut total = 0_u64;
    for entry in sorted_entries(directory, budget)? {
        let file_type = entry
            .file_type()
            .map_err(|source| FilesystemSfxError::SafeIo {
                operation: "inspect SFX count entry",
                source,
            })?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            total = total
                .checked_add(count_supported_files(&entry.path(), budget)?)
                .ok_or(FilesystemSfxError::CapacityExceeded)?;
        } else if file_type.is_file() {
            let name = entry.file_name();
            if let Some(name) = name.to_str()
                && SfxPath::parse(name).is_ok()
                && Path::new(name)
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|extension| {
                        SFX_EXTENSIONS
                            .iter()
                            .any(|supported| extension.eq_ignore_ascii_case(supported))
                    })
                && !is_internal_artifact(&entry)
            {
                total = total
                    .checked_add(1)
                    .ok_or(FilesystemSfxError::CapacityExceeded)?;
            }
        }
    }
    Ok(total)
}

fn has_child_directories(directory: &Path) -> Result<bool, FilesystemSfxError> {
    for entry in std::fs::read_dir(directory).map_err(|source| FilesystemSfxError::SafeIo {
        operation: "read SFX folder",
        source,
    })? {
        let entry = entry.map_err(|source| FilesystemSfxError::SafeIo {
            operation: "read SFX child folder",
            source,
        })?;
        let file_type = entry
            .file_type()
            .map_err(|source| FilesystemSfxError::SafeIo {
                operation: "inspect SFX child folder",
                source,
            })?;
        if !file_type.is_symlink() && file_type.is_dir() && child_path(None, &entry).is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn resolve_upload_destination(
    root: &SfxRoot,
    requested: &SfxPath,
    policy: SfxUploadConflictPolicy,
) -> Result<SfxUploadResolution, FilesystemSfxError> {
    if !sfx_upload_target_exists(root, requested).map_err(FilesystemSfxError::RootedPath)? {
        return Ok(SfxUploadResolution::Publish {
            destination: requested.clone(),
            replace_existing: policy == SfxUploadConflictPolicy::Overwrite,
        });
    }
    match policy {
        SfxUploadConflictPolicy::Overwrite => {
            let absolute = root
                .resolve_for_creation(requested)
                .map_err(FilesystemSfxError::RootedPath)?;
            if upload_target_state(&absolute)? != UploadTargetState::RegularFile {
                return Err(FilesystemSfxError::UnexpectedArtifact);
            }
            Ok(SfxUploadResolution::Publish {
                destination: requested.clone(),
                replace_existing: true,
            })
        }
        SfxUploadConflictPolicy::Skip => Ok(SfxUploadResolution::Skip),
        SfxUploadConflictPolicy::Rename => {
            for sequence in 1..=MAX_UPLOAD_RENAME_ATTEMPTS {
                let candidate = renamed_upload_path(requested, sequence)?;
                if !sfx_upload_target_exists(root, &candidate)
                    .map_err(FilesystemSfxError::RootedPath)?
                {
                    return Ok(SfxUploadResolution::Publish {
                        destination: candidate,
                        replace_existing: false,
                    });
                }
            }
            Err(FilesystemSfxError::UploadRenameExhausted)
        }
    }
}

fn renamed_upload_path(requested: &SfxPath, sequence: u32) -> Result<SfxPath, FilesystemSfxError> {
    let file_name = requested.file_name();
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(file_name);
    let suffix = path
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map_or_else(String::new, |extension| format!(".{extension}"));
    sibling_path(requested, &format!("{stem}-{sequence}{suffix}"))
}

fn upload_backup_path(
    destination: &SfxPath,
    journal_id: &RecoveryJournalId,
) -> Result<SfxPath, FilesystemSfxError> {
    sibling_path(
        destination,
        &format!(
            ".{}.{}.sfx-upload-backup",
            destination.file_name(),
            journal_id.as_str()
        ),
    )
}

fn rename_temporary_path(
    source: &SfxPath,
    journal_id: &RecoveryJournalId,
    kind: &'static str,
) -> Result<SfxPath, FilesystemSfxError> {
    sibling_path(
        source,
        &format!(
            ".{}.{}.sfx-{kind}-rename",
            source.file_name(),
            journal_id.as_str()
        ),
    )
}

fn sibling_path(path: &SfxPath, name: &str) -> Result<SfxPath, FilesystemSfxError> {
    path.parent()
        .map_or_else(|| SfxPath::parse(name), |parent| parent.join(name))
        .map_err(|_| FilesystemSfxError::InvalidPath)
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ArtifactState {
    Missing,
    RegularFile,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum UploadTargetState {
    Missing,
    RegularFile,
    Occupied,
}

#[derive(Debug, Clone, Copy)]
struct ReplacingUploadState {
    staged: ArtifactState,
    target: UploadTargetState,
    backup: ArtifactState,
}

fn mutation_artifact_state(path: &Path) -> Result<ArtifactState, FilesystemSfxError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(FilesystemSfxError::UnexpectedArtifact)
        }
        Ok(_) => Ok(ArtifactState::RegularFile),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(ArtifactState::Missing),
        Err(source) => Err(FilesystemSfxError::Io {
            operation: "inspect SFX mutation artifact",
            source,
        }),
    }
}

fn upload_target_state(path: &Path) -> Result<UploadTargetState, FilesystemSfxError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Ok(UploadTargetState::Occupied)
        }
        Ok(_) => Ok(UploadTargetState::RegularFile),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(UploadTargetState::Missing)
        }
        Err(source) => Err(FilesystemSfxError::Io {
            operation: "inspect SFX upload destination",
            source,
        }),
    }
}

fn publish_new_upload(
    staged: &Path,
    destination: &Path,
    staged_state: ArtifactState,
    target_state: UploadTargetState,
    replay: bool,
) -> Result<(), FilesystemSfxError> {
    match (staged_state, target_state) {
        (ArtifactState::RegularFile, UploadTargetState::Missing) => {
            match std::fs::hard_link(staged, destination) {
                Ok(()) => {}
                Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(FilesystemSfxError::DestinationExists);
                }
                Err(source) => {
                    return Err(FilesystemSfxError::SafeIo {
                        operation: "publish new SFX upload",
                        source,
                    });
                }
            }
            remove_regular_file_if_present(staged, "remove published SFX upload stage")
        }
        (ArtifactState::RegularFile, UploadTargetState::RegularFile)
            if replay
                && same_file::is_same_file(staged, destination).map_err(|source| {
                    FilesystemSfxError::Io {
                        operation: "compare SFX upload identities",
                        source,
                    }
                })? =>
        {
            remove_regular_file_if_present(staged, "finish published SFX upload")
        }
        (ArtifactState::Missing, UploadTargetState::RegularFile) if replay => Ok(()),
        (ArtifactState::Missing, UploadTargetState::Missing) => {
            Err(FilesystemSfxError::UploadStageMissing)
        }
        (_, UploadTargetState::Occupied) => Err(FilesystemSfxError::RecoveryArtifactConflict),
        _ => Err(FilesystemSfxError::DestinationExists),
    }
}

fn publish_replacing_upload(
    staged: &Path,
    destination: &Path,
    backup: &Path,
    state: ReplacingUploadState,
    replay: bool,
) -> Result<(), FilesystemSfxError> {
    if state.target == UploadTargetState::Occupied {
        return Err(FilesystemSfxError::RecoveryArtifactConflict);
    }
    match state.staged {
        ArtifactState::RegularFile => {
            match (state.target, state.backup) {
                (UploadTargetState::RegularFile, ArtifactState::Missing) => {
                    std::fs::rename(destination, backup).map_err(|source| {
                        FilesystemSfxError::Io {
                            operation: "stage replaced SFX upload",
                            source,
                        }
                    })?;
                }
                (UploadTargetState::Missing, _) => {}
                (UploadTargetState::RegularFile, ArtifactState::RegularFile)
                | (UploadTargetState::Occupied, _) => {
                    return Err(FilesystemSfxError::RecoveryArtifactConflict);
                }
            }
            std::fs::rename(staged, destination).map_err(|source| FilesystemSfxError::Io {
                operation: "publish replacing SFX upload",
                source,
            })?;
            remove_regular_file_if_present(backup, "remove replaced SFX upload backup")
        }
        ArtifactState::Missing if replay && state.target == UploadTargetState::RegularFile => {
            remove_regular_file_if_present(backup, "finish replacing SFX upload")
        }
        ArtifactState::Missing
            if state.target == UploadTargetState::Missing
                && state.backup == ArtifactState::RegularFile =>
        {
            std::fs::rename(backup, destination).map_err(|source| FilesystemSfxError::Io {
                operation: "restore interrupted SFX upload",
                source,
            })?;
            Err(FilesystemSfxError::UploadStageMissing)
        }
        ArtifactState::Missing => Err(FilesystemSfxError::UploadStageMissing),
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum EntryKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum EntryState {
    Missing,
    Present,
    Occupied,
}

fn entry_state(path: &Path, expected: EntryKind) -> Result<EntryState, FilesystemSfxError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(EntryState::Occupied),
        Ok(metadata)
            if (expected == EntryKind::File && metadata.is_file())
                || (expected == EntryKind::Directory && metadata.is_dir()) =>
        {
            Ok(EntryState::Present)
        }
        Ok(_) => Ok(EntryState::Occupied),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(EntryState::Missing),
        Err(source) => Err(FilesystemSfxError::Io {
            operation: "inspect SFX rename path",
            source,
        }),
    }
}

fn finish_rename(
    source: &Path,
    destination: &Path,
    kind: EntryKind,
    replay: bool,
) -> Result<(), FilesystemSfxError> {
    match (entry_state(source, kind)?, entry_state(destination, kind)?) {
        (EntryState::Present, EntryState::Missing) => {
            std::fs::rename(source, destination).map_err(|source| FilesystemSfxError::Io {
                operation: "rename SFX entry",
                source,
            })
        }
        (EntryState::Missing, EntryState::Present) if replay => Ok(()),
        (EntryState::Missing, _) => Err(FilesystemSfxError::NotFound),
        (_, EntryState::Present | EntryState::Occupied) => {
            Err(FilesystemSfxError::DestinationExists)
        }
        (EntryState::Occupied, EntryState::Missing) => Err(FilesystemSfxError::InvalidPath),
    }
}

fn finish_case_only_rename(
    source: &Path,
    temporary: &Path,
    destination: &Path,
    kind: EntryKind,
    replay: bool,
) -> Result<(), FilesystemSfxError> {
    let source_state = entry_state(source, kind)?;
    let temporary_state = entry_state(temporary, kind)?;
    let destination_state = entry_state(destination, kind)?;
    if replay
        && source_state == EntryState::Missing
        && temporary_state == EntryState::Missing
        && destination_state == EntryState::Present
    {
        return Ok(());
    }
    match (source_state, temporary_state) {
        (EntryState::Present, EntryState::Missing) => {
            std::fs::rename(source, temporary).map_err(|source| FilesystemSfxError::Io {
                operation: "stage case-only SFX rename",
                source,
            })?;
        }
        (EntryState::Missing, EntryState::Present) if replay => {}
        (EntryState::Missing, EntryState::Missing) => return Err(FilesystemSfxError::NotFound),
        _ => return Err(FilesystemSfxError::RecoveryArtifactConflict),
    }
    if entry_state(destination, kind)? == EntryState::Present {
        return Err(FilesystemSfxError::DestinationExists);
    }
    std::fs::rename(temporary, destination).map_err(|source| FilesystemSfxError::Io {
        operation: "finish case-only SFX rename",
        source,
    })
}

fn remove_regular_file_if_present(
    path: &Path,
    operation: &'static str,
) -> Result<(), FilesystemSfxError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(FilesystemSfxError::UnexpectedArtifact)
        }
        Ok(_) => std::fs::remove_file(path)
            .map_err(|source| FilesystemSfxError::Io { operation, source }),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(FilesystemSfxError::Io { operation, source }),
    }
}

fn cleanup_upload_orphans(root: &SfxRoot) -> Result<(), FilesystemSfxError> {
    let mut directories = vec![root.canonical_path().to_path_buf()];
    let mut observed = 0_usize;
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(&directory).map_err(|source| FilesystemSfxError::SafeIo {
            operation: "read SFX directory during cleanup",
            source,
        })? {
            observed = observed
                .checked_add(1)
                .ok_or(FilesystemSfxError::CapacityExceeded)?;
            if observed > MAX_SFX_ENTRIES {
                return Err(FilesystemSfxError::CapacityExceeded);
            }
            let entry = entry.map_err(|source| FilesystemSfxError::SafeIo {
                operation: "read SFX cleanup entry",
                source,
            })?;
            let file_type = entry
                .file_type()
                .map_err(|source| FilesystemSfxError::SafeIo {
                    operation: "inspect SFX cleanup entry",
                    source,
                })?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                directories.push(entry.path());
            } else if file_type.is_file() && is_upload_artifact_name(&entry.file_name()) {
                std::fs::remove_file(entry.path()).map_err(|source| {
                    FilesystemSfxError::SafeIo {
                        operation: "remove orphaned SFX upload artifact",
                        source,
                    }
                })?;
            }
        }
    }
    Ok(())
}

fn is_internal_artifact(entry: &std::fs::DirEntry) -> bool {
    is_upload_artifact_name(&entry.file_name())
}

fn is_upload_artifact_name(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    if let Some(id) = name
        .strip_prefix(".sfx-upload-")
        .and_then(|value| value.strip_suffix(".partial"))
    {
        return Uuid::parse_str(id).is_ok();
    }
    let Some(prefix) = name.strip_suffix(".sfx-upload-backup") else {
        return false;
    };
    prefix
        .rsplit_once('.')
        .is_some_and(|(_, id)| Uuid::parse_str(id).is_ok())
}

fn system_time_seconds(value: SystemTime) -> i64 {
    match value.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
        Err(error) => -i64::try_from(error.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

#[derive(Debug, Default)]
struct TraversalBudget {
    observed: usize,
}

impl TraversalBudget {
    fn observe(&mut self) -> Result<(), FilesystemSfxError> {
        self.observed = self
            .observed
            .checked_add(1)
            .ok_or(FilesystemSfxError::CapacityExceeded)?;
        if self.observed > MAX_SFX_ENTRIES {
            Err(FilesystemSfxError::CapacityExceeded)
        } else {
            Ok(())
        }
    }
}

fn rooted_path_is_missing(error: &RootedPathError) -> bool {
    matches!(
        error,
        RootedPathError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound
    )
}

fn boxed(error: impl Error + Send + Sync + 'static) -> Box<dyn Error + Send + Sync> {
    Box::new(error)
}

fn safe_failure(
    code: &'static str,
    source: impl Error + Send + Sync + 'static,
) -> SfxMutationFailure {
    SfxMutationFailure::without_recovery(SfxMutationFailureKind::Io, code, Box::new(source))
}

fn map_safe_failure(error: FilesystemSfxError) -> SfxMutationFailure {
    let (kind, code, _) = classify_failure(&error);
    SfxMutationFailure::without_recovery(kind, code, Box::new(error))
}

fn map_failure(error: FilesystemSfxError) -> SfxMutationFailure {
    let (kind, code, recovery_required) = classify_failure(&error);
    if recovery_required {
        SfxMutationFailure::new(kind, code, Box::new(error))
    } else {
        SfxMutationFailure::without_recovery(kind, code, Box::new(error))
    }
}

fn classify_failure(error: &FilesystemSfxError) -> (SfxMutationFailureKind, &'static str, bool) {
    match error {
        FilesystemSfxError::RootedPath(error) if rooted_path_is_missing(error) => (
            SfxMutationFailureKind::NotFound,
            "sfx_path_not_found",
            false,
        ),
        FilesystemSfxError::NotFound | FilesystemSfxError::UploadStageMissing => (
            SfxMutationFailureKind::NotFound,
            "sfx_path_not_found",
            false,
        ),
        FilesystemSfxError::RootedPath(RootedPathError::Io { .. }) => {
            (SfxMutationFailureKind::Io, "sfx_file_io_failed", true)
        }
        FilesystemSfxError::RootedPath(_)
        | FilesystemSfxError::NotAFile
        | FilesystemSfxError::InvalidMove
        | FilesystemSfxError::InvalidPath => {
            (SfxMutationFailureKind::Invalid, "sfx_path_invalid", false)
        }
        FilesystemSfxError::DestinationExists | FilesystemSfxError::UnexpectedArtifact => (
            SfxMutationFailureKind::Conflict,
            "sfx_destination_exists",
            false,
        ),
        FilesystemSfxError::DirectoryNotEmpty => (
            SfxMutationFailureKind::NotEmpty,
            "sfx_folder_not_empty",
            false,
        ),
        FilesystemSfxError::CapacityExceeded => (
            SfxMutationFailureKind::Capacity,
            "sfx_inventory_capacity_exceeded",
            false,
        ),
        FilesystemSfxError::UploadRenameExhausted => (
            SfxMutationFailureKind::Conflict,
            "sfx_upload_rename_exhausted",
            false,
        ),
        FilesystemSfxError::RecoveryArtifactConflict => (
            SfxMutationFailureKind::Conflict,
            "sfx_recovery_artifact_conflict",
            true,
        ),
        FilesystemSfxError::Io { .. } => (SfxMutationFailureKind::Io, "sfx_file_io_failed", true),
        FilesystemSfxError::SafeIo { .. } => {
            (SfxMutationFailureKind::Io, "sfx_file_io_failed", true)
        }
    }
}

#[derive(Debug)]
enum FilesystemSfxError {
    RootedPath(RootedPathError),
    Io {
        operation: &'static str,
        source: std::io::Error,
    },
    SafeIo {
        operation: &'static str,
        source: std::io::Error,
    },
    NotFound,
    NotAFile,
    DestinationExists,
    DirectoryNotEmpty,
    InvalidMove,
    InvalidPath,
    UnexpectedArtifact,
    RecoveryArtifactConflict,
    UploadStageMissing,
    UploadRenameExhausted,
    CapacityExceeded,
}

impl Display for FilesystemSfxError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootedPath(source) => Display::fmt(source, formatter),
            Self::Io { operation, .. } | Self::SafeIo { operation, .. } => {
                write!(formatter, "failed to {operation}")
            }
            Self::NotFound => formatter.write_str("SFX path was not found"),
            Self::NotAFile => formatter.write_str("SFX path is not a file"),
            Self::DestinationExists => formatter.write_str("SFX destination already exists"),
            Self::DirectoryNotEmpty => formatter.write_str("SFX folder is not empty"),
            Self::InvalidMove => formatter.write_str("SFX destination is invalid"),
            Self::InvalidPath => formatter.write_str("SFX path is invalid"),
            Self::UnexpectedArtifact => formatter.write_str("SFX mutation artifact is invalid"),
            Self::RecoveryArtifactConflict => {
                formatter.write_str("SFX recovery artifact conflicts with the destination")
            }
            Self::UploadStageMissing => formatter.write_str("staged SFX upload is missing"),
            Self::UploadRenameExhausted => {
                formatter.write_str("no available SFX upload filename was found")
            }
            Self::CapacityExceeded => formatter.write_str("SFX inventory is too large"),
        }
    }
}

impl Error for FilesystemSfxError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::RootedPath(source) => Some(source),
            Self::Io { source, .. } | Self::SafeIo { source, .. } => Some(source),
            Self::NotFound
            | Self::NotAFile
            | Self::DestinationExists
            | Self::DirectoryNotEmpty
            | Self::InvalidMove
            | Self::InvalidPath
            | Self::UnexpectedArtifact
            | Self::RecoveryArtifactConflict
            | Self::UploadStageMissing
            | Self::UploadRenameExhausted
            | Self::CapacityExceeded => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use music_application::sfx::{SfxEffects, SfxMutation};
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn recovery_artifact_conflicts_require_operator_recovery() {
        let failure = map_failure(FilesystemSfxError::RecoveryArtifactConflict);
        assert_eq!(failure.kind(), SfxMutationFailureKind::Conflict);
        assert_eq!(failure.code(), "sfx_recovery_artifact_conflict");
        assert!(failure.requires_recovery());
    }

    #[tokio::test]
    async fn inventory_mutations_and_upload_replay_are_coherent()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let root = directory.path().join("sfx");
        std::fs::create_dir_all(root.join("dnd/doors"))?;
        std::fs::write(root.join("dnd/doors/door.ogg"), b"door")?;
        std::fs::write(root.join("dnd/readme.txt"), b"not audio")?;
        let effects = FilesystemSfxEffects::new(SfxRoot::open(&root)?);

        let files = effects.list_files().await?;
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path.as_str(), "dnd/doors/door.ogg");
        let folders = effects.list_folders().await?;
        let by_path = folders
            .iter()
            .map(|folder| (folder.path.as_str(), folder))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(by_path["dnd"].file_count, 1);
        assert!(by_path["dnd"].has_children);
        assert_eq!(by_path["dnd/doors"].file_count, 1);

        let missing_move_error = match effects
            .apply(
                &RecoveryJournalId::new(),
                SfxMutation::MoveFile {
                    source: SfxPath::parse("missing.wav")?,
                    destination: SfxPath::parse("created/path.wav")?,
                },
                false,
            )
            .await
        {
            Err(error) => error,
            Ok(_) => {
                return Err(
                    std::io::Error::other("a missing source should reject the move").into(),
                );
            }
        };
        assert_eq!(missing_move_error.kind(), SfxMutationFailureKind::NotFound);
        assert!(!root.join("created").exists());

        let moved = SfxMutation::MoveFile {
            source: SfxPath::parse("dnd/doors/door.ogg")?,
            destination: SfxPath::parse("other/renamed.ogg")?,
        };
        let move_id = RecoveryJournalId::new();
        effects.apply(&move_id, moved.clone(), false).await?;
        effects.apply(&move_id, moved, true).await?;
        assert!(root.join("other/renamed.ogg").is_file());

        let requested = SfxPath::parse("uploads/clip.wav")?;
        std::fs::create_dir_all(root.join("uploads"))?;
        let staged = SfxPath::parse(format!(
            "uploads/.sfx-upload-{}.partial",
            Uuid::new_v4().simple()
        ))?;
        std::fs::write(root.join(staged.as_str()), b"first")?;
        assert_eq!(
            effects
                .resolve_upload(&requested, SfxUploadConflictPolicy::Rename)
                .await?,
            SfxUploadResolution::Publish {
                destination: requested.clone(),
                replace_existing: false,
            }
        );
        let upload_id = RecoveryJournalId::new();
        let publish = SfxMutation::PublishUpload {
            staged: staged.clone(),
            destination: requested.clone(),
            replace_existing: false,
        };
        effects.apply(&upload_id, publish.clone(), false).await?;
        effects.apply(&upload_id, publish, true).await?;
        assert!(!root.join(staged.as_str()).exists());
        assert_eq!(std::fs::read(root.join(requested.as_str()))?, b"first");

        let overwrite_stage = SfxPath::parse(format!(
            "uploads/.sfx-upload-{}.partial",
            Uuid::new_v4().simple()
        ))?;
        std::fs::write(root.join(overwrite_stage.as_str()), b"second")?;
        effects
            .apply(
                &RecoveryJournalId::new(),
                SfxMutation::PublishUpload {
                    staged: overwrite_stage,
                    destination: requested.clone(),
                    replace_existing: true,
                },
                false,
            )
            .await?;
        assert_eq!(std::fs::read(root.join(requested.as_str()))?, b"second");

        effects
            .apply(
                &RecoveryJournalId::new(),
                SfxMutation::DeleteFolder {
                    path: SfxPath::parse("other")?,
                    recursive: true,
                },
                false,
            )
            .await?;
        assert!(!root.join("other").exists());
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn inventory_skips_symlinks_and_mutations_refuse_them()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        use std::os::unix::fs::symlink;

        let directory = tempdir()?;
        let root = directory.path().join("sfx");
        let outside = directory.path().join("outside");
        std::fs::create_dir_all(&root)?;
        std::fs::create_dir_all(&outside)?;
        std::fs::write(outside.join("secret.wav"), b"secret")?;
        symlink(&outside, root.join("linked"))?;
        let effects = FilesystemSfxEffects::new(SfxRoot::open(&root)?);

        assert!(effects.list_files().await?.is_empty());
        let error = effects
            .apply(
                &RecoveryJournalId::new(),
                SfxMutation::DeleteFile {
                    path: SfxPath::parse("linked/secret.wav")?,
                },
                false,
            )
            .await
            .expect_err("symlink mutation should be rejected");
        assert_eq!(error.kind(), SfxMutationFailureKind::Invalid);
        assert!(outside.join("secret.wav").is_file());
        Ok(())
    }
}
