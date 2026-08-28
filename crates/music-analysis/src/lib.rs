#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![forbid(unsafe_code)]

//! Streaming signal analysis and bounded voice-inference adapters.
//!
//! CPU work runs only on the dedicated fixed analysis executor; model weights
//! remain optional, checksum-pinned, operator supplied, and non-fatal.

mod context;
mod executor;
mod signal;

pub use context::{
    AudioContextAnalyzer, AudioContextDocument, AudioContextError, AudioContextPerformance,
    FfmpegContextAnalyzer,
};
pub use executor::{AnalysisExecutor, AnalysisExecutorError};
pub use signal::{
    AudioSignalAnalyzer, AudioSignalError, AudioSignalMeasurements, AudioSignalProfile,
    FfmpegSignalAnalyzer,
};
