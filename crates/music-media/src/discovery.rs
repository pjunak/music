use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use music_application::library::{
    DiscoveredTrack, LibraryDiscovery, LibraryDiscoveryFailure, LibraryDiscoveryFuture,
};
use music_domain::{LibraryPath, MediaPathError, TrackMetadata};
use tokio_util::sync::CancellationToken;

use crate::{LibraryRoot, MetadataAdapter, RootedPathError};

const MAX_SCAN_DEPTH: usize = 128;
const MAX_SCAN_ENTRIES: usize = 1_000_000;
const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "ogg", "opus", "m4a", "aac", "wav", "wma"];

#[derive(Debug, Clone)]
pub struct FilesystemLibraryDiscovery {
    root: LibraryRoot,
    metadata: MetadataAdapter,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LibraryDirectory {
    pub path: LibraryPath,
    pub name: String,
    pub has_children: bool,
}

pub fn list_library_directories(
    root: &LibraryRoot,
) -> Result<Vec<LibraryDirectory>, FilesystemDiscoveryError> {
    let mut remaining = MAX_SCAN_ENTRIES;
    let mut paths = Vec::new();
    let mut work = vec![(root.canonical_path().to_path_buf(), 0_usize)];
    while let Some((directory, depth)) = work.pop() {
        if depth > MAX_SCAN_DEPTH {
            return Err(FilesystemDiscoveryError::DepthLimit);
        }
        let entries = std::fs::read_dir(&directory)
            .map_err(|source| FilesystemDiscoveryError::Io {
                operation: "read library directory",
                path: directory.clone(),
                source,
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| FilesystemDiscoveryError::Io {
                operation: "read library directory entry",
                path: directory,
                source,
            })?;
        for entry in entries {
            if remaining == 0 {
                return Err(FilesystemDiscoveryError::EntryLimit);
            }
            remaining -= 1;
            let file_type = entry
                .file_type()
                .map_err(|source| FilesystemDiscoveryError::Io {
                    operation: "inspect library directory entry",
                    path: entry.path(),
                    source,
                })?;
            if file_type.is_symlink() || !file_type.is_dir() {
                continue;
            }
            let path = library_path(root, &entry.path())?;
            let absolute = root
                .resolve_existing(&path)
                .map_err(FilesystemDiscoveryError::RootedPath)?;
            if !absolute.is_dir() {
                return Err(FilesystemDiscoveryError::ChangedDuringScan(absolute));
            }
            paths.push(path);
            work.push((absolute, depth + 1));
        }
    }
    paths.sort_by(|left, right| {
        left.as_str()
            .to_lowercase()
            .cmp(&right.as_str().to_lowercase())
            .then_with(|| left.cmp(right))
    });
    let parents = paths
        .iter()
        .filter_map(LibraryPath::parent)
        .collect::<BTreeSet<_>>();
    Ok(paths
        .into_iter()
        .map(|path| LibraryDirectory {
            name: path.file_name().to_owned(),
            has_children: parents.contains(&path),
            path,
        })
        .collect())
}

impl FilesystemLibraryDiscovery {
    #[must_use]
    pub const fn new(root: LibraryRoot, metadata: MetadataAdapter) -> Self {
        Self { root, metadata }
    }

    fn discover_blocking(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<Vec<DiscoveredTrack>, FilesystemDiscoveryError> {
        let mut tracks = Vec::new();
        let mut remaining = MAX_SCAN_ENTRIES;
        let mut work = vec![DiscoveryWork::Directory {
            path: self.root.canonical_path().to_path_buf(),
            depth: 0,
        }];
        while let Some(item) = work.pop() {
            if cancellation.is_cancelled() {
                return Err(FilesystemDiscoveryError::Cancelled);
            }
            match item {
                DiscoveryWork::Directory { path, depth } => {
                    if depth > MAX_SCAN_DEPTH {
                        return Err(FilesystemDiscoveryError::DepthLimit);
                    }
                    let mut entries = std::fs::read_dir(&path)
                        .map_err(|source| FilesystemDiscoveryError::Io {
                            operation: "read library directory",
                            path: path.clone(),
                            source,
                        })?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|source| FilesystemDiscoveryError::Io {
                            operation: "read library directory entry",
                            path: path.clone(),
                            source,
                        })?;
                    entries.sort_by_key(std::fs::DirEntry::file_name);
                    for entry in entries.into_iter().rev() {
                        if remaining == 0 {
                            return Err(FilesystemDiscoveryError::EntryLimit);
                        }
                        remaining -= 1;
                        let file_type =
                            entry
                                .file_type()
                                .map_err(|source| FilesystemDiscoveryError::Io {
                                    operation: "inspect library directory entry",
                                    path: entry.path(),
                                    source,
                                })?;
                        if file_type.is_symlink() {
                            continue;
                        }
                        if file_type.is_dir() {
                            work.push(DiscoveryWork::Directory {
                                path: entry.path(),
                                depth: depth + 1,
                            });
                        } else if file_type.is_file() && is_audio_file(&entry.path()) {
                            work.push(DiscoveryWork::AudioFile(entry.path()));
                        }
                    }
                }
                DiscoveryWork::AudioFile(path) => tracks.push(self.discover_track(&path)?),
            }
        }
        tracks.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(tracks)
    }

    fn discover_track(
        &self,
        candidate: &Path,
    ) -> Result<DiscoveredTrack, FilesystemDiscoveryError> {
        let path = library_path(&self.root, candidate)?;
        inspect_library_track(&self.root, &self.metadata, &path)
    }
}

pub fn inspect_library_track(
    root: &LibraryRoot,
    metadata_adapter: &MetadataAdapter,
    path: &LibraryPath,
) -> Result<DiscoveredTrack, FilesystemDiscoveryError> {
    let absolute = root
        .resolve_existing_file_for_mutation(path)
        .map_err(FilesystemDiscoveryError::RootedPath)?;
    let relative = absolute
        .strip_prefix(root.canonical_path())
        .map_err(|_| FilesystemDiscoveryError::EscapedRoot(absolute.clone()))?;
    let facts = std::fs::metadata(&absolute).map_err(|source| FilesystemDiscoveryError::Io {
        operation: "inspect discovered audio file",
        path: absolute.clone(),
        source,
    })?;
    if !facts.is_file() {
        return Err(FilesystemDiscoveryError::ChangedDuringScan(absolute));
    }
    if facts.len() > i64::MAX as u64 {
        return Err(FilesystemDiscoveryError::FileTooLarge(absolute));
    }
    let metadata = metadata_adapter
        .read(&absolute)
        .unwrap_or_else(|_| fallback_metadata(path));
    let album_artist = if metadata.album_artist.is_empty() {
        metadata.artist.clone()
    } else {
        metadata.album_artist
    };
    Ok(DiscoveredTrack {
        path: path.clone(),
        metadata: TrackMetadata {
            title: if metadata.title.is_empty() {
                fallback_title(&absolute)
            } else {
                metadata.title
            },
            artist: metadata.artist,
            album_artist,
            album: if metadata.album.is_empty() {
                fallback_album(relative)
            } else {
                metadata.album
            },
            track_no: metadata.track_no,
            disc_no: metadata.disc_no,
            year: metadata.year,
            genre: metadata.genre,
            bpm: metadata.bpm,
        },
        duration: metadata.duration,
        size_bytes: facts.len(),
        mtime_unix_seconds: unix_seconds(facts.modified().map_err(|source| {
            FilesystemDiscoveryError::Io {
                operation: "read audio modification time",
                path: absolute,
                source,
            }
        })?)?,
    })
}

#[must_use]
pub fn is_supported_library_path(path: &LibraryPath) -> bool {
    Path::new(path.as_str())
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(is_audio_extension)
}

impl LibraryDiscovery for FilesystemLibraryDiscovery {
    fn discover(&self, cancellation: CancellationToken) -> LibraryDiscoveryFuture<'_> {
        let discovery = self.clone();
        Box::pin(async move {
            match tokio::task::spawn_blocking(move || discovery.discover_blocking(&cancellation))
                .await
            {
                Ok(Ok(tracks)) => Ok(tracks),
                Ok(Err(source)) => Err(LibraryDiscoveryFailure::new(
                    source.code(),
                    Box::new(source),
                )),
                Err(source) => Err(LibraryDiscoveryFailure::new(
                    "scan_worker_failed",
                    Box::new(source),
                )),
            }
        })
    }
}

#[derive(Debug)]
enum DiscoveryWork {
    Directory { path: PathBuf, depth: usize },
    AudioFile(PathBuf),
}

#[derive(Debug)]
pub enum FilesystemDiscoveryError {
    Io {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    InvalidPath(MediaPathError),
    RootedPath(RootedPathError),
    NonUnicode(PathBuf),
    EscapedRoot(PathBuf),
    ChangedDuringScan(PathBuf),
    FileTooLarge(PathBuf),
    TimestampOutOfRange,
    Cancelled,
    DepthLimit,
    EntryLimit,
}

impl FilesystemDiscoveryError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Io { .. } => "scan_io_failed",
            Self::InvalidPath(_) => "scan_path_invalid",
            Self::RootedPath(_) | Self::EscapedRoot(_) => "scan_path_escaped",
            Self::NonUnicode(_) => "scan_path_not_unicode",
            Self::ChangedDuringScan(_) => "scan_file_changed",
            Self::FileTooLarge(_) => "scan_file_too_large",
            Self::TimestampOutOfRange => "scan_timestamp_invalid",
            Self::Cancelled => "scan_cancelled",
            Self::DepthLimit => "scan_depth_exceeded",
            Self::EntryLimit => "scan_entry_limit_exceeded",
        }
    }
}

impl Display for FilesystemDiscoveryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation, path, ..
            } => write!(formatter, "failed to {operation} {}", path.display()),
            Self::InvalidPath(_) => formatter.write_str("library contains an invalid stored path"),
            Self::RootedPath(source) => Display::fmt(source, formatter),
            Self::NonUnicode(path) => {
                write!(formatter, "library path is not Unicode: {}", path.display())
            }
            Self::EscapedRoot(path) => {
                write!(
                    formatter,
                    "library path escaped the root: {}",
                    path.display()
                )
            }
            Self::ChangedDuringScan(path) => write!(
                formatter,
                "library entry changed type during scan: {}",
                path.display()
            ),
            Self::FileTooLarge(path) => {
                write!(
                    formatter,
                    "library file is too large to index: {}",
                    path.display()
                )
            }
            Self::TimestampOutOfRange => {
                formatter.write_str("library file timestamp is outside the supported range")
            }
            Self::Cancelled => formatter.write_str("library discovery was cancelled"),
            Self::DepthLimit => formatter.write_str("library exceeds the scan depth limit"),
            Self::EntryLimit => formatter.write_str("library exceeds the scan entry limit"),
        }
    }
}

impl Error for FilesystemDiscoveryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidPath(source) => Some(source),
            Self::RootedPath(source) => Some(source),
            Self::NonUnicode(_)
            | Self::EscapedRoot(_)
            | Self::ChangedDuringScan(_)
            | Self::FileTooLarge(_)
            | Self::TimestampOutOfRange
            | Self::Cancelled
            | Self::DepthLimit
            | Self::EntryLimit => None,
        }
    }
}

fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(is_audio_extension)
}

fn is_audio_extension(extension: &str) -> bool {
    AUDIO_EXTENSIONS
        .iter()
        .any(|candidate| extension.eq_ignore_ascii_case(candidate))
}

fn library_path(
    root: &LibraryRoot,
    candidate: &Path,
) -> Result<LibraryPath, FilesystemDiscoveryError> {
    let relative = candidate
        .strip_prefix(root.canonical_path())
        .map_err(|_| FilesystemDiscoveryError::EscapedRoot(candidate.to_path_buf()))?;
    let encoded = relative
        .components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .ok_or_else(|| FilesystemDiscoveryError::NonUnicode(candidate.to_path_buf()))
        })
        .collect::<Result<Vec<_>, _>>()?
        .join("/");
    LibraryPath::parse(encoded).map_err(FilesystemDiscoveryError::InvalidPath)
}

fn fallback_metadata(path: &LibraryPath) -> crate::AudioMetadata {
    crate::AudioMetadata {
        title: path
            .file_name()
            .rsplit_once('.')
            .map_or(path.file_name(), |(stem, _)| stem)
            .to_owned(),
        artist: String::new(),
        album_artist: String::new(),
        album: path
            .parent()
            .map_or_else(String::new, |parent| parent.file_name().to_owned()),
        track_no: None,
        disc_no: None,
        year: None,
        genre: String::new(),
        bpm: None,
        duration: std::time::Duration::ZERO,
        artwork: None,
    }
}

fn fallback_title(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .or_else(|| path.file_name().and_then(|name| name.to_str()))
        .unwrap_or_default()
        .to_owned()
}

fn fallback_album(relative: &Path) -> String {
    relative
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned()
}

fn unix_seconds(timestamp: SystemTime) -> Result<i64, FilesystemDiscoveryError> {
    match timestamp.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs())
            .map_err(|_| FilesystemDiscoveryError::TimestampOutOfRange),
        Err(before_epoch) => {
            let magnitude = i64::try_from(before_epoch.duration().as_secs())
                .map_err(|_| FilesystemDiscoveryError::TimestampOutOfRange)?;
            magnitude
                .checked_neg()
                .ok_or(FilesystemDiscoveryError::TimestampOutOfRange)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use music_application::library::LibraryDiscovery;
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    use super::{FilesystemLibraryDiscovery, list_library_directories};
    use crate::{LibraryRoot, MetadataAdapter};

    #[tokio::test]
    async fn discovers_supported_files_deterministically_with_metadata_fallbacks()
    -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let root = directory.path().join("music");
        std::fs::create_dir_all(root.join("Album"))?;
        std::fs::write(root.join("Album/02 - Second.MP3"), b"not real audio")?;
        std::fs::write(root.join("Album/01 - First.flac"), b"not real audio")?;
        std::fs::write(root.join("Album/notes.txt"), b"ignored")?;
        let discovery = FilesystemLibraryDiscovery::new(
            LibraryRoot::open(root)?,
            MetadataAdapter::native_only(),
        );

        let tracks = discovery.discover(CancellationToken::new()).await?;
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].path.as_str(), "Album/01 - First.flac");
        assert_eq!(tracks[0].metadata.title, "01 - First");
        assert_eq!(tracks[0].metadata.album, "Album");
        assert_eq!(tracks[1].path.as_str(), "Album/02 - Second.MP3");
        Ok(())
    }

    #[test]
    fn lists_the_complete_directory_tree_without_following_symlinks() -> Result<(), Box<dyn Error>>
    {
        let directory = tempdir()?;
        let root = directory.path().join("music");
        std::fs::create_dir_all(root.join("Campaign/Scenes/Empty"))?;
        std::fs::create_dir_all(root.join("Album"))?;

        let directories = list_library_directories(&LibraryRoot::open(root)?)?;
        let paths = directories
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                "Album",
                "Campaign",
                "Campaign/Scenes",
                "Campaign/Scenes/Empty"
            ]
        );
        assert!(!directories[0].has_children);
        assert!(directories[1].has_children);
        assert!(directories[2].has_children);
        assert!(!directories[3].has_children);
        Ok(())
    }
}
