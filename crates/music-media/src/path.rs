use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use music_domain::{LibraryMedia, RootedMediaPath, SfxMedia};

pub type LibraryRoot = MediaRoot<LibraryMedia>;
pub type SfxRoot = MediaRoot<SfxMedia>;

/// A canonical filesystem capability for one configured media directory.
/// Callers can only resolve paths carrying the matching domain marker.
#[derive(Debug, Clone)]
pub struct MediaRoot<Kind> {
    canonical: PathBuf,
    marker: PhantomData<fn() -> Kind>,
}

#[derive(Debug)]
pub enum RootedPathError {
    Io {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    RootIsNotDirectory(PathBuf),
    EscapesRoot(PathBuf),
    ParentIsNotDirectory(PathBuf),
    SymbolicLinkTarget(PathBuf),
}

impl Display for RootedPathError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation, path, ..
            } => write!(formatter, "failed to {operation} {}", path.display()),
            Self::RootIsNotDirectory(path) => {
                write!(
                    formatter,
                    "media root is not a directory: {}",
                    path.display()
                )
            }
            Self::EscapesRoot(path) => {
                write!(
                    formatter,
                    "resolved media path escapes its root: {}",
                    path.display()
                )
            }
            Self::ParentIsNotDirectory(path) => write!(
                formatter,
                "media creation parent is not a directory: {}",
                path.display()
            ),
            Self::SymbolicLinkTarget(path) => write!(
                formatter,
                "media creation target must not be a symbolic link: {}",
                path.display()
            ),
        }
    }
}

impl Error for RootedPathError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::RootIsNotDirectory(_)
            | Self::EscapesRoot(_)
            | Self::ParentIsNotDirectory(_)
            | Self::SymbolicLinkTarget(_) => None,
        }
    }
}

impl<Kind> MediaRoot<Kind> {
    /// Open an existing root. Root creation is an explicit startup concern so
    /// a typo cannot silently create a second empty media library.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RootedPathError> {
        let requested = path.as_ref();
        let canonical = canonicalize("canonicalize media root", requested)?;
        let metadata = std::fs::metadata(&canonical).map_err(|source| RootedPathError::Io {
            operation: "inspect media root",
            path: canonical.clone(),
            source,
        })?;
        if !metadata.is_dir() {
            return Err(RootedPathError::RootIsNotDirectory(canonical));
        }
        Ok(Self {
            canonical,
            marker: PhantomData,
        })
    }

    #[must_use]
    pub fn canonical_path(&self) -> &Path {
        &self.canonical
    }

    /// Resolve an existing path to its canonical location and reject symlink
    /// traversal outside the configured root.
    pub fn resolve_existing(
        &self,
        relative: &RootedMediaPath<Kind>,
    ) -> Result<PathBuf, RootedPathError> {
        let candidate = self.canonical.join(posix_to_native(relative.as_str()));
        let resolved = canonicalize("canonicalize existing media path", &candidate)?;
        self.ensure_beneath_root(resolved)
    }

    /// Resolve a target whose parent already exists. Returning a path built
    /// from the canonical parent avoids retaining a user-controlled parent
    /// symlink in the write path. Existing symlink targets are refused.
    pub fn resolve_for_creation(
        &self,
        relative: &RootedMediaPath<Kind>,
    ) -> Result<PathBuf, RootedPathError> {
        let parent = match relative.parent() {
            Some(parent) => {
                let candidate = self.canonical.join(posix_to_native(parent.as_str()));
                let resolved = canonicalize("canonicalize media creation parent", &candidate)?;
                self.ensure_beneath_root(resolved)?
            }
            None => self.canonical.clone(),
        };
        let metadata = std::fs::metadata(&parent).map_err(|source| RootedPathError::Io {
            operation: "inspect media creation parent",
            path: parent.clone(),
            source,
        })?;
        if !metadata.is_dir() {
            return Err(RootedPathError::ParentIsNotDirectory(parent));
        }
        let target = parent.join(relative.file_name());
        match std::fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                Err(RootedPathError::SymbolicLinkTarget(target))
            }
            Ok(_) => self.ensure_beneath_root(target),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                self.ensure_beneath_root(target)
            }
            Err(source) => Err(RootedPathError::Io {
                operation: "inspect media creation target",
                path: target,
                source,
            }),
        }
    }

    fn ensure_beneath_root(&self, candidate: PathBuf) -> Result<PathBuf, RootedPathError> {
        if candidate.starts_with(&self.canonical) && candidate != self.canonical {
            Ok(candidate)
        } else {
            Err(RootedPathError::EscapesRoot(candidate))
        }
    }
}

fn canonicalize(operation: &'static str, path: &Path) -> Result<PathBuf, RootedPathError> {
    std::fs::canonicalize(path).map_err(|source| RootedPathError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    })
}

fn posix_to_native(path: &str) -> PathBuf {
    path.split('/').collect()
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use music_domain::{LibraryPath, SfxPath};
    use tempfile::tempdir;

    use super::{LibraryRoot, RootedPathError, SfxRoot};

    #[test]
    fn resolves_existing_and_creation_paths_from_canonical_parents() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let music = directory.path().join("music");
        std::fs::create_dir_all(music.join("Albums"))?;
        std::fs::write(music.join("Albums").join("track.flac"), b"fixture")?;
        let root = LibraryRoot::open(&music)?;

        assert_eq!(
            root.resolve_existing(&LibraryPath::parse("Albums/track.flac")?)?,
            std::fs::canonicalize(music.join("Albums").join("track.flac"))?
        );
        assert_eq!(
            root.resolve_for_creation(&LibraryPath::parse("Albums/new.flac")?)?,
            std::fs::canonicalize(music.join("Albums"))?.join("new.flac")
        );
        assert!(
            root.resolve_for_creation(&LibraryPath::parse("Missing/new.flac")?)
                .is_err()
        );

        let sfx = directory.path().join("sfx");
        std::fs::create_dir(&sfx)?;
        let sfx_root = SfxRoot::open(sfx)?;
        assert!(
            sfx_root
                .resolve_for_creation(&SfxPath::parse("bell.opus")?)
                .is_ok()
        );
        Ok(())
    }

    #[test]
    fn rejects_a_file_as_the_configured_root() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let file = directory.path().join("not-a-directory");
        std::fs::write(&file, b"fixture")?;
        assert!(matches!(
            LibraryRoot::open(file),
            Err(RootedPathError::RootIsNotDirectory(_))
        ));
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn rejects_symlink_escape_for_reads_and_writes() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let music = directory.path().join("music");
        let outside = directory.path().join("outside");
        std::fs::create_dir(&music)?;
        std::fs::create_dir(&outside)?;
        std::fs::write(outside.join("secret.flac"), b"fixture")?;
        if let Err(source) = symlink_directory(&outside, &music.join("escape")) {
            // Windows may require Developer Mode or an elevated token to make
            // a test symlink. The production rejection path is still covered
            // wherever the host permits creating the fixture.
            if cfg!(windows)
                && (source.kind() == std::io::ErrorKind::PermissionDenied
                    || source.raw_os_error() == Some(1_314))
            {
                return Ok(());
            }
            return Err(source.into());
        }
        let root = LibraryRoot::open(&music)?;

        assert!(matches!(
            root.resolve_existing(&LibraryPath::parse("escape/secret.flac")?),
            Err(RootedPathError::EscapesRoot(_))
        ));
        assert!(matches!(
            root.resolve_for_creation(&LibraryPath::parse("escape/new.flac")?),
            Err(RootedPathError::EscapesRoot(_))
        ));
        Ok(())
    }

    #[cfg(unix)]
    fn symlink_directory(
        source: &std::path::Path,
        target: &std::path::Path,
    ) -> std::io::Result<()> {
        std::os::unix::fs::symlink(source, target)
    }

    #[cfg(windows)]
    fn symlink_directory(
        source: &std::path::Path,
        target: &std::path::Path,
    ) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(source, target)
    }
}
