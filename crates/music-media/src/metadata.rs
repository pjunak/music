use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use lofty::config::{GlobalOptions, ParseOptions, WriteOptions, apply_global_options};
use lofty::file::{AudioFile, TaggedFile, TaggedFileExt};
use lofty::flac::FlacFile;
use lofty::mp4::{Atom, AtomData, AtomIdent, Ilst, Mp4File};
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::{OpusFile, VorbisFile};
use lofty::picture::PictureType;
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey, Tag, TagExt, TagType};

mod asf;
mod ffmpeg;

pub use ffmpeg::FfmpegTools;

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

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MetadataWriteCapability {
    Native,
    Ffmpeg,
    ReadOnly,
    Unsupported,
}

pub fn metadata_write_capability(path: &Path) -> MetadataWriteCapability {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if extension.eq_ignore_ascii_case("aac") {
        MetadataWriteCapability::ReadOnly
    } else if extension.eq_ignore_ascii_case("wma") {
        MetadataWriteCapability::Ffmpeg
    } else if ["aif", "aiff", "flac", "m4a", "mp3", "ogg", "opus", "wav"]
        .iter()
        .any(|supported| extension.eq_ignore_ascii_case(supported))
    {
        MetadataWriteCapability::Native
    } else {
        MetadataWriteCapability::Unsupported
    }
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
        if value.contains('\0') {
            return Err(MetadataError::InvalidTextCharacter { field });
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
    fn new(path: PathBuf, metadata: AudioMetadata) -> Self {
        Self {
            path: Some(path),
            metadata,
        }
    }

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

#[derive(Debug, Clone)]
pub struct MetadataAdapter {
    ffmpeg: Option<FfmpegTools>,
}

impl MetadataAdapter {
    pub const fn native_only() -> Self {
        Self { ffmpeg: None }
    }

    pub const fn with_ffmpeg(tools: FfmpegTools) -> Self {
        Self {
            ffmpeg: Some(tools),
        }
    }

    pub fn read(&self, path: &Path) -> Result<AudioMetadata, MetadataError> {
        if has_extension(path, "wma") || has_extension(path, "aac") {
            let tools = self
                .ffmpeg
                .as_ref()
                .ok_or(MetadataError::ExternalToolRequired {
                    extension: if has_extension(path, "wma") {
                        ".wma"
                    } else {
                        ".aac"
                    },
                    tool: "ffprobe",
                })?;
            if has_extension(path, "wma") {
                ffmpeg::read_wma_metadata(path, tools)
            } else {
                ffmpeg::read_aac_metadata(path, tools)
            }
        } else {
            read_audio_metadata(path)
        }
    }

    pub fn stage_update(
        &self,
        source: &Path,
        staged: &Path,
        patch: &TagPatch,
    ) -> Result<StagedTagUpdate, MetadataError> {
        if has_extension(source, "wma") {
            let tools = self
                .ffmpeg
                .as_ref()
                .ok_or(MetadataError::ExternalToolRequired {
                    extension: ".wma",
                    tool: "ffmpeg",
                })?;
            ffmpeg::stage_wma_tag_update(source, staged, patch, tools)
        } else {
            stage_tag_update(source, staged, patch)
        }
    }
}

impl Default for MetadataAdapter {
    fn default() -> Self {
        Self::native_only()
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
    ExternalToolRequired {
        extension: &'static str,
        tool: &'static str,
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
    InvalidTextCharacter {
        field: TagField,
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
    CodecChanged,
    MissingAudioStream,
    InvalidAsf(String),
    ProcessTimedOut {
        tool: &'static str,
        timeout: Duration,
    },
    ProcessFailed {
        tool: &'static str,
        code: Option<i32>,
    },
    ProcessOutputTruncated {
        tool: &'static str,
    },
}

impl Display for MetadataError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFormat { extension } => {
                write!(formatter, "unsupported metadata format: {extension}")
            }
            Self::ExternalToolRequired { extension, tool } => {
                write!(formatter, "{extension} metadata requires {tool}")
            }
            Self::EmptyPatch => formatter.write_str("metadata patch contains no changes"),
            Self::DuplicateField { field } => {
                write!(formatter, "duplicate metadata field: {field}")
            }
            Self::WrongValueType { field } => write!(formatter, "wrong value type for {field}"),
            Self::TextTooLong { field, maximum } => {
                write!(formatter, "{field} exceeds {maximum} characters")
            }
            Self::InvalidTextCharacter { field } => {
                write!(formatter, "{field} contains a null character")
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
            Self::CodecChanged => {
                formatter.write_str("staged metadata changed the encoded audio stream")
            }
            Self::MissingAudioStream => formatter.write_str("media has no audio stream"),
            Self::InvalidAsf(message) => write!(formatter, "invalid ASF metadata: {message}"),
            Self::ProcessTimedOut { tool, timeout } => {
                write!(formatter, "{tool} exceeded its {timeout:?} deadline")
            }
            Self::ProcessFailed { tool, code } => {
                write!(formatter, "{tool} failed with exit code {code:?}")
            }
            Self::ProcessOutputTruncated { tool } => {
                write!(formatter, "{tool} exceeded its output limit")
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
    if has_extension(path, "aac") {
        return Err(MetadataError::ExternalToolRequired {
            extension: ".aac",
            tool: "ffprobe",
        });
    }
    if has_extension(path, "m4a") {
        return read_mp4_metadata(path);
    }
    let tagged_file = read_tagged_file(path)?;
    Ok(metadata_from_tagged_file(&tagged_file))
}

pub fn stage_tag_update(
    source: &Path,
    staged: &Path,
    patch: &TagPatch,
) -> Result<StagedTagUpdate, MetadataError> {
    validate_stage_request(source, staged, patch)?;
    if has_extension(source, "aac") {
        return Err(MetadataError::UnsupportedFormat {
            extension: ".aac metadata is read-only".to_owned(),
        });
    }

    let result = stage_tag_update_inner(source, staged, patch);
    if result.is_err() {
        let _ = fs::remove_file(staged);
    }
    result
}

fn validate_stage_request(
    source: &Path,
    staged: &Path,
    patch: &TagPatch,
) -> Result<(), MetadataError> {
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
    Ok(())
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
    let trailing_iff_bytes = iff_trailing_bytes(staged, original_type)?;
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
        lofty::file::FileType::Mp4 => {
            drop(before);
            apply_mp4_patch(staged, patch)?;
        }
        _ => {
            let mut tagged_file = before;
            apply_generic_patch(&mut tagged_file, patch)?;
            tagged_file
                .primary_tag()
                .ok_or_else(|| MetadataError::Write("primary tag was not created".to_owned()))?
                .save_to_path(staged, write_options())
                .map_err(|error| MetadataError::Write(error.to_string()))?;
        }
    }
    normalize_iff_stream_length(staged, original_type, trailing_iff_bytes)?;

    let verified = read_tagged_file(staged)?;
    if verified.file_type() != original_type {
        return Err(MetadataError::FormatChanged);
    }
    if verified.properties().duration() != original_duration {
        return Err(MetadataError::DurationChanged);
    }
    let metadata = if original_type == lofty::file::FileType::Mp4 {
        drop(verified);
        read_mp4_metadata(staged)?
    } else {
        metadata_from_tagged_file(&verified)
    };
    verify_patch(&metadata, patch)?;

    Ok(StagedTagUpdate::new(staged.to_path_buf(), metadata))
}

fn iff_trailing_bytes(path: &Path, file_type: lofty::file::FileType) -> Result<u64, MetadataError> {
    if !matches!(
        file_type,
        lofty::file::FileType::Wav | lofty::file::FileType::Aiff
    ) {
        return Ok(0);
    }
    let mut file = File::open(path).map_err(|source| MetadataError::Io {
        action: "open IFF metadata source",
        source,
    })?;
    let mut header = [0_u8; 12];
    file.read_exact(&mut header)
        .map_err(|source| MetadataError::Io {
            action: "read IFF metadata header",
            source,
        })?;
    let declared = match file_type {
        lofty::file::FileType::Wav if &header[..4] == b"RIFF" && &header[8..] == b"WAVE" => {
            u64::from(u32::from_le_bytes(header[4..8].try_into().map_err(
                |_| MetadataError::Parse("invalid WAV size header".to_owned()),
            )?))
        }
        lofty::file::FileType::Aiff if &header[..4] == b"FORM" => {
            u64::from(u32::from_be_bytes(header[4..8].try_into().map_err(
                |_| MetadataError::Parse("invalid AIFF size header".to_owned()),
            )?))
        }
        _ => return Err(MetadataError::Parse("invalid IFF header".to_owned())),
    }
    .checked_add(8)
    .ok_or_else(|| MetadataError::Parse("IFF stream size overflowed".to_owned()))?;
    let actual = file
        .metadata()
        .map_err(|source| MetadataError::Io {
            action: "inspect IFF metadata source",
            source,
        })?
        .len();
    actual
        .checked_sub(declared)
        .ok_or_else(|| MetadataError::Parse("IFF stream exceeds its file size".to_owned()))
}

fn normalize_iff_stream_length(
    path: &Path,
    file_type: lofty::file::FileType,
    trailing_bytes: u64,
) -> Result<(), MetadataError> {
    if !matches!(
        file_type,
        lofty::file::FileType::Wav | lofty::file::FileType::Aiff
    ) {
        return Ok(());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|source| MetadataError::Io {
            action: "open staged IFF metadata",
            source,
        })?;
    let stream_size = file
        .metadata()
        .map_err(|source| MetadataError::Io {
            action: "inspect staged IFF metadata",
            source,
        })?
        .len()
        .checked_sub(trailing_bytes)
        .and_then(|size| size.checked_sub(8))
        .and_then(|size| u32::try_from(size).ok())
        .ok_or_else(|| MetadataError::Write("IFF stream size is invalid".to_owned()))?;
    file.seek(SeekFrom::Start(4))
        .map_err(|source| MetadataError::Io {
            action: "seek staged IFF metadata",
            source,
        })?;
    let encoded = match file_type {
        lofty::file::FileType::Wav => stream_size.to_le_bytes(),
        lofty::file::FileType::Aiff => stream_size.to_be_bytes(),
        _ => return Ok(()),
    };
    file.write_all(&encoded)
        .and_then(|()| file.sync_all())
        .map_err(|source| MetadataError::Io {
            action: "synchronize staged IFF metadata",
            source,
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

fn has_extension(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
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
    Some(embedded_artwork(picture))
}

fn embedded_artwork(picture: &lofty::picture::Picture) -> EmbeddedArtwork {
    EmbeddedArtwork {
        bytes: picture.data().to_vec(),
        mime_type: picture
            .mime_type()
            .map_or("application/octet-stream", |mime| mime.as_str())
            .to_owned(),
    }
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

fn read_mp4_metadata(path: &Path) -> Result<AudioMetadata, MetadataError> {
    configure_lofty();
    let mut reader = open_media_reader(path)?;
    let media = Mp4File::read_from(&mut reader, parse_options())
        .map_err(|error| MetadataError::Parse(error.to_string()))?;
    Ok(metadata_from_mp4(&media))
}

fn metadata_from_mp4(media: &Mp4File) -> AudioMetadata {
    let tag = media.ilst();
    AudioMetadata {
        title: mp4_text(tag, AtomIdent::Fourcc(*b"\xA9nam")),
        artist: mp4_text(tag, AtomIdent::Fourcc(*b"\xA9ART")),
        album_artist: mp4_text(tag, AtomIdent::Fourcc(*b"aART")),
        album: mp4_text(tag, AtomIdent::Fourcc(*b"\xA9alb")),
        track_no: tag.and_then(Accessor::track),
        disc_no: tag.and_then(Accessor::disk),
        year: coerce_number(&mp4_text(tag, AtomIdent::Fourcc(*b"\xA9day"))),
        genre: mp4_text(tag, AtomIdent::Fourcc(*b"\xA9gen")),
        bpm: mp4_number(tag, AtomIdent::Fourcc(*b"tmpo")),
        duration: media.properties().duration(),
        artwork: tag.and_then(|tag| {
            tag.pictures()
                .and_then(|mut pictures| pictures.next().map(embedded_artwork))
        }),
    }
}

fn mp4_text(tag: Option<&Ilst>, ident: AtomIdent<'_>) -> String {
    tag.and_then(|tag| tag.get(&ident))
        .and_then(|atom| {
            atom.data().find_map(|data| match data {
                AtomData::UTF8(text) | AtomData::UTF16(text) => Some(text.clone()),
                _ => None,
            })
        })
        .unwrap_or_default()
}

fn mp4_number(tag: Option<&Ilst>, ident: AtomIdent<'_>) -> Option<u32> {
    tag.and_then(|tag| tag.get(&ident)).and_then(|atom| {
        atom.data().find_map(|data| match data {
            AtomData::SignedInteger(value) => u32::try_from(*value).ok(),
            AtomData::UnsignedInteger(value) => Some(*value),
            AtomData::UTF8(value) | AtomData::UTF16(value) => coerce_number(value),
            _ => None,
        })
    })
}

fn apply_mp4_patch(path: &Path, patch: &TagPatch) -> Result<(), MetadataError> {
    configure_lofty();
    let mut reader = open_media_reader(path)?;
    let mut media = Mp4File::read_from(&mut reader, parse_options())
        .map_err(|error| MetadataError::Parse(error.to_string()))?;
    if media.ilst().is_none() {
        media.set_ilst(Ilst::new());
    }
    let tag = media
        .ilst_mut()
        .ok_or_else(|| MetadataError::Write("MP4 ilst was not created".to_owned()))?;
    for (&field, value) in &patch.changes {
        match field {
            TagField::TrackNumber => apply_mp4_track(tag, value.as_ref()),
            TagField::DiscNumber => apply_mp4_disc(tag, value.as_ref()),
            TagField::Bpm => apply_mp4_bpm(tag, value.as_ref()),
            _ => apply_mp4_text(tag, field, value.as_ref()),
        }
    }
    media
        .save_to_path(path, write_options())
        .map_err(|error| MetadataError::Write(error.to_string()))
}

fn apply_mp4_track(tag: &mut Ilst, value: Option<&TagValue>) {
    match value {
        Some(TagValue::Number(value)) => tag.set_track(*value),
        _ => tag.remove_track(),
    }
}

fn apply_mp4_disc(tag: &mut Ilst, value: Option<&TagValue>) {
    match value {
        Some(TagValue::Number(value)) => tag.set_disk(*value),
        _ => tag.remove_disk(),
    }
}

fn apply_mp4_bpm(tag: &mut Ilst, value: Option<&TagValue>) {
    let ident = AtomIdent::Fourcc(*b"tmpo");
    let _ = tag.remove(&ident);
    if let Some(TagValue::Number(value)) = value {
        tag.replace_atom(Atom::new(
            ident,
            AtomData::SignedInteger(i32::try_from(*value).unwrap_or(i32::MAX)),
        ));
    }
}

fn apply_mp4_text(tag: &mut Ilst, field: TagField, value: Option<&TagValue>) {
    let ident = match field {
        TagField::Title => AtomIdent::Fourcc(*b"\xA9nam"),
        TagField::Artist => AtomIdent::Fourcc(*b"\xA9ART"),
        TagField::AlbumArtist => AtomIdent::Fourcc(*b"aART"),
        TagField::Album => AtomIdent::Fourcc(*b"\xA9alb"),
        TagField::Year => AtomIdent::Fourcc(*b"\xA9day"),
        TagField::Genre => AtomIdent::Fourcc(*b"\xA9gen"),
        TagField::TrackNumber | TagField::DiscNumber | TagField::Bpm => return,
    };
    let _ = tag.remove(&ident);
    if let Some(value) = value {
        tag.replace_atom(Atom::new(ident, AtomData::UTF8(tag_value_text(value))));
    }
}

fn tag_value_text(value: &TagValue) -> String {
    match value {
        TagValue::Text(value) => value.clone(),
        TagValue::Number(value) => value.to_string(),
    }
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
    use std::env;
    use std::error::Error;
    use std::fs;
    use std::path::PathBuf;

    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::Deserialize;

    use super::{
        FfmpegTools, MetadataAdapter, MetadataWriteCapability, TagField, TagPatch,
        metadata_write_capability, read_audio_metadata, stage_tag_update,
    };

    const METADATA_EXAMPLES: &str =
        include_str!("../../../contracts/reference/v1/metadata.examples.json");

    #[derive(Deserialize)]
    struct MetadataFixture {
        cases: Vec<MetadataCase>,
        covered_extensions: Vec<String>,
        read_only_extensions: Vec<String>,
        write_supported_extensions: Vec<String>,
    }

    #[derive(Deserialize)]
    struct MetadataCase {
        artwork_expected: bool,
        canonical: CanonicalMetadata,
        duration_millis: u128,
        extension: String,
        legacy_write_error: Option<String>,
        metadata_write_supported: bool,
        preservation_markers: Vec<String>,
        source_base64: String,
    }

    #[derive(Deserialize)]
    struct CanonicalMetadata {
        title: String,
        artist: String,
        album_artist: String,
        album: String,
        track_no: Option<u32>,
        disc_no: Option<u32>,
        year: Option<u32>,
        genre: String,
        bpm: Option<u32>,
    }

    #[test]
    fn reads_python_metadata_corpus_and_stages_verified_updates() -> Result<(), Box<dyn Error>> {
        let fixture: MetadataFixture = serde_json::from_str(METADATA_EXAMPLES)?;
        assert_eq!(
            fixture
                .covered_extensions
                .into_iter()
                .collect::<BTreeSet<_>>(),
            [
                ".aac", ".aiff", ".flac", ".m4a", ".mp3", ".ogg", ".opus", ".wav", ".wma",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        );
        assert_eq!(
            fixture
                .read_only_extensions
                .into_iter()
                .collect::<BTreeSet<_>>(),
            [".aac"].into_iter().map(str::to_owned).collect()
        );
        assert_eq!(fixture.write_supported_extensions.len(), 8);

        let ffmpeg_adapter = MetadataAdapter::with_ffmpeg(test_ffmpeg_tools());

        for case in fixture.cases {
            let temp = tempfile::tempdir()?;
            let source = temp.path().join(format!("source{}", case.extension));
            let staged = temp.path().join(format!("staged{}", case.extension));
            let source_bytes = STANDARD.decode(&case.source_base64)?;
            fs::write(&source, &source_bytes)?;

            let expected_capability = match case.extension.as_str() {
                ".aac" => MetadataWriteCapability::ReadOnly,
                ".wma" => MetadataWriteCapability::Ffmpeg,
                _ => MetadataWriteCapability::Native,
            };
            assert_eq!(metadata_write_capability(&source), expected_capability);

            let read = if matches!(case.extension.as_str(), ".aac" | ".wma") {
                ffmpeg_adapter.read(&source)?
            } else {
                read_audio_metadata(&source)?
            };
            assert_canonical(&read, &case.canonical, &case.extension);
            assert!(
                read.duration.as_millis().abs_diff(case.duration_millis) <= 2,
                "{} duration: {:?} versus {} ms",
                case.extension,
                read.duration,
                case.duration_millis
            );
            if case.artwork_expected {
                assert!(read.artwork.is_some(), "{} artwork", case.extension);
            } else {
                assert!(read.artwork.is_none(), "{} artwork", case.extension);
            }

            if !case.metadata_write_supported {
                assert_eq!(
                    case.legacy_write_error.as_deref(),
                    Some("mutagen.aac.AACError")
                );
                assert!(stage_tag_update(&source, &staged, &replacement_patch()?).is_err());
                assert_eq!(fs::read(&source)?, source_bytes);
                assert!(!staged.exists());
                continue;
            }

            let patch = replacement_patch()?;
            let staged_update = if case.extension == ".wma" {
                ffmpeg_adapter.stage_update(&source, &staged, &patch)?
            } else {
                stage_tag_update(&source, &staged, &patch)?
            };
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
                    contains_marker(&staged_bytes, &marker),
                    "{} lost preservation marker {marker}",
                    case.extension
                );
            }
            drop(staged_update);
            assert!(!staged.exists());

            let cleared = temp.path().join(format!("cleared{}", case.extension));
            let clear_update = if case.extension == ".wma" {
                ffmpeg_adapter.stage_update(&source, &cleared, &clearing_patch()?)?
            } else {
                stage_tag_update(&source, &cleared, &clearing_patch()?)?
            };
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
        assert!(patch.insert_text(TagField::Artist, "bad\0value").is_err());

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

    fn assert_canonical(
        actual: &super::AudioMetadata,
        expected: &CanonicalMetadata,
        extension: &str,
    ) {
        assert_eq!(actual.title, expected.title, "{extension} title");
        assert_eq!(actual.artist, expected.artist, "{extension} artist");
        assert_eq!(
            actual.album_artist, expected.album_artist,
            "{extension} album artist"
        );
        assert_eq!(actual.album, expected.album, "{extension} album");
        assert_eq!(actual.track_no, expected.track_no, "{extension} track");
        assert_eq!(actual.disc_no, expected.disc_no, "{extension} disc");
        assert_eq!(actual.year, expected.year, "{extension} year");
        assert_eq!(actual.genre, expected.genre, "{extension} genre");
        assert_eq!(actual.bpm, expected.bpm, "{extension} bpm");
    }

    fn contains_marker(haystack: &[u8], marker: &str) -> bool {
        let ascii = marker.as_bytes();
        let utf16: Vec<u8> = marker.encode_utf16().flat_map(u16::to_le_bytes).collect();
        contains_ascii_case_insensitive(haystack, ascii)
            || haystack.windows(utf16.len()).any(|window| window == utf16)
    }

    fn contains_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
        !needle.is_empty()
            && haystack
                .windows(needle.len())
                .any(|window| window.eq_ignore_ascii_case(needle))
    }

    fn test_ffmpeg_tools() -> FfmpegTools {
        let ffmpeg =
            env::var_os("MUSIC_TEST_FFMPEG").map_or_else(|| PathBuf::from("ffmpeg"), PathBuf::from);
        let ffprobe = env::var_os("MUSIC_TEST_FFPROBE")
            .map_or_else(|| PathBuf::from("ffprobe"), PathBuf::from);
        FfmpegTools::new(ffmpeg, ffprobe)
    }
}
