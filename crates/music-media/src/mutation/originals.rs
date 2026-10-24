//! Kept originals of converted tracks. The originals root mirrors the library:
//! a track's original sits at the library path's folder and file stem with its
//! own extension, so every library file operation mirrors onto it here.

use std::path::{Path, PathBuf};

use music_application::recovery::RecoveryJournalId;
use music_domain::{LibraryPath, TrackOriginal};

use super::{
    FilesystemMutationError, MutationArtifactState, mutation_artifact_state,
    remove_regular_file_if_present, rooted_path_is_missing,
};
use crate::{LibraryRoot, is_supported_library_path};

/// The kept original sharing `path`'s folder and stem, if any. Hidden files are
/// mutation artifacts, never originals.
pub(super) fn find_original(
    originals: &LibraryRoot,
    path: &LibraryPath,
) -> Result<Option<LibraryPath>, FilesystemMutationError> {
    let directory = match path.parent() {
        Some(parent) => match originals.resolve_existing_directory(&parent) {
            Ok(directory) => directory,
            Err(error) if rooted_path_is_missing(&error) => return Ok(None),
            Err(error) => return Err(FilesystemMutationError::RootedPath(error)),
        },
        None => originals.canonical_path().to_path_buf(),
    };
    let entries = std::fs::read_dir(&directory).map_err(|source| FilesystemMutationError::Io {
        operation: "read kept originals folder",
        source,
    })?;
    let mut found = None;
    for entry in entries {
        let entry = entry.map_err(|source| FilesystemMutationError::Io {
            operation: "read kept originals entry",
            source,
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let candidate = sibling(path, &name)?;
        if candidate.file_stem() != path.file_stem() || !is_supported_library_path(&candidate) {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|source| FilesystemMutationError::Io {
                operation: "inspect kept original",
                source,
            })?;
        if file_type.is_symlink() || !file_type.is_file() {
            return Err(FilesystemMutationError::UnexpectedMutationArtifact);
        }
        if found.replace(candidate).is_some() {
            return Err(FilesystemMutationError::AmbiguousOriginal);
        }
    }
    Ok(found)
}

/// Like [`find_original`], but a replayed tag edit may have left the original
/// only as that journal's hidden stage or backup copy, which names it.
pub(super) fn find_original_for_tag_edit(
    originals: &LibraryRoot,
    path: &LibraryPath,
    journal_id: &RecoveryJournalId,
    replay: bool,
) -> Result<Option<LibraryPath>, FilesystemMutationError> {
    if let Some(original) = find_original(originals, path)? {
        return Ok(Some(original));
    }
    if !replay {
        return Ok(None);
    }
    let directory = match path.parent() {
        Some(parent) => match originals.resolve_existing_directory(&parent) {
            Ok(directory) => directory,
            Err(error) if rooted_path_is_missing(&error) => return Ok(None),
            Err(error) => return Err(FilesystemMutationError::RootedPath(error)),
        },
        None => originals.canonical_path().to_path_buf(),
    };
    let suffixes =
        ["stage", "backup"].map(|role| format!(".{}.metadata-{role}", journal_id.as_str()));
    let entries = std::fs::read_dir(&directory).map_err(|source| FilesystemMutationError::Io {
        operation: "read kept originals folder",
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| FilesystemMutationError::Io {
            operation: "read kept originals entry",
            source,
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(hidden) = name.strip_prefix('.') else {
            continue;
        };
        for suffix in &suffixes {
            if let Some(original_name) = hidden.strip_suffix(suffix.as_str()) {
                let candidate = sibling(path, original_name)?;
                if candidate.file_stem() == path.file_stem()
                    && is_supported_library_path(&candidate)
                {
                    return Ok(Some(candidate));
                }
            }
        }
    }
    Ok(None)
}

/// The original's facts as the index stores them.
pub(super) fn original_facts(
    originals: &LibraryRoot,
    path: &LibraryPath,
) -> Result<TrackOriginal, FilesystemMutationError> {
    let absolute = originals
        .resolve_existing_file_for_mutation(path)
        .map_err(FilesystemMutationError::RootedPath)?;
    let metadata = std::fs::metadata(&absolute).map_err(|source| FilesystemMutationError::Io {
        operation: "inspect kept original",
        source,
    })?;
    let mtime_unix_seconds = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or_default();
    let extension = Path::new(path.file_name())
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or(FilesystemMutationError::InvalidMutationPath)?
        .to_owned();
    Ok(TrackOriginal {
        extension,
        size_bytes: metadata.len(),
        mtime_unix_seconds,
    })
}

/// Moves the original of the library file at `source` beside `destination`.
/// Runs before the library move, so a conflict here leaves both untouched and a
/// replay after the original moved finds nothing left to move.
pub(super) fn move_original(
    originals: &LibraryRoot,
    source: &LibraryPath,
    destination: &LibraryPath,
    case_only: bool,
) -> Result<(), FilesystemMutationError> {
    let Some(original) = find_original(originals, source)? else {
        return Ok(());
    };
    let extension = Path::new(original.file_name())
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or(FilesystemMutationError::InvalidMutationPath)?;
    let target = destination
        .with_extension(extension)
        .map_err(|_| FilesystemMutationError::InvalidMutationPath)?;
    if target == original {
        return Ok(());
    }
    let original_absolute = originals
        .resolve_existing_file_for_mutation(&original)
        .map_err(FilesystemMutationError::RootedPath)?;
    if let Some(parent) = target.parent() {
        originals
            .ensure_directory(&parent)
            .map_err(FilesystemMutationError::RootedPath)?;
    }
    let target_absolute = originals
        .resolve_for_creation(&target)
        .map_err(FilesystemMutationError::RootedPath)?;
    if !case_only && mutation_artifact_state(&target_absolute)? != MutationArtifactState::Missing {
        return Err(FilesystemMutationError::DestinationExists);
    }
    std::fs::rename(&original_absolute, &target_absolute).map_err(|source| {
        FilesystemMutationError::SafeIo {
            operation: "move kept original",
            source,
        }
    })
}

pub(super) fn delete_original(
    originals: &LibraryRoot,
    path: &LibraryPath,
) -> Result<(), FilesystemMutationError> {
    let Some(original) = find_original(originals, path)? else {
        return Ok(());
    };
    let absolute = originals
        .resolve_existing_file_for_mutation(&original)
        .map_err(FilesystemMutationError::RootedPath)?;
    std::fs::remove_file(absolute).map_err(|source| FilesystemMutationError::SafeIo {
        operation: "delete kept original",
        source,
    })
}

/// Renames the mirrored originals folder. Runs before the library rename; a
/// missing source means there is nothing to mirror or it already moved.
pub(super) fn rename_original_folder(
    originals: &LibraryRoot,
    source: &LibraryPath,
    destination: &LibraryPath,
    case_only: bool,
) -> Result<(), FilesystemMutationError> {
    let source_absolute = match originals.resolve_existing_directory(source) {
        Ok(path) => path,
        Err(error) if rooted_path_is_missing(&error) => return Ok(()),
        Err(error) => return Err(FilesystemMutationError::RootedPath(error)),
    };
    if case_only
        && source_absolute.file_name().and_then(|name| name.to_str())
            == Some(destination.file_name())
    {
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        originals
            .ensure_directory(&parent)
            .map_err(FilesystemMutationError::RootedPath)?;
    }
    let destination_absolute = originals
        .resolve_for_creation(destination)
        .map_err(FilesystemMutationError::RootedPath)?;
    if !case_only && path_exists(&destination_absolute)? {
        return Err(FilesystemMutationError::DestinationExists);
    }
    std::fs::rename(&source_absolute, &destination_absolute).map_err(|source| {
        FilesystemMutationError::SafeIo {
            operation: "rename kept originals folder",
            source,
        }
    })
}

/// Removes the mirrored originals folder after its library folder is gone. A
/// non-recursive delete keeps originals that no longer belong to any track.
pub(super) fn delete_original_folder(
    originals: &LibraryRoot,
    path: &LibraryPath,
    recursive: bool,
) -> Result<(), FilesystemMutationError> {
    let absolute = match originals.resolve_existing_directory(path) {
        Ok(path) => path,
        Err(error) if rooted_path_is_missing(&error) => return Ok(()),
        Err(error) => return Err(FilesystemMutationError::RootedPath(error)),
    };
    let result = if recursive {
        std::fs::remove_dir_all(&absolute)
    } else {
        std::fs::remove_dir(&absolute)
    };
    match result {
        Ok(()) => Ok(()),
        Err(source) if !recursive && source.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
            Ok(())
        }
        Err(source) => Err(FilesystemMutationError::Io {
            operation: "delete kept originals folder",
            source,
        }),
    }
}

/// Whether a regular file is at `relative`, treating a missing parent folder
/// as a missing file.
pub(super) fn file_state(
    root: &LibraryRoot,
    relative: &LibraryPath,
) -> Result<(MutationArtifactState, Option<PathBuf>), FilesystemMutationError> {
    match root.resolve_for_creation(relative) {
        Ok(absolute) => Ok((mutation_artifact_state(&absolute)?, Some(absolute))),
        Err(error) if rooted_path_is_missing(&error) => Ok((MutationArtifactState::Missing, None)),
        Err(error) => Err(FilesystemMutationError::RootedPath(error)),
    }
}

/// For a step that runs before anything has changed: its I/O failure refuses
/// the mutation instead of leaving it to recovery.
pub(super) fn before_any_change(error: FilesystemMutationError) -> FilesystemMutationError {
    match error {
        FilesystemMutationError::Io { operation, source } => {
            FilesystemMutationError::SafeIo { operation, source }
        }
        other => other,
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum Transfer {
    Renamed,
    /// The source remains for the caller to remove once the copy is in place.
    Copied,
}

/// Places `source` at `target`, which may be in another root on another
/// filesystem. A rename is used where possible; otherwise a synced copy that
/// keeps the modification time is written to `partial` and renamed into place.
pub(super) fn transfer_file(
    source: &Path,
    target: &Path,
    partial: &Path,
) -> Result<Transfer, FilesystemMutationError> {
    match std::fs::rename(source, target) {
        Ok(()) => Ok(Transfer::Renamed),
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            copy_into_place(source, target, partial)?;
            Ok(Transfer::Copied)
        }
        Err(source) => Err(FilesystemMutationError::Io {
            operation: "move track file between roots",
            source,
        }),
    }
}

/// Writes a synced copy of `source` that keeps its modification time to
/// `partial`, then renames it to `target`.
pub(super) fn copy_into_place(
    source: &Path,
    target: &Path,
    partial: &Path,
) -> Result<(), FilesystemMutationError> {
    remove_regular_file_if_present(partial, "remove stale cross-root copy")?;
    let modified = std::fs::metadata(source)
        .and_then(|metadata| metadata.modified())
        .map_err(|source| FilesystemMutationError::Io {
            operation: "inspect cross-root copy source",
            source,
        })?;
    std::fs::copy(source, partial).map_err(|source| FilesystemMutationError::Io {
        operation: "copy track file between roots",
        source,
    })?;
    let copy = std::fs::OpenOptions::new()
        .write(true)
        .open(partial)
        .map_err(|source| FilesystemMutationError::Io {
            operation: "open cross-root copy",
            source,
        })?;
    copy.set_modified(modified)
        .and_then(|()| copy.sync_all())
        .map_err(|source| FilesystemMutationError::Io {
            operation: "finish cross-root copy",
            source,
        })?;
    drop(copy);
    std::fs::rename(partial, target).map_err(|source| FilesystemMutationError::Io {
        operation: "publish cross-root copy",
        source,
    })
}

/// A hidden sibling of `path` for one journaled mutation's cross-root copy.
pub(super) fn partial_path(
    path: &LibraryPath,
    journal_id: &RecoveryJournalId,
    role: &'static str,
) -> Result<LibraryPath, FilesystemMutationError> {
    sibling(
        path,
        &format!(
            ".{}.{}.{role}-partial",
            path.file_name(),
            journal_id.as_str()
        ),
    )
}

fn sibling(path: &LibraryPath, name: &str) -> Result<LibraryPath, FilesystemMutationError> {
    path.parent()
        .map_or_else(|| LibraryPath::parse(name), |parent| parent.join(name))
        .map_err(|_| FilesystemMutationError::InvalidMutationPath)
}

fn path_exists(path: &Path) -> Result<bool, FilesystemMutationError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(FilesystemMutationError::Io {
            operation: "inspect kept originals destination",
            source,
        }),
    }
}
