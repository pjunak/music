use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::path::Path;

use music_application::library::{
    LibraryFileMutation, LibraryFileMutationOutcome, LibraryMutationEffects,
    LibraryMutationFailure, LibraryMutationFailureKind, LibraryMutationFuture,
    LibraryUploadDiscardFuture, LibraryUploadResolution, LibraryUploadResolutionFuture,
    TrackMetadataField, TrackMetadataPatch, TrackMetadataPatchValue, UploadConflictPolicy,
};
use music_application::recovery::RecoveryJournalId;

use crate::{
    FilesystemDiscoveryError, LibraryRoot, MetadataAdapter, MetadataError, RootedPathError,
    TagField, TagPatch, inspect_library_track, is_supported_library_path,
};

const MAX_UPLOAD_RENAME_ATTEMPTS: u32 = 10_000;

#[derive(Debug, Clone)]
pub struct FilesystemLibraryMutations {
    root: LibraryRoot,
    metadata: MetadataAdapter,
}

impl FilesystemLibraryMutations {
    #[must_use]
    pub const fn new(root: LibraryRoot, metadata: MetadataAdapter) -> Self {
        Self { root, metadata }
    }

    fn apply_blocking(
        &self,
        journal_id: &RecoveryJournalId,
        mutation: LibraryFileMutation,
        replay: bool,
    ) -> Result<LibraryFileMutationOutcome, FilesystemMutationError> {
        match mutation {
            LibraryFileMutation::CreateFolder { path } => {
                let absolute = self
                    .root
                    .ensure_directory(&path)
                    .map_err(FilesystemMutationError::RootedPath)?;
                Ok(LibraryFileMutationOutcome::Folder {
                    path,
                    has_children: has_child_directories(&absolute)?,
                })
            }
            LibraryFileMutation::RenameFolder {
                source,
                destination,
            } => {
                let source_folded = source.as_str().to_lowercase();
                let destination_folded = destination.as_str().to_lowercase();
                if source == destination
                    || destination
                        .as_str()
                        .starts_with(&format!("{}/", source.as_str()))
                    || (cfg!(windows)
                        && destination_folded.starts_with(&format!("{source_folded}/")))
                {
                    return Err(FilesystemMutationError::InvalidMove);
                }
                let case_only = source.as_str() != destination.as_str()
                    && source_folded == destination_folded
                    && source
                        .parent()
                        .map(|path| path.into_string().to_lowercase())
                        == destination
                            .parent()
                            .map(|path| path.into_string().to_lowercase());
                let source_absolute = match self.root.resolve_existing_directory(&source) {
                    Ok(path) => path,
                    Err(error) if replay && rooted_path_is_missing(&error) => {
                        let destination_absolute = self
                            .root
                            .resolve_existing_directory(&destination)
                            .map_err(FilesystemMutationError::RootedPath)?;
                        ensure_directory(&destination_absolute)?;
                        return Ok(LibraryFileMutationOutcome::Folder {
                            path: destination,
                            has_children: has_child_directories(&destination_absolute)?,
                        });
                    }
                    Err(error) => return Err(FilesystemMutationError::RootedPath(error)),
                };
                ensure_directory(&source_absolute)?;
                if replay
                    && case_only
                    && source_absolute.file_name().and_then(|name| name.to_str())
                        == Some(destination.file_name())
                {
                    return Ok(LibraryFileMutationOutcome::Folder {
                        path: destination,
                        has_children: has_child_directories(&source_absolute)?,
                    });
                }
                if let Some(parent) = destination.parent() {
                    self.root
                        .ensure_directory(&parent)
                        .map_err(FilesystemMutationError::RootedPath)?;
                }
                let destination_absolute = self
                    .root
                    .resolve_for_creation(&destination)
                    .map_err(FilesystemMutationError::RootedPath)?;
                match std::fs::symlink_metadata(&destination_absolute) {
                    Ok(_) if !case_only => {
                        return Err(FilesystemMutationError::DestinationExists);
                    }
                    Ok(_) => {}
                    Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
                    Err(source) => {
                        return Err(FilesystemMutationError::Io {
                            operation: "inspect folder rename destination",
                            source,
                        });
                    }
                }
                std::fs::rename(&source_absolute, &destination_absolute).map_err(|source| {
                    FilesystemMutationError::Io {
                        operation: "rename library folder",
                        source,
                    }
                })?;
                Ok(LibraryFileMutationOutcome::Folder {
                    path: destination,
                    has_children: has_child_directories(&destination_absolute)?,
                })
            }
            LibraryFileMutation::DeleteFolder { path, recursive } => {
                let absolute = match self.root.resolve_existing_directory(&path) {
                    Ok(path) => path,
                    Err(error) if replay && rooted_path_is_missing(&error) => {
                        return Ok(LibraryFileMutationOutcome::Deleted);
                    }
                    Err(error) => return Err(FilesystemMutationError::RootedPath(error)),
                };
                ensure_directory(&absolute)?;
                let result = if recursive {
                    std::fs::remove_dir_all(&absolute)
                } else {
                    std::fs::remove_dir(&absolute)
                };
                result.map_err(|source| {
                    if source.kind() == std::io::ErrorKind::DirectoryNotEmpty {
                        FilesystemMutationError::DirectoryNotEmpty
                    } else {
                        FilesystemMutationError::Io {
                            operation: "delete library folder",
                            source,
                        }
                    }
                })?;
                Ok(LibraryFileMutationOutcome::Deleted)
            }
            LibraryFileMutation::MoveTrack {
                track_id,
                source,
                destination,
            } => {
                if source == destination {
                    let track = inspect_library_track(&self.root, &self.metadata, &destination)
                        .map_err(FilesystemMutationError::Discovery)?;
                    return Ok(LibraryFileMutationOutcome::TrackMoved { track_id, track });
                }
                let source_folded = source.as_str().to_lowercase();
                let destination_folded = destination.as_str().to_lowercase();
                let case_only = source_folded == destination_folded
                    && source
                        .parent()
                        .map(|path| path.into_string().to_lowercase())
                        == destination
                            .parent()
                            .map(|path| path.into_string().to_lowercase());
                let source_absolute = match self.root.resolve_existing_file_for_mutation(&source) {
                    Ok(path) => path,
                    Err(error) if replay && rooted_path_is_missing(&error) => {
                        self.root
                            .resolve_existing_file_for_mutation(&destination)
                            .map_err(FilesystemMutationError::RootedPath)?;
                        let track = inspect_library_track(&self.root, &self.metadata, &destination)
                            .map_err(FilesystemMutationError::Discovery)?;
                        return Ok(LibraryFileMutationOutcome::TrackMoved { track_id, track });
                    }
                    Err(error) => return Err(FilesystemMutationError::RootedPath(error)),
                };
                if replay
                    && case_only
                    && source_absolute.file_name().and_then(|name| name.to_str())
                        == Some(destination.file_name())
                {
                    let track = inspect_library_track(&self.root, &self.metadata, &destination)
                        .map_err(FilesystemMutationError::Discovery)?;
                    return Ok(LibraryFileMutationOutcome::TrackMoved { track_id, track });
                }
                if let Some(parent) = destination.parent() {
                    self.root
                        .ensure_directory(&parent)
                        .map_err(FilesystemMutationError::RootedPath)?;
                }
                let destination_absolute = self
                    .root
                    .resolve_for_creation(&destination)
                    .map_err(FilesystemMutationError::RootedPath)?;
                match std::fs::symlink_metadata(&destination_absolute) {
                    Ok(_) if !case_only => {
                        return Err(FilesystemMutationError::DestinationExists);
                    }
                    Ok(_) => {}
                    Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
                    Err(source) => {
                        return Err(FilesystemMutationError::Io {
                            operation: "inspect track move destination",
                            source,
                        });
                    }
                }
                std::fs::rename(&source_absolute, &destination_absolute).map_err(|source| {
                    FilesystemMutationError::Io {
                        operation: "move library track",
                        source,
                    }
                })?;
                let track = inspect_library_track(&self.root, &self.metadata, &destination)
                    .map_err(FilesystemMutationError::Discovery)?;
                Ok(LibraryFileMutationOutcome::TrackMoved { track_id, track })
            }
            LibraryFileMutation::DeleteTrack { track_id, path } => {
                let absolute = match self.root.resolve_existing_file_for_mutation(&path) {
                    Ok(path) => path,
                    Err(error) if rooted_path_is_missing(&error) => {
                        return Ok(LibraryFileMutationOutcome::TrackDeleted { track_id });
                    }
                    Err(error) => return Err(FilesystemMutationError::RootedPath(error)),
                };
                std::fs::remove_file(&absolute).map_err(|source| FilesystemMutationError::Io {
                    operation: "delete library track",
                    source,
                })?;
                Ok(LibraryFileMutationOutcome::TrackDeleted { track_id })
            }
            LibraryFileMutation::UpdateTrackMetadata {
                track_id,
                path,
                patch,
            } => self.apply_metadata_update(journal_id, track_id, path, &patch, replay),
            LibraryFileMutation::PublishUpload {
                staged,
                destination,
                replace_existing,
            } => self.apply_upload_publish(
                journal_id,
                &staged,
                destination,
                replace_existing,
                replay,
            ),
        }
    }

    fn apply_metadata_update(
        &self,
        journal_id: &RecoveryJournalId,
        track_id: music_domain::TrackId,
        path: music_domain::LibraryPath,
        patch: &TrackMetadataPatch,
        replay: bool,
    ) -> Result<LibraryFileMutationOutcome, FilesystemMutationError> {
        if !patch.has_tag_changes() {
            return Ok(LibraryFileMutationOutcome::TrackMetadataUpdated {
                track_id,
                discovered: None,
            });
        }
        let stage_path = metadata_sibling_path(&path, journal_id, "stage")?;
        let backup_path = metadata_sibling_path(&path, journal_id, "backup")?;
        let stage_absolute = self
            .root
            .resolve_for_creation(&stage_path)
            .map_err(FilesystemMutationError::RootedPath)?;
        let backup_absolute = self
            .root
            .resolve_for_creation(&backup_path)
            .map_err(FilesystemMutationError::RootedPath)?;
        let stage_state = mutation_artifact_state(&stage_absolute)?;
        let mut backup_state = mutation_artifact_state(&backup_absolute)?;

        let source_absolute = match self.root.resolve_existing_file_for_mutation(&path) {
            Ok(source) => source,
            Err(error) if rooted_path_is_missing(&error) => {
                if backup_state == MutationArtifactState::RegularFile
                    && stage_state == MutationArtifactState::RegularFile
                {
                    let target = self
                        .root
                        .resolve_for_creation(&path)
                        .map_err(FilesystemMutationError::RootedPath)?;
                    std::fs::rename(&stage_absolute, target).map_err(|source| {
                        FilesystemMutationError::Io {
                            operation: "publish recovered metadata update",
                            source,
                        }
                    })?;
                    remove_regular_file_if_present(
                        &backup_absolute,
                        "remove recovered metadata backup",
                    )?;
                    let discovered = inspect_library_track(&self.root, &self.metadata, &path)
                        .map_err(FilesystemMutationError::Discovery)?;
                    return Ok(LibraryFileMutationOutcome::TrackMetadataUpdated {
                        track_id,
                        discovered: Some(discovered),
                    });
                }
                if backup_state == MutationArtifactState::RegularFile {
                    let target = self
                        .root
                        .resolve_for_creation(&path)
                        .map_err(FilesystemMutationError::RootedPath)?;
                    std::fs::rename(&backup_absolute, target).map_err(|source| {
                        FilesystemMutationError::Io {
                            operation: "restore interrupted metadata source",
                            source,
                        }
                    })?;
                    backup_state = MutationArtifactState::Missing;
                    self.root
                        .resolve_existing_file_for_mutation(&path)
                        .map_err(FilesystemMutationError::RootedPath)?
                } else {
                    return Err(FilesystemMutationError::RootedPath(error));
                }
            }
            Err(error) => return Err(FilesystemMutationError::RootedPath(error)),
        };

        if replay
            && backup_state == MutationArtifactState::RegularFile
            && stage_state == MutationArtifactState::Missing
        {
            remove_regular_file_if_present(&backup_absolute, "remove recovered metadata backup")?;
            let discovered = inspect_library_track(&self.root, &self.metadata, &path)
                .map_err(FilesystemMutationError::Discovery)?;
            return Ok(LibraryFileMutationOutcome::TrackMetadataUpdated {
                track_id,
                discovered: Some(discovered),
            });
        }
        remove_regular_file_if_present(&stage_absolute, "remove stale metadata stage")?;
        if backup_state != MutationArtifactState::Missing {
            return Err(FilesystemMutationError::UnexpectedMutationArtifact);
        }

        let tag_patch = media_tag_patch(patch)?;
        let staged = self
            .metadata
            .stage_update(&source_absolute, &stage_absolute, &tag_patch)
            .map_err(FilesystemMutationError::Metadata)?;
        let staged_path = staged
            .persist()
            .map_err(FilesystemMutationError::Metadata)?;
        std::fs::rename(&source_absolute, &backup_absolute).map_err(|source| {
            FilesystemMutationError::Io {
                operation: "stage metadata source backup",
                source,
            }
        })?;
        if let Err(source) = std::fs::rename(&staged_path, &source_absolute) {
            let _ = std::fs::rename(&backup_absolute, &source_absolute);
            return Err(FilesystemMutationError::Io {
                operation: "publish metadata update",
                source,
            });
        }
        remove_regular_file_if_present(&backup_absolute, "remove metadata source backup")?;
        let discovered = inspect_library_track(&self.root, &self.metadata, &path)
            .map_err(FilesystemMutationError::Discovery)?;
        Ok(LibraryFileMutationOutcome::TrackMetadataUpdated {
            track_id,
            discovered: Some(discovered),
        })
    }

    fn apply_upload_publish(
        &self,
        journal_id: &RecoveryJournalId,
        staged: &music_domain::LibraryPath,
        destination: music_domain::LibraryPath,
        replace_existing: bool,
        replay: bool,
    ) -> Result<LibraryFileMutationOutcome, FilesystemMutationError> {
        let staged_absolute = self
            .root
            .resolve_for_creation(staged)
            .map_err(FilesystemMutationError::RootedPath)?;
        let destination_absolute = self
            .root
            .resolve_for_creation(&destination)
            .map_err(FilesystemMutationError::RootedPath)?;
        let backup_path = upload_backup_path(&destination, journal_id)?;
        let backup_absolute = self
            .root
            .resolve_for_creation(&backup_path)
            .map_err(FilesystemMutationError::RootedPath)?;
        let stage_state = mutation_artifact_state(&staged_absolute)?;
        let target_state = upload_target_state(&destination_absolute)?;
        let backup_state = mutation_artifact_state(&backup_absolute)?;

        if replace_existing {
            publish_replacing_upload(
                &staged_absolute,
                &destination_absolute,
                &backup_absolute,
                ReplacingUploadState {
                    staged: stage_state,
                    target: target_state,
                    backup: backup_state,
                },
                replay,
            )?;
        } else {
            if backup_state != MutationArtifactState::Missing {
                return Err(FilesystemMutationError::RecoveryArtifactConflict);
            }
            publish_new_upload(
                &staged_absolute,
                &destination_absolute,
                stage_state,
                target_state,
                replay,
            )?;
        }

        let discovered = if is_supported_library_path(&destination) {
            Some(
                inspect_library_track(&self.root, &self.metadata, &destination)
                    .map_err(FilesystemMutationError::Discovery)?,
            )
        } else {
            None
        };
        Ok(LibraryFileMutationOutcome::UploadPublished {
            destination,
            discovered,
        })
    }
}

pub fn library_upload_target_exists(
    root: &LibraryRoot,
    path: &music_domain::LibraryPath,
) -> Result<bool, RootedPathError> {
    let absolute = match root.resolve_for_creation(path) {
        Ok(absolute) => absolute,
        Err(RootedPathError::SymbolicLinkTarget(_)) => return Ok(true),
        Err(error) => return Err(error),
    };
    match std::fs::symlink_metadata(&absolute) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(RootedPathError::Io {
            operation: "inspect library upload target",
            path: absolute,
            source,
        }),
    }
}

fn resolve_upload_destination(
    root: &LibraryRoot,
    requested: &music_domain::LibraryPath,
    policy: UploadConflictPolicy,
) -> Result<LibraryUploadResolution, FilesystemMutationError> {
    if !library_upload_target_exists(root, requested)
        .map_err(FilesystemMutationError::RootedPath)?
    {
        return Ok(LibraryUploadResolution::Publish {
            destination: requested.clone(),
            replace_existing: policy == UploadConflictPolicy::Overwrite,
        });
    }
    match policy {
        UploadConflictPolicy::Overwrite => {
            let absolute = root
                .resolve_for_creation(requested)
                .map_err(|error| match error {
                    RootedPathError::SymbolicLinkTarget(_) => {
                        FilesystemMutationError::UnexpectedMutationArtifact
                    }
                    other => FilesystemMutationError::RootedPath(other),
                })?;
            if upload_target_state(&absolute)? != UploadTargetState::RegularFile {
                return Err(FilesystemMutationError::UnexpectedMutationArtifact);
            }
            Ok(LibraryUploadResolution::Publish {
                destination: requested.clone(),
                replace_existing: true,
            })
        }
        UploadConflictPolicy::Skip => Ok(LibraryUploadResolution::Skip),
        UploadConflictPolicy::Rename => {
            for sequence in 1..=MAX_UPLOAD_RENAME_ATTEMPTS {
                let candidate = renamed_upload_path(requested, sequence)?;
                if !library_upload_target_exists(root, &candidate)
                    .map_err(FilesystemMutationError::RootedPath)?
                {
                    return Ok(LibraryUploadResolution::Publish {
                        destination: candidate,
                        replace_existing: false,
                    });
                }
            }
            Err(FilesystemMutationError::UploadRenameExhausted)
        }
    }
}

fn renamed_upload_path(
    requested: &music_domain::LibraryPath,
    sequence: u32,
) -> Result<music_domain::LibraryPath, FilesystemMutationError> {
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
    let candidate = format!("{stem}-{sequence}{suffix}");
    requested
        .parent()
        .map_or_else(
            || music_domain::LibraryPath::parse(&candidate),
            |parent| parent.join(&candidate),
        )
        .map_err(|_| FilesystemMutationError::InvalidMutationPath)
}

fn upload_backup_path(
    destination: &music_domain::LibraryPath,
    journal_id: &RecoveryJournalId,
) -> Result<music_domain::LibraryPath, FilesystemMutationError> {
    let name = format!(
        ".{}.{}.upload-backup",
        destination.file_name(),
        journal_id.as_str()
    );
    destination
        .parent()
        .map_or_else(
            || music_domain::LibraryPath::parse(&name),
            |parent| parent.join(&name),
        )
        .map_err(|_| FilesystemMutationError::InvalidMutationPath)
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum UploadTargetState {
    Missing,
    RegularFile,
    Occupied,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct ReplacingUploadState {
    staged: MutationArtifactState,
    target: UploadTargetState,
    backup: MutationArtifactState,
}

fn upload_target_state(path: &Path) -> Result<UploadTargetState, FilesystemMutationError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Ok(UploadTargetState::Occupied)
        }
        Ok(_) => Ok(UploadTargetState::RegularFile),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(UploadTargetState::Missing)
        }
        Err(source) => Err(FilesystemMutationError::Io {
            operation: "inspect library upload destination",
            source,
        }),
    }
}

fn publish_new_upload(
    staged: &Path,
    destination: &Path,
    stage_state: MutationArtifactState,
    target_state: UploadTargetState,
    replay: bool,
) -> Result<(), FilesystemMutationError> {
    match (stage_state, target_state) {
        (MutationArtifactState::RegularFile, UploadTargetState::Missing) => {
            match std::fs::hard_link(staged, destination) {
                Ok(()) => {}
                Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(FilesystemMutationError::DestinationExists);
                }
                Err(source) => {
                    return Err(FilesystemMutationError::SafeIo {
                        operation: "publish new library upload",
                        source,
                    });
                }
            }
            remove_regular_file_if_present(staged, "remove published library upload stage")
        }
        (MutationArtifactState::RegularFile, UploadTargetState::RegularFile)
            if replay && files_share_identity(staged, destination)? =>
        {
            remove_regular_file_if_present(staged, "finish published library upload")
        }
        (MutationArtifactState::Missing, UploadTargetState::RegularFile) if replay => Ok(()),
        (MutationArtifactState::Missing, UploadTargetState::Missing) => {
            Err(FilesystemMutationError::UploadStageMissing)
        }
        (_, UploadTargetState::Occupied) => Err(FilesystemMutationError::RecoveryArtifactConflict),
        _ => Err(FilesystemMutationError::DestinationExists),
    }
}

fn publish_replacing_upload(
    staged: &Path,
    destination: &Path,
    backup: &Path,
    state: ReplacingUploadState,
    replay: bool,
) -> Result<(), FilesystemMutationError> {
    if state.target == UploadTargetState::Occupied {
        return Err(FilesystemMutationError::RecoveryArtifactConflict);
    }
    match state.staged {
        MutationArtifactState::RegularFile => {
            match (state.target, state.backup) {
                (UploadTargetState::RegularFile, MutationArtifactState::Missing) => {
                    std::fs::rename(destination, backup).map_err(|source| {
                        FilesystemMutationError::Io {
                            operation: "stage replaced library upload",
                            source,
                        }
                    })?;
                }
                (UploadTargetState::Missing, _) => {}
                (UploadTargetState::RegularFile, MutationArtifactState::RegularFile) => {
                    return Err(FilesystemMutationError::RecoveryArtifactConflict);
                }
                (UploadTargetState::Occupied, _) => {
                    return Err(FilesystemMutationError::RecoveryArtifactConflict);
                }
            }
            std::fs::rename(staged, destination).map_err(|source| FilesystemMutationError::Io {
                operation: "publish replacing library upload",
                source,
            })?;
            remove_regular_file_if_present(backup, "remove replaced library upload backup")
        }
        MutationArtifactState::Missing
            if replay && state.target == UploadTargetState::RegularFile =>
        {
            remove_regular_file_if_present(backup, "finish replacing library upload")
        }
        MutationArtifactState::Missing
            if state.target == UploadTargetState::Missing
                && state.backup == MutationArtifactState::RegularFile =>
        {
            std::fs::rename(backup, destination).map_err(|source| FilesystemMutationError::Io {
                operation: "restore interrupted library upload",
                source,
            })?;
            Err(FilesystemMutationError::UploadStageMissing)
        }
        MutationArtifactState::Missing => Err(FilesystemMutationError::UploadStageMissing),
    }
}

fn files_share_identity(left: &Path, right: &Path) -> Result<bool, FilesystemMutationError> {
    same_file::is_same_file(left, right).map_err(|source| FilesystemMutationError::Io {
        operation: "compare staged and published upload identities",
        source,
    })
}

fn metadata_sibling_path(
    path: &music_domain::LibraryPath,
    journal_id: &RecoveryJournalId,
    role: &'static str,
) -> Result<music_domain::LibraryPath, FilesystemMutationError> {
    let name = format!(
        ".{}.{}.metadata-{role}",
        path.file_name(),
        journal_id.as_str()
    );
    path.parent()
        .map_or_else(
            || music_domain::LibraryPath::parse(&name),
            |parent| parent.join(&name),
        )
        .map_err(|_| FilesystemMutationError::InvalidMutationPath)
}

fn media_tag_patch(patch: &TrackMetadataPatch) -> Result<TagPatch, FilesystemMutationError> {
    let mut tags = TagPatch::new();
    for (field, value) in patch.changes() {
        let Some(field) = media_tag_field(field) else {
            continue;
        };
        match value {
            TrackMetadataPatchValue::Text(value) => tags
                .insert_text(field, value.clone())
                .map_err(FilesystemMutationError::Metadata)?,
            TrackMetadataPatchValue::Number(value) => tags
                .insert_number(field, *value)
                .map_err(FilesystemMutationError::Metadata)?,
            TrackMetadataPatchValue::Cleared => tags
                .clear(field)
                .map_err(FilesystemMutationError::Metadata)?,
        }
    }
    Ok(tags)
}

const fn media_tag_field(field: TrackMetadataField) -> Option<TagField> {
    match field {
        TrackMetadataField::Title => Some(TagField::Title),
        TrackMetadataField::Artist => Some(TagField::Artist),
        TrackMetadataField::AlbumArtist => Some(TagField::AlbumArtist),
        TrackMetadataField::Album => Some(TagField::Album),
        TrackMetadataField::TrackNumber => Some(TagField::TrackNumber),
        TrackMetadataField::DiscNumber => Some(TagField::DiscNumber),
        TrackMetadataField::Year => Some(TagField::Year),
        TrackMetadataField::Genre => Some(TagField::Genre),
        TrackMetadataField::Bpm => Some(TagField::Bpm),
        TrackMetadataField::DisplayTitle | TrackMetadataField::Origin => None,
    }
}

fn remove_regular_file_if_present(
    path: &Path,
    operation: &'static str,
) -> Result<(), FilesystemMutationError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(FilesystemMutationError::UnexpectedMutationArtifact)
        }
        Ok(_) => std::fs::remove_file(path)
            .map_err(|source| FilesystemMutationError::Io { operation, source }),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(FilesystemMutationError::Io { operation, source }),
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum MutationArtifactState {
    Missing,
    RegularFile,
}

fn mutation_artifact_state(path: &Path) -> Result<MutationArtifactState, FilesystemMutationError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(FilesystemMutationError::UnexpectedMutationArtifact)
        }
        Ok(_) => Ok(MutationArtifactState::RegularFile),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(MutationArtifactState::Missing)
        }
        Err(source) => Err(FilesystemMutationError::Io {
            operation: "inspect metadata mutation artifact",
            source,
        }),
    }
}

impl LibraryMutationEffects for FilesystemLibraryMutations {
    fn resolve_upload<'a>(
        &'a self,
        requested: &'a music_domain::LibraryPath,
        policy: UploadConflictPolicy,
    ) -> LibraryUploadResolutionFuture<'a> {
        let root = self.root.clone();
        let requested = requested.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                resolve_upload_destination(&root, &requested, policy)
            })
            .await
            .map_err(|source| {
                LibraryMutationFailure::without_recovery(
                    LibraryMutationFailureKind::Io,
                    "upload_resolution_worker_failed",
                    Box::new(source),
                )
            })?
            .map_err(map_failure)
        })
    }

    fn discard_upload<'a>(
        &'a self,
        staged: &'a music_domain::LibraryPath,
    ) -> LibraryUploadDiscardFuture<'a> {
        let root = self.root.clone();
        let staged = staged.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let absolute = root
                    .resolve_for_creation(&staged)
                    .map_err(FilesystemMutationError::RootedPath)?;
                remove_regular_file_if_present(&absolute, "discard staged library upload")
            })
            .await
            .map_err(|source| {
                LibraryMutationFailure::without_recovery(
                    LibraryMutationFailureKind::Io,
                    "upload_discard_worker_failed",
                    Box::new(source),
                )
            })?
            .map_err(map_failure)
        })
    }

    fn apply<'a>(
        &'a self,
        journal_id: &'a RecoveryJournalId,
        mutation: LibraryFileMutation,
        replay: bool,
    ) -> LibraryMutationFuture<'a> {
        let effects = self.clone();
        let journal_id = journal_id.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                effects.apply_blocking(&journal_id, mutation, replay)
            })
            .await
            .map_err(|source| {
                LibraryMutationFailure::new(
                    LibraryMutationFailureKind::Io,
                    "mutation_worker_failed",
                    Box::new(source),
                )
            })?
            .map_err(map_failure)
        })
    }
}

#[derive(Debug)]
enum FilesystemMutationError {
    RootedPath(RootedPathError),
    Discovery(FilesystemDiscoveryError),
    Metadata(MetadataError),
    Io {
        operation: &'static str,
        source: std::io::Error,
    },
    SafeIo {
        operation: &'static str,
        source: std::io::Error,
    },
    NotADirectory,
    DestinationExists,
    DirectoryNotEmpty,
    InvalidMove,
    InvalidMutationPath,
    UnexpectedMutationArtifact,
    RecoveryArtifactConflict,
    UploadStageMissing,
    UploadRenameExhausted,
}

impl Display for FilesystemMutationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootedPath(source) => Display::fmt(source, formatter),
            Self::Discovery(source) => Display::fmt(source, formatter),
            Self::Metadata(source) => Display::fmt(source, formatter),
            Self::Io { operation, .. } => write!(formatter, "failed to {operation}"),
            Self::SafeIo { operation, .. } => write!(formatter, "failed to {operation}"),
            Self::NotADirectory => formatter.write_str("library folder path is not a directory"),
            Self::DestinationExists => formatter.write_str("library destination already exists"),
            Self::DirectoryNotEmpty => formatter.write_str("library folder is not empty"),
            Self::InvalidMove => formatter.write_str("folder destination is invalid"),
            Self::InvalidMutationPath => formatter.write_str("mutation path is invalid"),
            Self::UnexpectedMutationArtifact => {
                formatter.write_str("mutation staging artifact is invalid")
            }
            Self::RecoveryArtifactConflict => {
                formatter.write_str("recovery artifact conflicts with the library destination")
            }
            Self::UploadStageMissing => formatter.write_str("staged library upload is missing"),
            Self::UploadRenameExhausted => {
                formatter.write_str("no available upload filename was found")
            }
        }
    }
}

impl Error for FilesystemMutationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::RootedPath(source) => Some(source),
            Self::Discovery(source) => Some(source),
            Self::Metadata(source) => Some(source),
            Self::Io { source, .. } => Some(source),
            Self::SafeIo { source, .. } => Some(source),
            Self::NotADirectory
            | Self::DestinationExists
            | Self::DirectoryNotEmpty
            | Self::InvalidMove
            | Self::InvalidMutationPath
            | Self::UnexpectedMutationArtifact
            | Self::RecoveryArtifactConflict
            | Self::UploadStageMissing
            | Self::UploadRenameExhausted => None,
        }
    }
}

fn ensure_directory(path: &Path) -> Result<(), FilesystemMutationError> {
    if path.is_dir() {
        Ok(())
    } else {
        Err(FilesystemMutationError::NotADirectory)
    }
}

fn has_child_directories(path: &Path) -> Result<bool, FilesystemMutationError> {
    for entry in std::fs::read_dir(path).map_err(|source| FilesystemMutationError::Io {
        operation: "read library folder",
        source,
    })? {
        let entry = entry.map_err(|source| FilesystemMutationError::Io {
            operation: "read library folder entry",
            source,
        })?;
        let file_type = entry
            .file_type()
            .map_err(|source| FilesystemMutationError::Io {
                operation: "inspect library folder entry",
                source,
            })?;
        if !file_type.is_symlink() && file_type.is_dir() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn rooted_path_is_missing(error: &RootedPathError) -> bool {
    matches!(
        error,
        RootedPathError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound
    )
}

fn map_failure(error: FilesystemMutationError) -> LibraryMutationFailure {
    let (kind, code, recovery_required) = match &error {
        FilesystemMutationError::RootedPath(error) if rooted_path_is_missing(error) => (
            LibraryMutationFailureKind::NotFound,
            "library_path_not_found",
            false,
        ),
        FilesystemMutationError::RootedPath(_)
        | FilesystemMutationError::NotADirectory
        | FilesystemMutationError::InvalidMove => (
            LibraryMutationFailureKind::Invalid,
            "library_path_invalid",
            false,
        ),
        FilesystemMutationError::DestinationExists => (
            LibraryMutationFailureKind::Conflict,
            "library_destination_exists",
            false,
        ),
        FilesystemMutationError::DirectoryNotEmpty => (
            LibraryMutationFailureKind::NotEmpty,
            "folder_not_empty",
            false,
        ),
        FilesystemMutationError::Io { .. } => (
            LibraryMutationFailureKind::Io,
            "library_file_io_failed",
            true,
        ),
        FilesystemMutationError::SafeIo { .. } => (
            LibraryMutationFailureKind::Io,
            "library_file_io_failed",
            false,
        ),
        FilesystemMutationError::Discovery(_) => (
            LibraryMutationFailureKind::Io,
            "track_metadata_refresh_failed",
            true,
        ),
        FilesystemMutationError::Metadata(MetadataError::UnsupportedFormat { .. }) => (
            LibraryMutationFailureKind::Invalid,
            "track_metadata_format_unsupported",
            false,
        ),
        FilesystemMutationError::Metadata(_) => (
            LibraryMutationFailureKind::Io,
            "track_metadata_update_failed",
            false,
        ),
        FilesystemMutationError::InvalidMutationPath => (
            LibraryMutationFailureKind::Invalid,
            "library_mutation_path_invalid",
            false,
        ),
        FilesystemMutationError::UnexpectedMutationArtifact => (
            LibraryMutationFailureKind::Conflict,
            "metadata_mutation_artifact_conflict",
            false,
        ),
        FilesystemMutationError::RecoveryArtifactConflict => (
            LibraryMutationFailureKind::Conflict,
            "library_recovery_artifact_conflict",
            true,
        ),
        FilesystemMutationError::UploadStageMissing => (
            LibraryMutationFailureKind::NotFound,
            "upload_stage_missing",
            false,
        ),
        FilesystemMutationError::UploadRenameExhausted => (
            LibraryMutationFailureKind::Conflict,
            "upload_rename_exhausted",
            false,
        ),
    };
    if recovery_required {
        LibraryMutationFailure::new(kind, code, Box::new(error))
    } else {
        LibraryMutationFailure::without_recovery(kind, code, Box::new(error))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use music_application::library::{
        LibraryFileMutation, LibraryFileMutationOutcome, LibraryMutationEffects,
        LibraryMutationFailureKind, LibraryUploadResolution, TrackMetadataField,
        TrackMetadataPatch, UploadConflictPolicy,
    };
    use music_application::recovery::RecoveryJournalId;
    use music_domain::{LibraryPath, TrackId};
    use tempfile::tempdir;

    use super::{FilesystemLibraryMutations, media_tag_patch, metadata_sibling_path};
    use crate::LibraryRoot;

    #[tokio::test]
    async fn folder_effects_are_rooted_serializable_and_replayable()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let root = directory.path().join("music");
        std::fs::create_dir(&root)?;
        let effects = FilesystemLibraryMutations::new(
            LibraryRoot::open(&root)?,
            crate::MetadataAdapter::native_only(),
        );
        let journal_id = RecoveryJournalId::new();
        let created = effects
            .apply(
                &journal_id,
                LibraryFileMutation::CreateFolder {
                    path: LibraryPath::parse("Old/Nested")?,
                },
                false,
            )
            .await?;
        assert!(matches!(created, LibraryFileMutationOutcome::Folder { .. }));
        let descendant = effects
            .apply(
                &journal_id,
                LibraryFileMutation::RenameFolder {
                    source: LibraryPath::parse("Old")?,
                    destination: LibraryPath::parse("Old/Nested/Moved")?,
                },
                false,
            )
            .await
            .err()
            .ok_or("descendant move unexpectedly succeeded")?;
        assert_eq!(descendant.kind(), LibraryMutationFailureKind::Invalid);
        effects
            .apply(
                &journal_id,
                LibraryFileMutation::RenameFolder {
                    source: LibraryPath::parse("Old")?,
                    destination: LibraryPath::parse("Archive/New")?,
                },
                false,
            )
            .await?;
        assert!(root.join("Archive/New/Nested").is_dir());
        let replayed = effects
            .apply(
                &journal_id,
                LibraryFileMutation::RenameFolder {
                    source: LibraryPath::parse("Old")?,
                    destination: LibraryPath::parse("Archive/New")?,
                },
                true,
            )
            .await?;
        assert!(matches!(
            replayed,
            LibraryFileMutationOutcome::Folder {
                has_children: true,
                ..
            }
        ));
        let not_empty = effects
            .apply(
                &journal_id,
                LibraryFileMutation::DeleteFolder {
                    path: LibraryPath::parse("Archive/New")?,
                    recursive: false,
                },
                false,
            )
            .await
            .err()
            .ok_or("non-recursive delete unexpectedly succeeded")?;
        assert_eq!(not_empty.kind(), LibraryMutationFailureKind::NotEmpty);
        effects
            .apply(
                &journal_id,
                LibraryFileMutation::DeleteFolder {
                    path: LibraryPath::parse("Archive/New")?,
                    recursive: true,
                },
                false,
            )
            .await?;
        assert!(!root.join("Archive/New").exists());

        effects
            .apply(
                &journal_id,
                LibraryFileMutation::CreateFolder {
                    path: LibraryPath::parse("Case/Nested")?,
                },
                false,
            )
            .await?;
        effects
            .apply(
                &journal_id,
                LibraryFileMutation::RenameFolder {
                    source: LibraryPath::parse("Case")?,
                    destination: LibraryPath::parse("case")?,
                },
                false,
            )
            .await?;
        assert!(root.join("case/Nested").is_dir());
        effects
            .apply(
                &journal_id,
                LibraryFileMutation::RenameFolder {
                    source: LibraryPath::parse("Case")?,
                    destination: LibraryPath::parse("case")?,
                },
                true,
            )
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn track_effects_move_delete_and_replay_without_following_untrusted_paths()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let root = directory.path().join("music");
        std::fs::create_dir_all(root.join("Source"))?;
        std::fs::write(root.join("Source/track.mp3"), b"fixture")?;
        let effects = FilesystemLibraryMutations::new(
            LibraryRoot::open(&root)?,
            crate::MetadataAdapter::native_only(),
        );
        let journal_id = RecoveryJournalId::new();
        let track_id = TrackId::new(7)?;
        let mutation = LibraryFileMutation::MoveTrack {
            track_id,
            source: LibraryPath::parse("Source/track.mp3")?,
            destination: LibraryPath::parse("Archive/renamed.mp3")?,
        };
        assert!(matches!(
            effects.apply(&journal_id, mutation.clone(), false).await?,
            LibraryFileMutationOutcome::TrackMoved { track, .. }
                if track.path.as_str() == "Archive/renamed.mp3"
        ));
        assert_eq!(std::fs::read(root.join("Archive/renamed.mp3"))?, b"fixture");
        assert!(matches!(
            effects.apply(&journal_id, mutation, true).await?,
            LibraryFileMutationOutcome::TrackMoved { .. }
        ));

        effects
            .apply(
                &journal_id,
                LibraryFileMutation::DeleteTrack {
                    track_id,
                    path: LibraryPath::parse("Archive/renamed.mp3")?,
                },
                false,
            )
            .await?;
        assert!(!root.join("Archive/renamed.mp3").exists());
        assert!(matches!(
            effects
                .apply(
                    &journal_id,
                    LibraryFileMutation::DeleteTrack {
                        track_id,
                        path: LibraryPath::parse("Archive/renamed.mp3")?,
                    },
                    true,
                )
                .await?,
            LibraryFileMutationOutcome::TrackDeleted { .. }
        ));

        let missing = effects
            .apply(
                &journal_id,
                LibraryFileMutation::MoveTrack {
                    track_id,
                    source: LibraryPath::parse("missing.mp3")?,
                    destination: LibraryPath::parse("elsewhere.mp3")?,
                },
                false,
            )
            .await
            .err()
            .ok_or("missing track move unexpectedly succeeded")?;
        assert_eq!(missing.kind(), LibraryMutationFailureKind::NotFound);
        Ok(())
    }

    #[tokio::test]
    async fn metadata_effects_replace_verified_media_and_replay_forward()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../contracts/reference/v1/metadata.examples.json"
        ))?;
        let wav = fixture["cases"]
            .as_array()
            .and_then(|cases| {
                cases
                    .iter()
                    .find(|case| case["extension"].as_str() == Some(".wav"))
            })
            .ok_or("WAV metadata fixture is missing")?;
        let source_bytes = STANDARD.decode(
            wav["source_base64"]
                .as_str()
                .ok_or("WAV fixture payload is missing")?,
        )?;

        let directory = tempdir()?;
        let root = directory.path().join("music");
        std::fs::create_dir_all(root.join("Album"))?;
        std::fs::write(root.join("Album/song.wav"), &source_bytes)?;
        let effects = FilesystemLibraryMutations::new(
            LibraryRoot::open(&root)?,
            crate::MetadataAdapter::native_only(),
        );
        let mut patch = TrackMetadataPatch::new();
        patch.insert_text(
            TrackMetadataField::Title,
            Some("Journaled title".to_owned()),
        )?;
        patch.insert_text(
            TrackMetadataField::Artist,
            Some("Journaled artist".to_owned()),
        )?;
        patch.insert_text(
            TrackMetadataField::DisplayTitle,
            Some("Database title".to_owned()),
        )?;
        let journal_id = RecoveryJournalId::new();
        let mutation = LibraryFileMutation::UpdateTrackMetadata {
            track_id: TrackId::new(9)?,
            path: LibraryPath::parse("Album/song.wav")?,
            patch,
        };

        let applied = effects.apply(&journal_id, mutation.clone(), false).await?;
        assert!(matches!(
            applied,
            LibraryFileMutationOutcome::TrackMetadataUpdated {
                discovered: Some(track),
                ..
            } if track.metadata.title == "Journaled title"
                && track.metadata.artist == "Journaled artist"
        ));
        assert!(std::fs::read_dir(root.join("Album"))?.all(|entry| {
            entry.is_ok_and(|entry| !entry.file_name().to_string_lossy().contains("metadata-"))
        }));

        let replayed = effects.apply(&journal_id, mutation, true).await?;
        assert!(matches!(
            replayed,
            LibraryFileMutationOutcome::TrackMetadataUpdated {
                discovered: Some(track),
                ..
            } if track.metadata.title == "Journaled title"
        ));

        let mut second_patch = TrackMetadataPatch::new();
        second_patch.insert_text(TrackMetadataField::Artist, Some("Second artist".to_owned()))?;
        let second = effects
            .apply(
                &RecoveryJournalId::new(),
                LibraryFileMutation::UpdateTrackMetadata {
                    track_id: TrackId::new(9)?,
                    path: LibraryPath::parse("Album/song.wav")?,
                    patch: second_patch,
                },
                false,
            )
            .await;
        assert!(second.is_ok(), "{second:?}");

        std::fs::write(root.join("Album/recovery.wav"), &source_bytes)?;
        let recovery_id = RecoveryJournalId::new();
        let recovery_path = LibraryPath::parse("Album/recovery.wav")?;
        let recovery_stage = metadata_sibling_path(&recovery_path, &recovery_id, "stage")?;
        let recovery_backup = metadata_sibling_path(&recovery_path, &recovery_id, "backup")?;
        let mut recovery_patch = TrackMetadataPatch::new();
        recovery_patch.insert_text(
            TrackMetadataField::Title,
            Some("Recovered title".to_owned()),
        )?;
        effects
            .metadata
            .stage_update(
                &root.join("Album/recovery.wav"),
                &root.join(recovery_stage.as_str()),
                &media_tag_patch(&recovery_patch)?,
            )?
            .persist()?;
        std::fs::rename(
            root.join("Album/recovery.wav"),
            root.join(recovery_backup.as_str()),
        )?;
        let recovered = effects
            .apply(
                &recovery_id,
                LibraryFileMutation::UpdateTrackMetadata {
                    track_id: TrackId::new(10)?,
                    path: recovery_path,
                    patch: recovery_patch,
                },
                true,
            )
            .await?;
        assert!(matches!(
            recovered,
            LibraryFileMutationOutcome::TrackMetadataUpdated {
                discovered: Some(track),
                ..
            } if track.metadata.title == "Recovered title"
        ));
        assert!(!root.join(recovery_stage.as_str()).exists());
        assert!(!root.join(recovery_backup.as_str()).exists());
        Ok(())
    }

    #[tokio::test]
    async fn upload_effects_resolve_publish_replace_skip_and_replay()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../contracts/reference/v1/metadata.examples.json"
        ))?;
        let wav = fixture["cases"]
            .as_array()
            .and_then(|cases| {
                cases
                    .iter()
                    .find(|case| case["extension"].as_str() == Some(".wav"))
            })
            .and_then(|case| case["source_base64"].as_str())
            .ok_or("WAV metadata fixture is missing")?;
        let source_bytes = STANDARD.decode(wav)?;
        let directory = tempdir()?;
        let root = directory.path().join("music");
        std::fs::create_dir_all(root.join("Uploads"))?;
        let effects = FilesystemLibraryMutations::new(
            LibraryRoot::open(&root)?,
            crate::MetadataAdapter::native_only(),
        );

        let requested = LibraryPath::parse("Uploads/song.wav")?;
        let staged = LibraryPath::parse("Uploads/.upload-first.partial")?;
        std::fs::write(root.join(staged.as_str()), &source_bytes)?;
        assert_eq!(
            effects
                .resolve_upload(&requested, UploadConflictPolicy::Rename)
                .await?,
            LibraryUploadResolution::Publish {
                destination: requested.clone(),
                replace_existing: false,
            }
        );
        let first_id = RecoveryJournalId::new();
        let first = LibraryFileMutation::PublishUpload {
            staged: staged.clone(),
            destination: requested.clone(),
            replace_existing: false,
        };
        assert!(matches!(
            effects.apply(&first_id, first.clone(), false).await?,
            LibraryFileMutationOutcome::UploadPublished {
                discovered: Some(_),
                ..
            }
        ));
        assert!(!root.join(staged.as_str()).exists());
        assert!(root.join(requested.as_str()).is_file());
        assert!(matches!(
            effects.apply(&first_id, first, true).await?,
            LibraryFileMutationOutcome::UploadPublished { .. }
        ));

        let renamed_stage = LibraryPath::parse("Uploads/.upload-second.partial")?;
        std::fs::write(root.join(renamed_stage.as_str()), &source_bytes)?;
        let LibraryUploadResolution::Publish {
            destination: renamed,
            replace_existing,
        } = effects
            .resolve_upload(&requested, UploadConflictPolicy::Rename)
            .await?
        else {
            return Err("rename upload was unexpectedly skipped".into());
        };
        assert_eq!(renamed.as_str(), "Uploads/song-1.wav");
        assert!(!replace_existing);
        effects
            .apply(
                &RecoveryJournalId::new(),
                LibraryFileMutation::PublishUpload {
                    staged: renamed_stage,
                    destination: renamed.clone(),
                    replace_existing,
                },
                false,
            )
            .await?;
        assert!(root.join(renamed.as_str()).is_file());

        let overwrite_stage = LibraryPath::parse("Uploads/.upload-overwrite.partial")?;
        std::fs::write(root.join(overwrite_stage.as_str()), &source_bytes)?;
        assert!(matches!(
            effects
                .resolve_upload(&requested, UploadConflictPolicy::Overwrite)
                .await?,
            LibraryUploadResolution::Publish {
                replace_existing: true,
                ..
            }
        ));
        effects
            .apply(
                &RecoveryJournalId::new(),
                LibraryFileMutation::PublishUpload {
                    staged: overwrite_stage,
                    destination: requested.clone(),
                    replace_existing: true,
                },
                false,
            )
            .await?;

        let skipped_stage = LibraryPath::parse("Uploads/.upload-skipped.partial")?;
        std::fs::write(root.join(skipped_stage.as_str()), &source_bytes)?;
        assert_eq!(
            effects
                .resolve_upload(&requested, UploadConflictPolicy::Skip)
                .await?,
            LibraryUploadResolution::Skip
        );
        effects.discard_upload(&skipped_stage).await?;
        assert!(!root.join(skipped_stage.as_str()).exists());

        let replay_stage = LibraryPath::parse("Uploads/.upload-replay.partial")?;
        let replay_target = LibraryPath::parse("Uploads/replay.wav")?;
        std::fs::write(root.join(replay_stage.as_str()), &source_bytes)?;
        std::fs::hard_link(
            root.join(replay_stage.as_str()),
            root.join(replay_target.as_str()),
        )?;
        effects
            .apply(
                &RecoveryJournalId::new(),
                LibraryFileMutation::PublishUpload {
                    staged: replay_stage.clone(),
                    destination: replay_target,
                    replace_existing: false,
                },
                true,
            )
            .await?;
        assert!(!root.join(replay_stage.as_str()).exists());
        Ok(())
    }
}
