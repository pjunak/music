use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use lofty::config::{GlobalOptions, ParseOptions, WriteOptions, apply_global_options};
use lofty::file::{AudioFile, TaggedFile, TaggedFileExt};
use lofty::flac::FlacFile;
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::{OpusFile, VorbisFile};
use lofty::picture::PictureType;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag, TagType};

const MAX_TAG_ITEM_BYTES: usize = 16 * 1024 * 1024;
const MAX_TEXT_LENGTH: usize = 512;
const MAX_GENRE_LENGTH: usize = 128;
const MAX_NUMERIC_VALUE: u32 = 9_999;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct EmbeddedArtwork {
    pub bytes: Vec<u8>,
    pub mime_type: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AudioMetadata {
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub year: Option<u32>,
    pub genre: String,
    pub bpm: Option<u32>,
    pub duration: Duration,
    pub artwork: Option<EmbeddedArtwork>,
}

#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub enum TagField {
    Title,
    Artist,
    AlbumArtist,
    Album,
    TrackNumber,
    DiscNumber,
    Year,
    Genre,
    Bpm,
}

impl Display for TagField {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Title => "title",
            Self::Artist => "artist",
            Self::AlbumArtist => "album_artist",
            Self::Album => "album",
            Self::TrackNumber => "track_no",
            Self::DiscNumber => "disc_no",
            Self::Year => "year",
            Self::Genre => "genre",
            Self::Bpm => "bpm",
        })
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TagValue {
    Text(String),
    Number(u32),
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TagPatch {
    changes: BTreeMap<TagField, Option<TagValue>>,
}

impl TagPatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_text(
        &mut self,
        field: TagField,
        value: impl Into<String>,
    ) -> Result<(), MetadataError> {
        if !is_text_field(field) {
            return Err(MetadataError::WrongValueType { field });
        }
        let value = value.into();
        if value.is_empty() {
            return self.insert_change(field, None);
        }
        let maximum = if field == TagField::Genre {
            MAX_GENRE_LENGTH
        } else {
            MAX_TEXT_LENGTH
        };
        if value.chars().count() > maximum {
            return Err(MetadataError::TextTooLong { field, maximum });
        }
        self.insert_change(field, Some(TagValue::Text(value)))
    }

    pub fn insert_number(&mut self, field: TagField, value: u32) -> Result<(), MetadataError> {
        if !is_numeric_field(field) {
            return Err(MetadataError::WrongValueType { field });
        }
        if value > MAX_NUMERIC_VALUE {
            return Err(MetadataError::NumberOutOfRange {
                field,
                maximum: MAX_NUMERIC_VALUE,
            });
        }
        self.insert_change(field, Some(TagValue::Number(value)))
    }

    pub fn clear(&mut self, field: TagField) -> Result<(), MetadataError> {
        self.insert_change(field, None)
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    fn insert_change(
        &mut self,
        field: TagField,
        value: Option<TagValue>,
    ) -> Result<(), MetadataError> {
        if self.changes.contains_key(&field) {
            return Err(MetadataError::DuplicateField { field });
        }
        self.changes.insert(field, value);
        Ok(())
    }
}

#[derive(Debug)]
pub struct StagedTagUpdate {
    path: Option<PathBuf>,
    metadata: AudioMetadata,
}

impl StagedTagUpdate {
    pub fn path(&self) -> Result<&Path, MetadataError> {
        self.path.as_deref().ok_or(MetadataError::MissingStagedPath)
    }

    pub fn metadata(&self) -> &AudioMetadata {
        &self.metadata
    }

    pub fn persist(mut self) -> Result<PathBuf, MetadataError> {
        self.path.take().ok_or(MetadataError::MissingStagedPath)
    }
}

impl Drop for StagedTagUpdate {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
    }
}

#[derive(Debug)]
pub enum MetadataError {
    UnsupportedFormat {
        extension: String,
    },
    EmptyPatch,
    DuplicateField {
        field: TagField,
    },
    WrongValueType {
        field: TagField,
    },
    TextTooLong {
        field: TagField,
        maximum: usize,
    },
    NumberOutOfRange {
        field: TagField,
        maximum: u32,
    },
    SourceEqualsStaged,
    StagedPathExists {
        path: PathBuf,
    },
    MissingStagedPath,
    Io {
        action: &'static str,
        source: io::Error,
    },
    Parse(String),
    Write(String),
    Verification {
        field: TagField,
    },
    FormatChanged,
    DurationChanged,
}

impl Display for MetadataError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFormat { extension } => {
                write!(formatter, "unsupported metadata format: {extension}")
            }
            Self::EmptyPatch => formatter.write_str("metadata patch contains no changes"),
            Self::DuplicateField { field } => {
                write!(formatter, "duplicate metadata field: {field}")
            }
            Self::WrongValueType { field } => write!(formatter, "wrong value type for {field}"),
            Self::TextTooLong { field, maximum } => {
                write!(formatter, "{field} exceeds {maximum} characters")
            }
            Self::NumberOutOfRange { field, maximum } => {
                write!(formatter, "{field} exceeds {maximum}")
            }
            Self::SourceEqualsStaged => {
                formatter.write_str("source and staged metadata paths must differ")
            }
            Self::StagedPathExists { path } => {
                write!(
                    formatter,
                    "staged metadata path already exists: {}",
                    path.display()
                )
            }
            Self::MissingStagedPath => formatter.write_str("staged metadata path is missing"),
            Self::Io { action, source } => write!(formatter, "{action}: {source}"),
            Self::Parse(message) => write!(formatter, "audio metadata parse failed: {message}"),
            Self::Write(message) => write!(formatter, "audio metadata write failed: {message}"),
            Self::Verification { field } => {
                write!(formatter, "staged metadata verification failed for {field}")
            }
            Self::FormatChanged => formatter.write_str("staged metadata changed the media format"),
            Self::DurationChanged => {
                formatter.write_str("staged metadata changed the reported audio duration")
            }
        }
    }
}

impl Error for MetadataError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub fn read_audio_metadata(path: &Path) -> Result<AudioMetadata, MetadataError> {
    let tagged_file = read_tagged_file(path)?;
    Ok(metadata_from_tagged_file(&tagged_file))
}

pub fn stage_tag_update(
    source: &Path,
    staged: &Path,
    patch: &TagPatch,
) -> Result<StagedTagUpdate, MetadataError> {
    if patch.is_empty() {
        return Err(MetadataError::EmptyPatch);
    }
    if source == staged {
        return Err(MetadataError::SourceEqualsStaged);
    }
    if staged.exists() {
        return Err(MetadataError::StagedPathExists {
            path: staged.to_path_buf(),
        });
    }

    let result = stage_tag_update_inner(source, staged, patch);
    if result.is_err() {
        let _ = fs::remove_file(staged);
    }
    result
}

fn stage_tag_update_inner(
    source: &Path,
    staged: &Path,
    patch: &TagPatch,
) -> Result<StagedTagUpdate, MetadataError> {
    copy_new_file(source, staged)?;

    let before = read_tagged_file(staged)?;
    let original_type = before.file_type();
    let original_duration = before.properties().duration();
    match original_type {
        lofty::file::FileType::Flac => {
            drop(before);
            apply_flac_patch(staged, patch)?;
        }
        lofty::file::FileType::Vorbis => {
            drop(before);
            apply_vorbis_file_patch(staged, patch)?;
        }
        lofty::file::FileType::Opus => {
            drop(before);
            apply_opus_patch(staged, patch)?;
        }
        _ => {
            let mut tagged_file = before;
            apply_generic_patch(&mut tagged_file, patch)?;
            tagged_file
                .save_to_path(staged, write_options())
                .map_err(|error| MetadataError::Write(error.to_string()))?;
        }
    }

    let verified = read_tagged_file(staged)?;
    if verified.file_type() != original_type {
        return Err(MetadataError::FormatChanged);
    }
    if verified.properties().duration() != original_duration {
        return Err(MetadataError::DurationChanged);
    }
    let metadata = metadata_from_tagged_file(&verified);
    verify_patch(&metadata, patch)?;

    Ok(StagedTagUpdate {
        path: Some(staged.to_path_buf()),
        metadata,
    })
}

fn copy_new_file(source: &Path, staged: &Path) -> Result<(), MetadataError> {
    let input = File::open(source).map_err(|source| MetadataError::Io {
        action: "open metadata source",
        source,
    })?;
    let output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(staged)
        .map_err(|source| MetadataError::Io {
            action: "create staged metadata file",
            source,
        })?;
    let mut reader = BufReader::new(input);
    let mut writer = BufWriter::new(output);
    io::copy(&mut reader, &mut writer).map_err(|source| MetadataError::Io {
        action: "copy metadata source to staging",
        source,
    })?;
    writer.flush().map_err(|source| MetadataError::Io {
        action: "flush staged metadata file",
        source,
    })?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|source| MetadataError::Io {
            action: "synchronize staged metadata file",
            source,
        })
}

fn read_tagged_file(path: &Path) -> Result<TaggedFile, MetadataError> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension == "wma" {
        return Err(MetadataError::UnsupportedFormat {
            extension: ".wma (handled by the FFmpeg adapter)".to_owned(),
        });
    }

    configure_lofty();
    Probe::open(path)
        .map_err(|error| MetadataError::Parse(error.to_string()))?
        .options(parse_options())
        .guess_file_type()
        .map_err(|source| MetadataError::Io {
            action: "probe audio metadata format",
            source,
        })?
        .read()
        .map_err(|error| MetadataError::Parse(error.to_string()))
}

fn configure_lofty() {
    apply_global_options(
        GlobalOptions::new()
            .allocation_limit(MAX_TAG_ITEM_BYTES)
            .preserve_format_specific_items(true)
            .use_custom_resolvers(false),
    );
}

fn parse_options() -> ParseOptions {
    ParseOptions::new()
        .read_properties(true)
        .read_tags(true)
        .read_cover_art(true)
}

fn write_options() -> WriteOptions {
    WriteOptions::new().remove_others(false)
}

fn metadata_from_tagged_file(tagged_file: &TaggedFile) -> AudioMetadata {
    let tag = tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag());
    AudioMetadata {
        title: read_text(tag, ItemKey::TrackTitle),
        artist: read_text(tag, ItemKey::TrackArtist),
        album_artist: read_text(tag, ItemKey::AlbumArtist),
        album: read_text(tag, ItemKey::AlbumTitle),
        track_no: read_number(tag, &[ItemKey::TrackNumber]),
        disc_no: read_number(tag, &[ItemKey::DiscNumber]),
        year: read_number(tag, &[ItemKey::RecordingDate, ItemKey::Year]),
        genre: read_text(tag, ItemKey::Genre),
        bpm: read_number(tag, &[ItemKey::IntegerBpm, ItemKey::Bpm]),
        duration: tagged_file.properties().duration(),
        artwork: tag.and_then(read_artwork),
    }
}

fn read_text(tag: Option<&Tag>, key: ItemKey) -> String {
    tag.and_then(|tag| tag.get_string(key))
        .unwrap_or_default()
        .to_owned()
}

fn read_number(tag: Option<&Tag>, keys: &[ItemKey]) -> Option<u32> {
    keys.iter()
        .find_map(|key| tag.and_then(|tag| tag.get_string(*key)))
        .and_then(coerce_number)
}

fn coerce_number(value: &str) -> Option<u32> {
    let head = value
        .trim()
        .split('/')
        .next()
        .unwrap_or_default()
        .split('-')
        .next()
        .unwrap_or_default()
        .trim();
    if head.is_empty() {
        None
    } else {
        head.parse().ok()
    }
}

fn read_artwork(tag: &Tag) -> Option<EmbeddedArtwork> {
    let pictures = tag.pictures();
    let picture = pictures
        .iter()
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first())?;
    Some(EmbeddedArtwork {
        bytes: picture.data().to_vec(),
        mime_type: picture
            .mime_type()
            .map_or("application/octet-stream", |mime| mime.as_str())
            .to_owned(),
    })
}

fn apply_generic_patch(
    tagged_file: &mut TaggedFile,
    patch: &TagPatch,
) -> Result<(), MetadataError> {
    let tag_type = tagged_file.primary_tag_type();
    if !tagged_file.tag_support(tag_type).is_writable() {
        return Err(MetadataError::UnsupportedFormat {
            extension: format!("{:?}", tagged_file.file_type()),
        });
    }
    if tagged_file.primary_tag().is_none() {
        tagged_file.insert_tag(Tag::new(tag_type));
    }
    let tag = tagged_file
        .primary_tag_mut()
        .ok_or_else(|| MetadataError::Write("primary tag was not created".to_owned()))?;
    for (&field, value) in &patch.changes {
        apply_change(tag, tag_type, field, value.as_ref());
    }
    Ok(())
}

fn apply_flac_patch(path: &Path, patch: &TagPatch) -> Result<(), MetadataError> {
    configure_lofty();
    let mut reader = open_media_reader(path)?;
    let mut media = FlacFile::read_from(&mut reader, parse_options())
        .map_err(|error| MetadataError::Parse(error.to_string()))?;
    if media.vorbis_comments().is_none() {
        media.set_vorbis_comments(VorbisComments::new());
    }
    let comments = media
        .vorbis_comments_mut()
        .ok_or_else(|| MetadataError::Write("FLAC comments were not created".to_owned()))?;
    apply_vorbis_patch(comments, patch);
    media
        .save_to_path(path, write_options())
        .map_err(|error| MetadataError::Write(error.to_string()))
}

fn apply_vorbis_file_patch(path: &Path, patch: &TagPatch) -> Result<(), MetadataError> {
    configure_lofty();
    let mut reader = open_media_reader(path)?;
    let mut media = VorbisFile::read_from(&mut reader, parse_options())
        .map_err(|error| MetadataError::Parse(error.to_string()))?;
    apply_vorbis_patch(media.vorbis_comments_mut(), patch);
    media
        .save_to_path(path, write_options())
        .map_err(|error| MetadataError::Write(error.to_string()))
}

fn apply_opus_patch(path: &Path, patch: &TagPatch) -> Result<(), MetadataError> {
    configure_lofty();
    let mut reader = open_media_reader(path)?;
    let mut media = OpusFile::read_from(&mut reader, parse_options())
        .map_err(|error| MetadataError::Parse(error.to_string()))?;
    apply_vorbis_patch(media.vorbis_comments_mut(), patch);
    media
        .save_to_path(path, write_options())
        .map_err(|error| MetadataError::Write(error.to_string()))
}

fn open_media_reader(path: &Path) -> Result<BufReader<File>, MetadataError> {
    File::open(path)
        .map(BufReader::new)
        .map_err(|source| MetadataError::Io {
            action: "open staged metadata file",
            source,
        })
}

fn apply_vorbis_patch(tag: &mut VorbisComments, patch: &TagPatch) {
    for (&field, value) in &patch.changes {
        let key = vorbis_key(field);
        let _ = tag.remove(key);
        if field == TagField::Year {
            let _ = tag.remove("YEAR");
        }
        let Some(value) = value else {
            continue;
        };
        let text = match value {
            TagValue::Text(value) => value.clone(),
            TagValue::Number(value) => value.to_string(),
        };
        tag.insert(key.to_owned(), text);
    }
}

const fn vorbis_key(field: TagField) -> &'static str {
    match field {
        TagField::Title => "TITLE",
        TagField::Artist => "ARTIST",
        TagField::AlbumArtist => "ALBUMARTIST",
        TagField::Album => "ALBUM",
        TagField::TrackNumber => "TRACKNUMBER",
        TagField::DiscNumber => "DISCNUMBER",
        TagField::Year => "DATE",
        TagField::Genre => "GENRE",
        TagField::Bpm => "BPM",
    }
}

fn apply_change(tag: &mut Tag, tag_type: TagType, field: TagField, value: Option<&TagValue>) {
    let keys = item_keys(field, tag_type);
    for key in &keys {
        tag.remove_key(*key);
    }
    let Some(value) = value else {
        return;
    };
    let text = match value {
        TagValue::Text(value) => value.clone(),
        TagValue::Number(value) => value.to_string(),
    };
    if let Some(key) = keys.into_iter().next() {
        tag.insert_text(key, text);
    }
}

fn item_keys(field: TagField, tag_type: TagType) -> Vec<ItemKey> {
    match field {
        TagField::Title => vec![ItemKey::TrackTitle],
        TagField::Artist => vec![ItemKey::TrackArtist],
        TagField::AlbumArtist => vec![ItemKey::AlbumArtist, ItemKey::AlbumArtists],
        TagField::Album => vec![ItemKey::AlbumTitle],
        TagField::TrackNumber => vec![ItemKey::TrackNumber],
        TagField::DiscNumber => vec![ItemKey::DiscNumber],
        TagField::Year => vec![ItemKey::RecordingDate, ItemKey::Year],
        TagField::Genre => vec![ItemKey::Genre],
        TagField::Bpm if matches!(tag_type, TagType::VorbisComments) => {
            vec![ItemKey::Bpm, ItemKey::IntegerBpm]
        }
        TagField::Bpm => vec![ItemKey::IntegerBpm, ItemKey::Bpm],
    }
}

fn verify_patch(metadata: &AudioMetadata, patch: &TagPatch) -> Result<(), MetadataError> {
    for (&field, expected) in &patch.changes {
        let matches = match (metadata_value(metadata, field), expected) {
            (None, None) => true,
            (Some(actual), Some(expected)) => &actual == expected,
            _ => false,
        };
        if !matches {
            return Err(MetadataError::Verification { field });
        }
    }
    Ok(())
}

fn metadata_value(metadata: &AudioMetadata, field: TagField) -> Option<TagValue> {
    match field {
        TagField::Title => non_empty(&metadata.title).map(TagValue::Text),
        TagField::Artist => non_empty(&metadata.artist).map(TagValue::Text),
        TagField::AlbumArtist => non_empty(&metadata.album_artist).map(TagValue::Text),
        TagField::Album => non_empty(&metadata.album).map(TagValue::Text),
        TagField::TrackNumber => metadata.track_no.map(TagValue::Number),
        TagField::DiscNumber => metadata.disc_no.map(TagValue::Number),
        TagField::Year => metadata.year.map(TagValue::Number),
        TagField::Genre => non_empty(&metadata.genre).map(TagValue::Text),
        TagField::Bpm => metadata.bpm.map(TagValue::Number),
    }
}

fn non_empty(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

const fn is_text_field(field: TagField) -> bool {
    matches!(
        field,
        TagField::Title
            | TagField::Artist
            | TagField::AlbumArtist
            | TagField::Album
            | TagField::Genre
    )
}

const fn is_numeric_field(field: TagField) -> bool {
    matches!(
        field,
        TagField::TrackNumber | TagField::DiscNumber | TagField::Year | TagField::Bpm
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error;
    use std::fs;

    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::Deserialize;

    use super::{TagField, TagPatch, read_audio_metadata, stage_tag_update};

    const METADATA_EXAMPLES: &str =
        include_str!("../../../contracts/reference/v1/metadata.examples.json");

    #[derive(Deserialize)]
    struct MetadataFixture {
        cases: Vec<MetadataCase>,
        covered_extensions: Vec<String>,
        pending_ffmpeg_extensions: Vec<String>,
    }

    #[derive(Deserialize)]
    struct MetadataCase {
        canonical: CanonicalMetadata,
        extension: String,
        preservation_markers: Vec<String>,
        source_base64: String,
    }

    #[derive(Deserialize)]
    struct CanonicalMetadata {
        title: String,
        artist: String,
        album_artist: String,
        album: String,
        track_no: u32,
        disc_no: u32,
        year: u32,
        genre: String,
        bpm: u32,
    }

    #[test]
    fn reads_python_metadata_corpus_and_stages_verified_updates() -> Result<(), Box<dyn Error>> {
        let fixture: MetadataFixture = serde_json::from_str(METADATA_EXAMPLES)?;
        assert_eq!(
            fixture
                .covered_extensions
                .into_iter()
                .collect::<BTreeSet<_>>(),
            [".aiff", ".flac", ".mp3", ".ogg", ".wav"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
        assert_eq!(
            fixture
                .pending_ffmpeg_extensions
                .into_iter()
                .collect::<BTreeSet<_>>(),
            [".aac", ".m4a", ".opus", ".wma"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );

        for case in fixture.cases {
            let temp = tempfile::tempdir()?;
            let source = temp.path().join(format!("source{}", case.extension));
            let staged = temp.path().join(format!("staged{}", case.extension));
            let source_bytes = STANDARD.decode(&case.source_base64)?;
            fs::write(&source, &source_bytes)?;

            let read = read_audio_metadata(&source)?;
            assert_canonical(&read, &case.canonical);
            if case.extension != ".ogg" {
                assert!(read.artwork.is_some(), "{} artwork", case.extension);
            }

            let patch = replacement_patch()?;
            let staged_update = stage_tag_update(&source, &staged, &patch)?;
            assert_eq!(
                fs::read(&source)?,
                source_bytes,
                "{} source",
                case.extension
            );
            assert_eq!(staged_update.metadata().title, "Rust Round Trip");
            assert_eq!(staged_update.metadata().artist, "Rust Artist");
            assert_eq!(staged_update.metadata().album_artist, "Rust Album Artist");
            assert_eq!(staged_update.metadata().album, "Rust Album");
            assert_eq!(staged_update.metadata().track_no, Some(8));
            assert_eq!(staged_update.metadata().disc_no, Some(3));
            assert_eq!(staged_update.metadata().year, Some(2026));
            assert_eq!(staged_update.metadata().genre, "Score");
            assert_eq!(staged_update.metadata().bpm, Some(140));
            assert_eq!(staged_update.metadata().artwork, read.artwork);

            let staged_bytes = fs::read(staged_update.path()?)?;
            for marker in case.preservation_markers {
                assert!(
                    contains_ascii_case_insensitive(&staged_bytes, marker.as_bytes()),
                    "{} lost preservation marker {marker}",
                    case.extension
                );
            }
            drop(staged_update);
            assert!(!staged.exists());

            let cleared = temp.path().join(format!("cleared{}", case.extension));
            let clear_update = stage_tag_update(&source, &cleared, &clearing_patch()?)?;
            assert_eq!(clear_update.metadata().title, "");
            assert_eq!(clear_update.metadata().artist, "");
            assert_eq!(clear_update.metadata().album_artist, "");
            assert_eq!(clear_update.metadata().album, "");
            assert_eq!(clear_update.metadata().track_no, None);
            assert_eq!(clear_update.metadata().disc_no, None);
            assert_eq!(clear_update.metadata().year, None);
            assert_eq!(clear_update.metadata().genre, "");
            assert_eq!(clear_update.metadata().bpm, None);
            assert_eq!(clear_update.metadata().artwork, read.artwork);
            drop(clear_update);
            assert!(!cleared.exists());
        }
        Ok(())
    }

    #[test]
    fn rejects_ambiguous_or_invalid_patches_without_touching_files() -> Result<(), Box<dyn Error>> {
        let mut patch = TagPatch::new();
        patch.insert_text(TagField::Title, "first")?;
        assert!(patch.insert_text(TagField::Title, "second").is_err());
        assert!(patch.insert_text(TagField::Bpm, "fast").is_err());
        assert!(patch.insert_number(TagField::Artist, 12).is_err());
        assert!(patch.insert_number(TagField::Bpm, 10_000).is_err());
        assert!(patch.insert_text(TagField::Genre, "x".repeat(129)).is_err());

        let temp = tempfile::tempdir()?;
        let path = temp.path().join("track.wma");
        fs::write(&path, b"synthetic")?;
        assert!(read_audio_metadata(&path).is_err());

        let malformed = temp.path().join("malformed.mp3");
        let staged = temp.path().join("malformed-staged.mp3");
        fs::write(&malformed, b"not an MPEG stream")?;
        assert!(stage_tag_update(&malformed, &staged, &patch).is_err());
        assert!(!staged.exists());
        assert_eq!(fs::read(malformed)?, b"not an MPEG stream");
        Ok(())
    }

    fn replacement_patch() -> Result<TagPatch, Box<dyn Error>> {
        let mut patch = TagPatch::new();
        patch.insert_text(TagField::Title, "Rust Round Trip")?;
        patch.insert_text(TagField::Artist, "Rust Artist")?;
        patch.insert_text(TagField::AlbumArtist, "Rust Album Artist")?;
        patch.insert_text(TagField::Album, "Rust Album")?;
        patch.insert_number(TagField::TrackNumber, 8)?;
        patch.insert_number(TagField::DiscNumber, 3)?;
        patch.insert_number(TagField::Year, 2026)?;
        patch.insert_text(TagField::Genre, "Score")?;
        patch.insert_number(TagField::Bpm, 140)?;
        Ok(patch)
    }

    fn clearing_patch() -> Result<TagPatch, Box<dyn Error>> {
        let mut patch = TagPatch::new();
        for field in [
            TagField::Title,
            TagField::Artist,
            TagField::AlbumArtist,
            TagField::Album,
            TagField::TrackNumber,
            TagField::DiscNumber,
            TagField::Year,
            TagField::Genre,
            TagField::Bpm,
        ] {
            patch.clear(field)?;
        }
        Ok(patch)
    }

    fn assert_canonical(actual: &super::AudioMetadata, expected: &CanonicalMetadata) {
        assert_eq!(actual.title, expected.title);
        assert_eq!(actual.artist, expected.artist);
        assert_eq!(actual.album_artist, expected.album_artist);
        assert_eq!(actual.album, expected.album);
        assert_eq!(actual.track_no, Some(expected.track_no));
        assert_eq!(actual.disc_no, Some(expected.disc_no));
        assert_eq!(actual.year, Some(expected.year));
        assert_eq!(actual.genre, expected.genre);
        assert_eq!(actual.bpm, Some(expected.bpm));
    }

    fn contains_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
        !needle.is_empty()
            && haystack
                .windows(needle.len())
                .any(|window| window.eq_ignore_ascii_case(needle))
    }
}
