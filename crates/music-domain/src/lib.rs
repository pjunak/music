#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![forbid(unsafe_code)]

//! Pure domain types and deterministic rules.
//!
//! This crate must remain independent of async runtimes, databases, HTTP,
//! filesystems, processes, and public wire compatibility types.

mod library;
mod media_path;
mod playback;

pub use library::{IndexedTrack, LibraryGeneration, LibraryRecordError, TrackMetadata};

pub use media_path::{
    LibraryMedia, LibraryPath, MAX_MEDIA_PATH_BYTES, MediaPathError, RootedMediaPath, SfxMedia,
    SfxPath,
};

pub use playback::{
    AmbientState, ClockSample, CrossfadeType, DomainEvent, InterruptState, LoopMode, LoopingSfx,
    PersistenceIntent, PlaybackCommand, PlaybackError, PlaybackState, PositionReport, Reduction,
    ReductionContext, ShuffleMode, TrackId, UnitInterval, materialize_positions, reduce,
};
