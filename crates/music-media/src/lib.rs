#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![forbid(unsafe_code)]

//! Rooted paths, metadata, streaming, staged mutations, and FFmpeg adapters.
//!
//! Filesystem effects stay behind explicit plans and recovery journals; pure
//! cleanup or authoring analysis must not acquire write-capable dependencies.

mod delivery;
mod discovery;
mod metadata;
mod modes;
mod mutation;
mod path;
mod yaml;

pub use delivery::{
    CoverArt, MediaDeliveryError, read_library_cover_art, resolve_library_media_file,
};
pub use discovery::{
    FilesystemDiscoveryError, FilesystemLibraryDiscovery, LibraryDirectory, inspect_library_track,
    is_supported_library_path, list_library_directories,
};
pub use metadata::{
    AudioMetadata, FfmpegTools, MetadataAdapter, MetadataError, MetadataWriteCapability,
    StagedTagUpdate, TagField, TagPatch, TagValue, metadata_write_capability, read_audio_metadata,
    stage_tag_update,
};
pub use modes::FilesystemModeCatalogSource;
pub use mutation::{FilesystemLibraryMutations, library_upload_target_exists};

pub use path::{LibraryRoot, MediaRoot, RootedPathError, SfxRoot};

pub use music_application::modes::{CueDocument, ModeDocument, PresetDocument, SoundboardDocument};
pub use yaml::{
    YamlDocumentError, parse_cue_document, parse_mode_document, parse_preset_document,
    parse_soundboard_document, serialize_document,
};
