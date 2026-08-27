use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::path::Path;

use music_application::library::{
    LibraryFileMutation, LibraryFileMutationOutcome, LibraryMutationEffects,
    LibraryMutationFailure, LibraryMutationFailureKind, LibraryMutationFuture,
};

use crate::{LibraryRoot, RootedPathError};

#[derive(Debug, Clone)]
pub struct FilesystemLibraryMutations {
    root: LibraryRoot,
}

impl FilesystemLibraryMutations {
    #[must_use]
    pub const fn new(root: LibraryRoot) -> Self {
        Self { root }
    }

    fn apply_blocking(
        &self,
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
        }
    }
}

impl LibraryMutationEffects for FilesystemLibraryMutations {
    fn apply(&self, mutation: LibraryFileMutation, replay: bool) -> LibraryMutationFuture<'_> {
        let effects = self.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || effects.apply_blocking(mutation, replay))
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
    Io {
        operation: &'static str,
        source: std::io::Error,
    },
    NotADirectory,
    DestinationExists,
    DirectoryNotEmpty,
    InvalidMove,
}

impl Display for FilesystemMutationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootedPath(source) => Display::fmt(source, formatter),
            Self::Io { operation, .. } => write!(formatter, "failed to {operation}"),
            Self::NotADirectory => formatter.write_str("library folder path is not a directory"),
            Self::DestinationExists => formatter.write_str("folder destination already exists"),
            Self::DirectoryNotEmpty => formatter.write_str("library folder is not empty"),
            Self::InvalidMove => formatter.write_str("folder destination is invalid"),
        }
    }
}

impl Error for FilesystemMutationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::RootedPath(source) => Some(source),
            Self::Io { source, .. } => Some(source),
            Self::NotADirectory
            | Self::DestinationExists
            | Self::DirectoryNotEmpty
            | Self::InvalidMove => None,
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
    let (kind, code) = match &error {
        FilesystemMutationError::RootedPath(error) if rooted_path_is_missing(error) => {
            (LibraryMutationFailureKind::NotFound, "folder_not_found")
        }
        FilesystemMutationError::RootedPath(_)
        | FilesystemMutationError::NotADirectory
        | FilesystemMutationError::InvalidMove => {
            (LibraryMutationFailureKind::Invalid, "folder_path_invalid")
        }
        FilesystemMutationError::DestinationExists => (
            LibraryMutationFailureKind::Conflict,
            "folder_destination_exists",
        ),
        FilesystemMutationError::DirectoryNotEmpty => {
            (LibraryMutationFailureKind::NotEmpty, "folder_not_empty")
        }
        FilesystemMutationError::Io { .. } => (LibraryMutationFailureKind::Io, "folder_io_failed"),
    };
    LibraryMutationFailure::new(kind, code, Box::new(error))
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use music_application::library::{
        LibraryFileMutation, LibraryFileMutationOutcome, LibraryMutationEffects,
        LibraryMutationFailureKind,
    };
    use music_domain::LibraryPath;
    use tempfile::tempdir;

    use super::FilesystemLibraryMutations;
    use crate::LibraryRoot;

    #[tokio::test]
    async fn folder_effects_are_rooted_serializable_and_replayable()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let root = directory.path().join("music");
        std::fs::create_dir(&root)?;
        let effects = FilesystemLibraryMutations::new(LibraryRoot::open(&root)?);
        let created = effects
            .apply(
                LibraryFileMutation::CreateFolder {
                    path: LibraryPath::parse("Old/Nested")?,
                },
                false,
            )
            .await?;
        assert!(matches!(created, LibraryFileMutationOutcome::Folder { .. }));
        let descendant = effects
            .apply(
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
                LibraryFileMutation::CreateFolder {
                    path: LibraryPath::parse("Case/Nested")?,
                },
                false,
            )
            .await?;
        effects
            .apply(
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
                LibraryFileMutation::RenameFolder {
                    source: LibraryPath::parse("Case")?,
                    destination: LibraryPath::parse("case")?,
                },
                true,
            )
            .await?;
        Ok(())
    }
}
