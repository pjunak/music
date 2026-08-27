#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![forbid(unsafe_code)]

//! Rooted paths, metadata, streaming, staged mutations, and FFmpeg adapters.
//!
//! Filesystem effects stay behind explicit plans and recovery journals; pure
//! cleanup or authoring analysis must not acquire write-capable dependencies.

mod yaml;

pub use yaml::{
    CueDocument, ModeDocument, PresetDocument, SoundboardDocument, YamlDocumentError,
    parse_cue_document, parse_mode_document, parse_preset_document, parse_soundboard_document,
    serialize_document,
};
