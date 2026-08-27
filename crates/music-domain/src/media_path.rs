use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::marker::PhantomData;
use std::str::FromStr;

/// Maximum UTF-8 storage size for a canonical media path. This matches the
/// existing SQLite `tracks.path` contract and is checked before filesystem or
/// database access.
pub const MAX_MEDIA_PATH_BYTES: usize = 1_024;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum LibraryMedia {}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum SfxMedia {}

/// A canonical, non-empty, POSIX-relative path associated with one configured
/// media root. The marker prevents a library path from being passed to an SFX
/// root (or the reverse) by accident.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct RootedMediaPath<Kind> {
    encoded: String,
    marker: PhantomData<fn() -> Kind>,
}

pub type LibraryPath = RootedMediaPath<LibraryMedia>;
pub type SfxPath = RootedMediaPath<SfxMedia>;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MediaPathError {
    Empty,
    TooLong,
    Absolute,
    PlatformSeparator,
    EmptyComponent,
    CurrentDirectoryComponent,
    ParentDirectoryComponent,
    PlatformPrefix,
    ControlCharacter,
}

impl Display for MediaPathError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "media path must not be empty",
            Self::TooLong => "media path exceeds 1024 UTF-8 bytes",
            Self::Absolute => "media path must be relative",
            Self::PlatformSeparator => "media path must use forward slashes",
            Self::EmptyComponent => "media path contains an empty component",
            Self::CurrentDirectoryComponent => "media path contains a current-directory component",
            Self::ParentDirectoryComponent => "media path contains a parent-directory component",
            Self::PlatformPrefix => "media path contains a platform-specific path prefix",
            Self::ControlCharacter => "media path contains a control character",
        })
    }
}

impl Error for MediaPathError {}

impl<Kind> RootedMediaPath<Kind> {
    pub fn parse(value: impl Into<String>) -> Result<Self, MediaPathError> {
        let value = value.into();
        validate(&value)?;
        Ok(Self {
            encoded: value,
            marker: PhantomData,
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.encoded
    }

    #[must_use]
    pub fn file_name(&self) -> &str {
        self.encoded
            .rsplit_once('/')
            .map_or(self.encoded.as_str(), |(_, file_name)| file_name)
    }

    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        self.encoded.rsplit_once('/').map(|(parent, _)| Self {
            encoded: parent.to_owned(),
            marker: PhantomData,
        })
    }

    pub fn join(&self, descendant: &str) -> Result<Self, MediaPathError> {
        Self::parse(format!("{}/{descendant}", self.encoded))
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.encoded
    }
}

impl<Kind> Display for RootedMediaPath<Kind> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.encoded)
    }
}

impl<Kind> AsRef<str> for RootedMediaPath<Kind> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl<Kind> FromStr for RootedMediaPath<Kind> {
    type Err = MediaPathError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl<Kind> TryFrom<String> for RootedMediaPath<Kind> {
    type Error = MediaPathError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

fn validate(value: &str) -> Result<(), MediaPathError> {
    if value.is_empty() {
        return Err(MediaPathError::Empty);
    }
    if value.len() > MAX_MEDIA_PATH_BYTES {
        return Err(MediaPathError::TooLong);
    }
    if value.starts_with('/') {
        return Err(MediaPathError::Absolute);
    }
    if value.contains('\\') {
        return Err(MediaPathError::PlatformSeparator);
    }
    if value.chars().any(char::is_control) {
        return Err(MediaPathError::ControlCharacter);
    }

    for component in value.split('/') {
        if component.is_empty() {
            return Err(MediaPathError::EmptyComponent);
        }
        if component == "." {
            return Err(MediaPathError::CurrentDirectoryComponent);
        }
        if component == ".." {
            return Err(MediaPathError::ParentDirectoryComponent);
        }
        if has_windows_drive_prefix(component) {
            return Err(MediaPathError::PlatformPrefix);
        }
    }
    Ok(())
}

fn has_windows_drive_prefix(component: &str) -> bool {
    let bytes = component.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{LibraryPath, MediaPathError, SfxPath};

    #[test]
    fn preserves_valid_portable_paths_and_typed_relationships() -> Result<(), MediaPathError> {
        let path = LibraryPath::parse("Albums/Björk/01 - Jóga.flac")?;
        assert_eq!(path.as_str(), "Albums/Björk/01 - Jóga.flac");
        assert_eq!(path.file_name(), "01 - Jóga.flac");
        assert_eq!(
            path.parent().as_ref().map(LibraryPath::as_str),
            Some("Albums/Björk")
        );
        assert_eq!(
            path.parent()
                .ok_or(MediaPathError::Empty)?
                .join("Covers/front.jpg")?
                .as_str(),
            "Albums/Björk/Covers/front.jpg"
        );
        assert_eq!(SfxPath::parse("cues/bell.opus")?.file_name(), "bell.opus");
        Ok(())
    }

    #[test]
    fn rejects_ambiguous_absolute_and_traversal_forms() {
        let invalid = [
            "",
            "/absolute/file.mp3",
            "C:/music/file.mp3",
            "C:relative.mp3",
            "folder/C:/file.mp3",
            r"\\server\share\file.mp3",
            r"folder\file.mp3",
            "folder//file.mp3",
            "folder/./file.mp3",
            "folder/../file.mp3",
            "../file.mp3",
            "folder/",
            "folder/line\nbreak.mp3",
            "folder/nul\0byte.mp3",
        ];
        for candidate in invalid {
            assert!(
                LibraryPath::parse(candidate).is_err(),
                "unexpectedly accepted {candidate:?}"
            );
        }
    }

    proptest! {
        #[test]
        fn every_accepted_path_is_already_canonical(candidate in any::<String>()) {
            if let Ok(path) = LibraryPath::parse(candidate.clone()) {
                prop_assert_eq!(path.as_str(), candidate);
                prop_assert!(!path.as_str().starts_with('/'));
                prop_assert!(!path.as_str().contains('\\'));
                prop_assert!(path.as_str().split('/').all(|part| !part.is_empty() && part != "." && part != ".."));
                prop_assert!(!path.as_str().chars().any(char::is_control));
            }
        }
    }
}
