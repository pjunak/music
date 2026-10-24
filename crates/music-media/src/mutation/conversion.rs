//! Journaled conversion and restore. Each step is a rename or a synced copy, and
//! a replay resumes from whatever the files show. Every check that can refuse
//! the work runs before the first file moves, so a refused mutation leaves the
//! library untouched and every later failure is left to recovery.

use music_application::library::LibraryFileMutationOutcome;
use music_application::recovery::RecoveryJournalId;
use music_domain::{LibraryPath, TrackId};

use super::originals::{
    Transfer, before_any_change, file_state, find_original, original_facts, partial_path,
    transfer_file,
};
use super::{
    FilesystemMutationError, MutationArtifactState, UploadTargetState,
    remove_regular_file_if_present, upload_target_state,
};
use crate::{LibraryRoot, MetadataAdapter, inspect_library_track};

pub(super) struct ConversionRoots<'a> {
    pub(super) library: &'a LibraryRoot,
    pub(super) originals: &'a LibraryRoot,
    pub(super) metadata: &'a MetadataAdapter,
}

pub(super) struct TrackConversion<'a> {
    pub(super) track_id: TrackId,
    pub(super) source: &'a LibraryPath,
    pub(super) staged: &'a LibraryPath,
    pub(super) destination: &'a LibraryPath,
}

/// Keeps `source` at the same path under the originals root, publishes the
/// staged streaming copy at `destination` and removes the library's `source`.
pub(super) fn apply_track_conversion(
    roots: &ConversionRoots<'_>,
    journal_id: &RecoveryJournalId,
    conversion: &TrackConversion<'_>,
    replay: bool,
) -> Result<LibraryFileMutationOutcome, FilesystemMutationError> {
    let TrackConversion {
        track_id,
        source,
        staged,
        destination,
    } = *conversion;
    ensure_sibling(source, destination)?;
    let same_path = source == destination;
    let (source_state, source_absolute) = file_state(roots.library, source)?;
    let staged_absolute = roots
        .library
        .resolve_for_creation(staged)
        .map_err(FilesystemMutationError::RootedPath)?;
    let staged_state = super::mutation_artifact_state(&staged_absolute)?;
    if let Some(parent) = source.parent() {
        roots
            .originals
            .ensure_directory(&parent)
            .map_err(FilesystemMutationError::RootedPath)?;
    }
    let original_absolute = roots
        .originals
        .resolve_for_creation(source)
        .map_err(FilesystemMutationError::RootedPath)?;
    let original_partial = roots
        .originals
        .resolve_for_creation(&partial_path(source, journal_id, "original")?)
        .map_err(FilesystemMutationError::RootedPath)?;
    let destination_absolute = roots
        .library
        .resolve_for_creation(destination)
        .map_err(FilesystemMutationError::RootedPath)?;

    let original_state = super::mutation_artifact_state(&original_absolute)?;
    if original_state == MutationArtifactState::RegularFile && !replay {
        // An original this mutation did not keep must never stand in for the source.
        return Err(FilesystemMutationError::DestinationExists);
    }
    let mut transfer = None;
    if original_state == MutationArtifactState::Missing {
        let Some(source_absolute) = source_absolute.as_ref() else {
            return Err(FilesystemMutationError::TrackFileMissing);
        };
        if source_state != MutationArtifactState::RegularFile {
            return Err(FilesystemMutationError::TrackFileMissing);
        }
        if staged_state != MutationArtifactState::RegularFile {
            remove_regular_file_if_present(&original_partial, "remove stale original copy")?;
            return Err(FilesystemMutationError::ConversionStageMissing);
        }
        if !same_path && upload_target_state(&destination_absolute)? != UploadTargetState::Missing {
            return Err(FilesystemMutationError::DestinationExists);
        }
        if find_original(roots.originals, source)?.is_some() {
            return Err(FilesystemMutationError::DestinationExists);
        }
        transfer = Some(
            transfer_file(source_absolute, &original_absolute, &original_partial)
                .map_err(before_any_change)?,
        );
    }

    // The original is kept; from here every failure needs recovery to finish.
    if super::mutation_artifact_state(&staged_absolute)? == MutationArtifactState::RegularFile {
        if !same_path && upload_target_state(&destination_absolute)? != UploadTargetState::Missing {
            return Err(FilesystemMutationError::RecoveryArtifactConflict);
        }
        std::fs::rename(&staged_absolute, &destination_absolute).map_err(|source| {
            FilesystemMutationError::Io {
                operation: "publish converted track",
                source,
            }
        })?;
    } else if transfer.is_none() && !stage_was_published(&destination_absolute, &original_absolute)?
    {
        // A replay found the stage gone before it reached the library: undo.
        roll_back_kept_original(
            roots.library,
            journal_id,
            source,
            &original_absolute,
            source_absolute.as_deref(),
        )?;
        return Err(FilesystemMutationError::ConversionStageMissing);
    }
    if !same_path && let Some(source_absolute) = source_absolute.as_ref() {
        remove_regular_file_if_present(source_absolute, "remove converted track source")?;
    }
    let track = inspect_library_track(roots.library, roots.metadata, destination)
        .map_err(FilesystemMutationError::Discovery)?;
    let original = original_facts(roots.originals, source)?;
    Ok(LibraryFileMutationOutcome::TrackConverted {
        track_id,
        track,
        original,
    })
}

/// Returns the original kept at `destination` under the originals root to the
/// library, replacing the converted file at `path`.
pub(super) fn apply_track_restore(
    roots: &ConversionRoots<'_>,
    journal_id: &RecoveryJournalId,
    track_id: TrackId,
    path: &LibraryPath,
    destination: &LibraryPath,
    replay: bool,
) -> Result<LibraryFileMutationOutcome, FilesystemMutationError> {
    ensure_sibling(path, destination)?;
    let same_path = path == destination;
    let (original_state, original_absolute) = file_state(roots.originals, destination)?;
    let (_, path_absolute) = file_state(roots.library, path)?;
    let destination_absolute = roots
        .library
        .resolve_for_creation(destination)
        .map_err(FilesystemMutationError::RootedPath)?;
    let restore_partial = roots
        .library
        .resolve_for_creation(&partial_path(destination, journal_id, "restore")?)
        .map_err(FilesystemMutationError::RootedPath)?;
    let destination_state = upload_target_state(&destination_absolute)?;

    if original_state == MutationArtifactState::Missing && !replay {
        return Err(FilesystemMutationError::OriginalMissing);
    }
    if let (MutationArtifactState::RegularFile, Some(original_absolute)) =
        (original_state, original_absolute.as_ref())
    {
        if !replay && !same_path && destination_state != UploadTargetState::Missing {
            return Err(FilesystemMutationError::DestinationExists);
        }
        if destination_state == UploadTargetState::Occupied {
            return Err(FilesystemMutationError::RecoveryArtifactConflict);
        }
        if transfer_file(original_absolute, &destination_absolute, &restore_partial)
            .map_err(before_any_change)?
            == Transfer::Copied
        {
            remove_regular_file_if_present(original_absolute, "remove restored original")?;
        }
    } else if destination_state != UploadTargetState::RegularFile {
        return Err(FilesystemMutationError::OriginalMissing);
    }

    if !same_path && let Some(path_absolute) = path_absolute.as_ref() {
        remove_regular_file_if_present(path_absolute, "remove converted track")?;
    }
    let track = inspect_library_track(roots.library, roots.metadata, destination)
        .map_err(FilesystemMutationError::Discovery)?;
    Ok(LibraryFileMutationOutcome::TrackRestored { track_id, track })
}

/// Whether the library file at `destination` is the published streaming copy
/// rather than the not yet replaced source. A copied original keeps the
/// source's size and modification time, so matching facts mean unpublished.
fn stage_was_published(
    destination: &std::path::Path,
    original: &std::path::Path,
) -> Result<bool, FilesystemMutationError> {
    if upload_target_state(destination)? != UploadTargetState::RegularFile {
        return Ok(false);
    }
    let facts = |path: &std::path::Path| {
        std::fs::metadata(path)
            .and_then(|metadata| Ok((metadata.len(), metadata.modified()?)))
            .map_err(|source| FilesystemMutationError::Io {
                operation: "compare converted track with its original",
                source,
            })
    };
    Ok(facts(destination)? != facts(original)?)
}

fn roll_back_kept_original(
    library: &LibraryRoot,
    journal_id: &RecoveryJournalId,
    source: &LibraryPath,
    original: &std::path::Path,
    source_absolute: Option<&std::path::Path>,
) -> Result<(), FilesystemMutationError> {
    let source_absolute =
        source_absolute.ok_or(FilesystemMutationError::RecoveryArtifactConflict)?;
    if upload_target_state(source_absolute)? == UploadTargetState::RegularFile {
        return remove_regular_file_if_present(original, "remove unused original copy");
    }
    let partial = library
        .resolve_for_creation(&partial_path(source, journal_id, "rollback")?)
        .map_err(FilesystemMutationError::RootedPath)?;
    if transfer_file(original, source_absolute, &partial)? == Transfer::Copied {
        remove_regular_file_if_present(original, "remove rolled back original")?;
    }
    Ok(())
}

fn ensure_sibling(left: &LibraryPath, right: &LibraryPath) -> Result<(), FilesystemMutationError> {
    if left.parent() == right.parent() && left.file_stem() == right.file_stem() {
        Ok(())
    } else {
        Err(FilesystemMutationError::InvalidMutationPath)
    }
}
