use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use music_domain::{LibraryPath, MediaPathError};

use crate::{LibraryRoot, MetadataAdapter, RootedPathError};

const MAX_COVER_BYTES: u64 = 16 * 1024 * 1024;
const FOLDER_COVER_NAMES: &[(&str, &str)] = &[
    ("cover.jpg", "image/jpeg"),
    ("cover.jpeg", "image/jpeg"),
    ("cover.png", "image/png"),
    ("folder.jpg", "image/jpeg"),
    ("folder.png", "image/png"),
];

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CoverArt {
    pub bytes: Vec<u8>,
    pub mime_type: String,
}

#[derive(Debug)]
pub enum MediaDeliveryError {
    InvalidPath(MediaPathError),
    RootedPath(RootedPathError),
    Io {
        operation: &'static str,
        source: std::io::Error,
    },
    NotAFile,
    CoverTooLarge,
}

impl Display for MediaDeliveryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath(_) => formatter.write_str("cover fallback path is invalid"),
            Self::RootedPath(source) => Display::fmt(source, formatter),
            Self::Io { operation, .. } => write!(formatter, "failed to {operation}"),
            Self::NotAFile => formatter.write_str("indexed media path is not a regular file"),
            Self::CoverTooLarge => {
                formatter.write_str("cover art exceeds the bounded response size")
            }
        }
    }
}

impl MediaDeliveryError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidPath(_) => "media_path_invalid",
            Self::RootedPath(_) => "media_path_unavailable",
            Self::Io { .. } => "media_io_failed",
            Self::NotAFile => "media_not_file",
            Self::CoverTooLarge => "cover_too_large",
        }
    }

    #[must_use]
    pub fn is_unavailable(&self) -> bool {
        matches!(
            self,
            Self::RootedPath(RootedPathError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound
        ) || matches!(self, Self::NotAFile)
    }
}

impl Error for MediaDeliveryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidPath(source) => Some(source),
            Self::RootedPath(source) => Some(source),
            Self::Io { source, .. } => Some(source),
            Self::NotAFile | Self::CoverTooLarge => None,
        }
    }
}

pub fn resolve_library_media_file(
    root: &LibraryRoot,
    path: &LibraryPath,
) -> Result<PathBuf, MediaDeliveryError> {
    let absolute = root
        .resolve_existing(path)
        .map_err(MediaDeliveryError::RootedPath)?;
    let metadata = std::fs::metadata(&absolute).map_err(|source| MediaDeliveryError::Io {
        operation: "inspect indexed media file",
        source,
    })?;
    if !metadata.is_file() {
        return Err(MediaDeliveryError::NotAFile);
    }
    Ok(absolute)
}

pub fn read_library_cover_art(
    root: &LibraryRoot,
    path: &LibraryPath,
    metadata: &MetadataAdapter,
) -> Result<Option<CoverArt>, MediaDeliveryError> {
    let absolute = resolve_library_media_file(root, path)?;
    if let Ok(audio) = metadata.read(&absolute)
        && let Some(artwork) = audio.artwork
    {
        if artwork.bytes.len() as u64 > MAX_COVER_BYTES {
            return Err(MediaDeliveryError::CoverTooLarge);
        }
        return Ok(Some(CoverArt {
            bytes: artwork.bytes,
            mime_type: artwork.mime_type,
        }));
    }

    for &(name, mime_type) in FOLDER_COVER_NAMES {
        let relative = sibling_path(path, name)?;
        let candidate = match root.resolve_existing(&relative) {
            Ok(candidate) => candidate,
            Err(error) if rooted_path_is_missing(&error) => continue,
            Err(error) => return Err(MediaDeliveryError::RootedPath(error)),
        };
        let Some(bytes) = read_bounded_regular_file(&candidate)? else {
            continue;
        };
        return Ok(Some(CoverArt {
            bytes,
            mime_type: mime_type.to_owned(),
        }));
    }
    Ok(None)
}

fn sibling_path(path: &LibraryPath, name: &str) -> Result<LibraryPath, MediaDeliveryError> {
    let relative = path.parent().map_or_else(
        || name.to_owned(),
        |parent| format!("{}/{name}", parent.as_str()),
    );
    LibraryPath::parse(relative).map_err(MediaDeliveryError::InvalidPath)
}

fn rooted_path_is_missing(error: &RootedPathError) -> bool {
    matches!(
        error,
        RootedPathError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound
    )
}

fn read_bounded_regular_file(path: &Path) -> Result<Option<Vec<u8>>, MediaDeliveryError> {
    let file = File::open(path).map_err(|source| MediaDeliveryError::Io {
        operation: "open folder cover art",
        source,
    })?;
    let metadata = file.metadata().map_err(|source| MediaDeliveryError::Io {
        operation: "inspect folder cover art",
        source,
    })?;
    if !metadata.is_file() {
        return Ok(None);
    }
    if metadata.len() > MAX_COVER_BYTES {
        return Err(MediaDeliveryError::CoverTooLarge);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_COVER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| MediaDeliveryError::Io {
            operation: "read folder cover art",
            source,
        })?;
    if bytes.len() as u64 > MAX_COVER_BYTES {
        return Err(MediaDeliveryError::CoverTooLarge);
    }
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use music_domain::LibraryPath;
    use tempfile::tempdir;

    use super::{read_library_cover_art, resolve_library_media_file};
    use crate::{LibraryRoot, MetadataAdapter};

    #[test]
    fn resolves_stream_files_and_uses_bounded_folder_cover_fallbacks() -> Result<(), Box<dyn Error>>
    {
        let directory = tempdir()?;
        let root_path = directory.path().join("music");
        std::fs::create_dir_all(root_path.join("Album"))?;
        std::fs::write(root_path.join("Album/track.mp3"), b"not real audio")?;
        std::fs::write(root_path.join("Album/cover.png"), b"bounded cover")?;
        let root = LibraryRoot::open(&root_path)?;
        let track = LibraryPath::parse("Album/track.mp3")?;

        assert_eq!(
            resolve_library_media_file(&root, &track)?,
            std::fs::canonicalize(root_path.join("Album/track.mp3"))?
        );
        let cover = read_library_cover_art(&root, &track, &MetadataAdapter::native_only())?
            .ok_or("folder cover was not found")?;
        assert_eq!(cover.bytes, b"bounded cover");
        assert_eq!(cover.mime_type, "image/png");
        Ok(())
    }
}
