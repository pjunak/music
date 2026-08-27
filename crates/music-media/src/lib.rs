#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![forbid(unsafe_code)]

//! Rooted paths, metadata, streaming, staged mutations, and FFmpeg adapters.
//!
//! Filesystem effects stay behind explicit plans and recovery journals; pure
//! cleanup or authoring analysis must not acquire write-capable dependencies.

mod metadata;
mod path;
mod yaml;

pub use metadata::{
    AudioMetadata, FfmpegTools, MetadataAdapter, MetadataWriteCapability, StagedTagUpdate,
    TagField, TagPatch, TagValue, metadata_write_capability, read_audio_metadata, stage_tag_update,
};

pub use path::{LibraryRoot, MediaRoot, RootedPathError, SfxRoot};

pub use yaml::{
    CueDocument, ModeDocument, PresetDocument, SoundboardDocument, YamlDocumentError,
    parse_cue_document, parse_mode_document, parse_preset_document, parse_soundboard_document,
    serialize_document,
};
