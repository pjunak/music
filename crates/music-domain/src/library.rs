use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::time::Duration;

use crate::{LibraryPath, TrackId};

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct LibraryGeneration(u64);

impl LibraryGeneration {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Result<Self, LibraryRecordError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(LibraryRecordError::GenerationOverflow)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TrackMetadata {
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub year: Option<u32>,
    pub genre: String,
    pub bpm: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexedTrack {
    pub id: TrackId,
    pub path: LibraryPath,
    pub metadata: TrackMetadata,
    pub duration: Duration,
    pub display_title: String,
    pub origin: String,
    pub size_bytes: u64,
    pub mtime_unix_seconds: i64,
    pub added_at_unix_seconds: i64,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LibraryRecordError {
    GenerationOverflow,
    NegativeGeneration,
    InvalidDuration,
    NegativeSize,
    NumericFieldOutOfRange(&'static str),
}

impl Display for LibraryRecordError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::GenerationOverflow => formatter.write_str("library generation overflowed"),
            Self::NegativeGeneration => formatter.write_str("library generation is negative"),
            Self::InvalidDuration => formatter.write_str("track duration is invalid"),
            Self::NegativeSize => formatter.write_str("track size is negative"),
            Self::NumericFieldOutOfRange(field) => {
                write!(
                    formatter,
                    "track field {field} is outside its supported range"
                )
            }
        }
    }
}

impl Error for LibraryRecordError {}

impl TryFrom<i64> for LibraryGeneration {
    type Error = LibraryRecordError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u64::try_from(value)
            .map(Self)
            .map_err(|_| LibraryRecordError::NegativeGeneration)
    }
}

#[cfg(test)]
mod tests {
    use super::{LibraryGeneration, LibraryRecordError};

    #[test]
    fn generation_is_monotonic_and_checked() -> Result<(), LibraryRecordError> {
        assert_eq!(LibraryGeneration::try_from(7_i64)?.next()?.get(), 8);
        assert_eq!(
            LibraryGeneration::try_from(-1_i64),
            Err(LibraryRecordError::NegativeGeneration)
        );
        assert_eq!(
            LibraryGeneration::new(u64::MAX).next(),
            Err(LibraryRecordError::GenerationOverflow)
        );
        Ok(())
    }
}
