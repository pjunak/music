use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};

use serde_json::{Map, Value};

const MAX_TEXT_LENGTH: usize = 512;
const MAX_GENRE_LENGTH: usize = 128;
const MAX_NUMERIC_VALUE: u32 = 9_999;

#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub enum TrackMetadataField {
    Title,
    Artist,
    AlbumArtist,
    Album,
    TrackNumber,
    DiscNumber,
    Year,
    Genre,
    Bpm,
    DisplayTitle,
    Origin,
}

impl TrackMetadataField {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Artist => "artist",
            Self::AlbumArtist => "album_artist",
            Self::Album => "album",
            Self::TrackNumber => "track_no",
            Self::DiscNumber => "disc_no",
            Self::Year => "year",
            Self::Genre => "genre",
            Self::Bpm => "bpm",
            Self::DisplayTitle => "display_title",
            Self::Origin => "origin",
        }
    }

    pub fn parse(value: &str) -> Result<Self, TrackMetadataPatchError> {
        match value {
            "title" => Ok(Self::Title),
            "artist" => Ok(Self::Artist),
            "album_artist" => Ok(Self::AlbumArtist),
            "album" => Ok(Self::Album),
            "track_no" => Ok(Self::TrackNumber),
            "disc_no" => Ok(Self::DiscNumber),
            "year" => Ok(Self::Year),
            "genre" => Ok(Self::Genre),
            "bpm" => Ok(Self::Bpm),
            "display_title" => Ok(Self::DisplayTitle),
            "origin" => Ok(Self::Origin),
            _ => Err(TrackMetadataPatchError::UnknownField),
        }
    }

    #[must_use]
    pub const fn is_tag_backed(self) -> bool {
        !matches!(self, Self::DisplayTitle | Self::Origin)
    }

    #[must_use]
    pub const fn is_numeric(self) -> bool {
        matches!(
            self,
            Self::TrackNumber | Self::DiscNumber | Self::Year | Self::Bpm
        )
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TrackMetadataPatchValue {
    Text(String),
    Number(u32),
    Cleared,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TrackMetadataPatch {
    changes: BTreeMap<TrackMetadataField, TrackMetadataPatchValue>,
}

impl TrackMetadataPatch {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_text(
        &mut self,
        field: TrackMetadataField,
        value: Option<String>,
    ) -> Result<(), TrackMetadataPatchError> {
        if field.is_numeric() {
            return Err(TrackMetadataPatchError::WrongValueType { field });
        }
        let value = match value {
            Some(value) if value.contains('\0') => {
                return Err(TrackMetadataPatchError::InvalidTextCharacter { field });
            }
            Some(value) => {
                let maximum = if field == TrackMetadataField::Genre {
                    MAX_GENRE_LENGTH
                } else {
                    MAX_TEXT_LENGTH
                };
                if value.chars().count() > maximum {
                    return Err(TrackMetadataPatchError::TextTooLong { field, maximum });
                }
                TrackMetadataPatchValue::Text(value)
            }
            None => TrackMetadataPatchValue::Cleared,
        };
        self.insert(field, value)
    }

    pub fn insert_number(
        &mut self,
        field: TrackMetadataField,
        value: Option<u32>,
    ) -> Result<(), TrackMetadataPatchError> {
        if !field.is_numeric() {
            return Err(TrackMetadataPatchError::WrongValueType { field });
        }
        let value = match value {
            Some(value) if value > MAX_NUMERIC_VALUE => {
                return Err(TrackMetadataPatchError::NumberOutOfRange { field });
            }
            Some(value) => TrackMetadataPatchValue::Number(value),
            None => TrackMetadataPatchValue::Cleared,
        };
        self.insert(field, value)
    }

    fn insert(
        &mut self,
        field: TrackMetadataField,
        value: TrackMetadataPatchValue,
    ) -> Result<(), TrackMetadataPatchError> {
        if self.changes.insert(field, value).is_some() {
            return Err(TrackMetadataPatchError::DuplicateField { field });
        }
        Ok(())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    #[must_use]
    pub fn has_tag_changes(&self) -> bool {
        self.changes.keys().any(|field| field.is_tag_backed())
    }

    #[must_use]
    pub fn has_database_only_changes(&self) -> bool {
        self.changes.keys().any(|field| !field.is_tag_backed())
    }

    #[must_use]
    pub fn database_only(&self) -> Self {
        Self {
            changes: self
                .changes
                .iter()
                .filter(|(field, _)| !field.is_tag_backed())
                .map(|(&field, value)| (field, value.clone()))
                .collect(),
        }
    }

    pub fn changes(&self) -> impl Iterator<Item = (TrackMetadataField, &TrackMetadataPatchValue)> {
        self.changes.iter().map(|(&field, value)| (field, value))
    }

    #[must_use]
    pub fn to_json(&self) -> Value {
        Value::Object(
            self.changes
                .iter()
                .map(|(&field, value)| {
                    let value = match value {
                        TrackMetadataPatchValue::Text(value) => Value::String(value.clone()),
                        TrackMetadataPatchValue::Number(value) => Value::from(*value),
                        TrackMetadataPatchValue::Cleared => Value::Null,
                    };
                    (field.as_str().to_owned(), value)
                })
                .collect::<Map<_, _>>(),
        )
    }

    pub fn from_json(value: &Value) -> Result<Self, TrackMetadataPatchError> {
        let object = value
            .as_object()
            .ok_or(TrackMetadataPatchError::InvalidJson)?;
        if object.len() > 11 {
            return Err(TrackMetadataPatchError::InvalidJson);
        }
        let mut patch = Self::new();
        for (name, value) in object {
            let field = TrackMetadataField::parse(name)?;
            if field.is_numeric() {
                let value = match value {
                    Value::Null => None,
                    Value::Number(value) => value
                        .as_u64()
                        .and_then(|value| u32::try_from(value).ok())
                        .map(Some)
                        .ok_or(TrackMetadataPatchError::InvalidJson)?,
                    _ => return Err(TrackMetadataPatchError::InvalidJson),
                };
                patch.insert_number(field, value)?;
            } else {
                let value = match value {
                    Value::Null => None,
                    Value::String(value) => Some(value.clone()),
                    _ => return Err(TrackMetadataPatchError::InvalidJson),
                };
                patch.insert_text(field, value)?;
            }
        }
        Ok(patch)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TrackMetadataPatchError {
    UnknownField,
    InvalidJson,
    DuplicateField {
        field: TrackMetadataField,
    },
    WrongValueType {
        field: TrackMetadataField,
    },
    TextTooLong {
        field: TrackMetadataField,
        maximum: usize,
    },
    InvalidTextCharacter {
        field: TrackMetadataField,
    },
    NumberOutOfRange {
        field: TrackMetadataField,
    },
}

impl Display for TrackMetadataPatchError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownField => formatter.write_str("metadata patch field is unknown"),
            Self::InvalidJson => formatter.write_str("metadata patch JSON is invalid"),
            Self::DuplicateField { field } => {
                write!(formatter, "metadata patch repeats {}", field.as_str())
            }
            Self::WrongValueType { field } => {
                write!(
                    formatter,
                    "metadata patch has the wrong type for {}",
                    field.as_str()
                )
            }
            Self::TextTooLong { field, maximum } => {
                write!(formatter, "{} exceeds {maximum} characters", field.as_str())
            }
            Self::InvalidTextCharacter { field } => {
                write!(formatter, "{} contains a null character", field.as_str())
            }
            Self::NumberOutOfRange { field } => {
                write!(formatter, "{} exceeds {MAX_NUMERIC_VALUE}", field.as_str())
            }
        }
    }
}

impl Error for TrackMetadataPatchError {}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{TrackMetadataField, TrackMetadataPatch, TrackMetadataPatchValue};

    #[test]
    fn patch_round_trip_preserves_unset_clear_and_typed_values()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut patch = TrackMetadataPatch::new();
        patch.insert_text(TrackMetadataField::Title, Some("New title".to_owned()))?;
        patch.insert_text(TrackMetadataField::Genre, None)?;
        patch.insert_number(TrackMetadataField::TrackNumber, Some(7))?;
        let encoded = patch.to_json();
        assert_eq!(
            encoded,
            json!({"title": "New title", "genre": null, "track_no": 7})
        );
        assert_eq!(TrackMetadataPatch::from_json(&encoded)?, patch);
        assert!(patch.has_tag_changes());
        assert!(patch.changes().any(|(field, value)| {
            field == TrackMetadataField::Genre && value == &TrackMetadataPatchValue::Cleared
        }));
        Ok(())
    }
}
