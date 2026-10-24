//! Streaming copies: AAC in MP4 at a fixed bitrate that carry the source's
//! tags and cover, verified to read back the same library fields.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::mp4::{Ilst, Mp4File};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::TagExt;

use super::ffmpeg::{AacEncoding, encode_aac};
use super::{
    AudioMetadata, FfmpegTools, MetadataAdapter, MetadataError, MetadataWriteCapability, TagField,
    TagPatch, TagValue, apply_mp4_patch, configure_lofty, metadata_write_capability,
    open_media_reader, parse_options, read_mp4_metadata, read_tagged_file, write_options,
};

/// Bitrate of every streaming copy; close to transparent for AAC.
pub const STREAMING_BITRATE_KBPS: u32 = 192;
/// Sources at or below this rate gain too little from re-encoding, including
/// lossless files that compress unusually well.
const CONVERSION_THRESHOLD_KBPS: u32 = 256;
/// Higher source rates only spend the fixed bitrate on inaudible bandwidth.
const MAX_STREAMING_SAMPLE_RATE: u32 = 48_000;
/// AAC framing may shift the reported length by a few frames, never more.
const DURATION_TOLERANCE: Duration = Duration::from_secs(1);
/// Encoding must keep up with at least real time after a fixed allowance.
const ENCODE_ALLOWANCE: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum StreamingAssessment {
    /// Audio above the threshold rate, which a streaming copy makes much lighter.
    Worthwhile,
    /// Already a light lossy encoding.
    AlreadyLight,
    /// Its original could not keep receiving tag edits natively.
    Unsupported,
}

#[derive(Debug, Clone)]
pub struct StreamingEncoder {
    tools: FfmpegTools,
    metadata: MetadataAdapter,
}

impl StreamingEncoder {
    #[must_use]
    pub fn new(tools: FfmpegTools) -> Self {
        Self {
            metadata: MetadataAdapter::with_ffmpeg(tools.clone()),
            tools,
        }
    }

    pub fn assess(&self, source: &Path) -> Result<StreamingAssessment, MetadataError> {
        if metadata_write_capability(source) != MetadataWriteCapability::Native {
            return Ok(StreamingAssessment::Unsupported);
        }
        let file = read_tagged_file(source)?;
        let properties = file.properties();
        let lossless = matches!(
            file.file_type(),
            FileType::Flac | FileType::Wav | FileType::Aiff
        ) || (file.file_type() == FileType::Mp4 && properties.bit_depth().is_some());
        let heavy = properties
            .audio_bitrate()
            .map_or(lossless, |rate| rate > CONVERSION_THRESHOLD_KBPS);
        Ok(if heavy {
            StreamingAssessment::Worthwhile
        } else {
            StreamingAssessment::AlreadyLight
        })
    }

    /// Writes a tagged streaming copy of `source` to the new path `target`.
    /// Nothing is left at `target` when encoding or verification fails.
    pub fn encode(&self, source: &Path, target: &Path) -> Result<(), MetadataError> {
        if target.exists() {
            return Err(MetadataError::StagedPathExists {
                path: target.to_path_buf(),
            });
        }
        let result = self.encode_new(source, target);
        if result.is_err() {
            let _ = std::fs::remove_file(target);
        }
        result
    }

    fn encode_new(&self, source: &Path, target: &Path) -> Result<(), MetadataError> {
        let expected = self.metadata.read(source)?;
        let file = read_tagged_file(source)?;
        let properties = file.properties();
        encode_aac(
            source,
            target,
            &AacEncoding {
                bitrate_kbps: STREAMING_BITRATE_KBPS,
                sample_rate: properties
                    .sample_rate()
                    .filter(|rate| *rate > MAX_STREAMING_SAMPLE_RATE)
                    .map(|_| MAX_STREAMING_SAMPLE_RATE),
                downmix_to_stereo: properties.channels().is_some_and(|channels| channels > 2),
                timeout: ENCODE_ALLOWANCE.saturating_add(expected.duration),
            },
            &self.tools,
        )?;
        write_streaming_tags(file.primary_tag().cloned(), &expected, target)?;
        verify_streaming_copy(&expected, &read_mp4_metadata(target)?)
    }
}

/// Carries every tag lofty can map into MP4, makes the library's cover the
/// only picture, then writes the library fields exactly as the MP4 reader
/// reads them back. An untagged source leaves the encoder's own atoms alone:
/// lofty cannot read back an MP4 whose tag list it wrote empty.
fn write_streaming_tags(
    source_tag: Option<lofty::tag::Tag>,
    expected: &AudioMetadata,
    target: &Path,
) -> Result<(), MetadataError> {
    configure_lofty();
    let mut reader = open_media_reader(target)?;
    let mut media = Mp4File::read_from(&mut reader, parse_options())
        .map_err(|error| MetadataError::Parse(error.to_string()))?;
    drop(reader);
    let mut tag = source_tag.map(Ilst::from).unwrap_or_default();
    tag.remove_pictures();
    if let Some(artwork) = &expected.artwork {
        tag.insert_picture(
            Picture::unchecked(artwork.bytes.clone())
                .pic_type(PictureType::CoverFront)
                .mime_type(MimeType::from_str(&artwork.mime_type))
                .build(),
        );
    }
    if !tag.is_empty() {
        media.set_ilst(tag);
        media
            .save_to_path(target, write_options())
            .map_err(|error| MetadataError::Write(error.to_string()))?;
    }
    let fields = library_fields(expected);
    if fields.changes.is_empty() {
        return Ok(());
    }
    apply_mp4_patch(target, &fields)
}

/// The source's non-empty library fields; a fresh encode has none to clear.
fn library_fields(expected: &AudioMetadata) -> TagPatch {
    let text = |value: &str| (!value.is_empty()).then(|| TagValue::Text(value.to_owned()));
    let release_date = if expected.release_date.is_empty() {
        expected
            .year
            .map(|year| TagValue::Text(format!("{year:04}")))
    } else {
        text(&expected.release_date)
    };
    TagPatch {
        changes: BTreeMap::from([
            (TagField::Title, text(&expected.title)),
            (TagField::Artist, text(&expected.artist)),
            (TagField::AlbumArtist, text(&expected.album_artist)),
            (TagField::Album, text(&expected.album)),
            (TagField::ReleaseDate, release_date),
            (
                TagField::OriginalReleaseDate,
                text(&expected.original_release_date),
            ),
            (TagField::Composer, text(&expected.composer)),
            (TagField::Genre, text(&expected.genre)),
            (
                TagField::TrackNumber,
                expected.track_no.map(TagValue::Number),
            ),
            (TagField::DiscNumber, expected.disc_no.map(TagValue::Number)),
            (TagField::Bpm, expected.bpm.map(TagValue::Number)),
        ])
        .into_iter()
        .filter_map(|(field, value)| value.map(|value| (field, Some(value))))
        .collect(),
    }
}

fn verify_streaming_copy(
    expected: &AudioMetadata,
    actual: &AudioMetadata,
) -> Result<(), MetadataError> {
    let checks = [
        (TagField::Title, expected.title == actual.title),
        (TagField::Artist, expected.artist == actual.artist),
        (
            TagField::AlbumArtist,
            expected.album_artist == actual.album_artist,
        ),
        (TagField::Album, expected.album == actual.album),
        (
            TagField::ReleaseDate,
            expected.release_date == actual.release_date
                || (expected.release_date.is_empty()
                    && expected.year.map(|year| format!("{year:04}"))
                        == Some(actual.release_date.clone())),
        ),
        (
            TagField::OriginalReleaseDate,
            expected.original_release_date == actual.original_release_date,
        ),
        (TagField::Composer, expected.composer == actual.composer),
        (TagField::Genre, expected.genre == actual.genre),
        (TagField::TrackNumber, expected.track_no == actual.track_no),
        (TagField::DiscNumber, expected.disc_no == actual.disc_no),
        (TagField::Year, expected.year == actual.year),
        (TagField::Bpm, expected.bpm == actual.bpm),
    ];
    if let Some((field, _)) = checks.into_iter().find(|(_, matches)| !matches) {
        return Err(MetadataError::Verification { field });
    }
    if expected.duration.abs_diff(actual.duration) > DURATION_TOLERANCE {
        return Err(MetadataError::DurationChanged);
    }
    if expected.artwork.as_ref().map(|artwork| &artwork.bytes)
        != actual.artwork.as_ref().map(|artwork| &artwork.bytes)
    {
        return Err(MetadataError::Write(
            "streaming copy cover differs from the source".to_owned(),
        ));
    }
    Ok(())
}
