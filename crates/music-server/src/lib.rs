#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![forbid(unsafe_code)]

//! Transport adapters and explicit application composition.
//!
//! The future `AppRuntime` is constructed once here. HTTP handlers translate
//! and authorize; they never become alternate owners of mutable application
//! state.

mod admin;
mod auth;
mod authoring;
mod cleanup;
mod config;
mod contracts;
mod devices;
mod diagnostics;
mod error;
mod health;
mod http;
mod library;
mod modes;
mod playback_projection;
mod playlists;
mod runtime;
mod sfx;
mod supervisor;
mod websocket;

pub use admin::{
    BackupError, RestoreOptions, RestoreOutcome, RestoreRecoveryOutcome,
    recover_interrupted_restore, restore_backup,
};
pub use config::{AppConfig, ConfigError, LogLevel};
pub use contracts::{
    ContractArtifact, ContractError, check_contracts, export_contracts, render_contracts,
};
pub use error::{
    ApiError, HttpValidationErrorBody, PlainErrorBody, PublicErrorBody, PublicErrorDetail,
    RuntimeError, ValidationErrorDetail,
};
pub use health::{ComponentStatus, HealthRegistry, ReadinessSnapshot, ReadinessStatus};
pub use http::CorrelationId;
pub use runtime::{AppRuntime, initialize_tracing};
pub use supervisor::{CriticalFailure, CriticalTaskError, TaskSupervisor};
