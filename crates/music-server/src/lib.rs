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
mod analysis;
mod assistant;
mod auth;
mod authoring;
mod blocking;
mod cleanup;
mod cleanup_enrichment;
mod config;
mod contracts;
mod devices;
mod diagnostics;
mod error;
mod health;
mod http;
mod jobs;
mod library;
mod model_jobs;
mod modes;
mod playback_projection;
mod playlist_evaluation_admin;
mod playlists;
mod provider_api;
mod provider_credentials;
mod provider_handlers;
mod provider_transport;
mod runtime;
mod sfx;
mod storage_admin;
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
pub use playlist_evaluation_admin::{
    ConfiguredPlaylistEvaluationError, evaluate_configured_playlist_suite,
};
pub use provider_credentials::{CredentialStoreError, load_configured_credential_vault};
pub use runtime::{AppRuntime, initialize_tracing};
pub use storage_admin::{ModeSeedOutcome, StorageInitializationOutcome, initialize_storage};
pub use supervisor::{CriticalFailure, CriticalTaskError, TaskSupervisor};

// Direct-session tests need a structurally valid user row, not fresh Argon2 work.
// Password hashing and verification stay covered by the storage and auth tests.
#[cfg(test)]
const TEST_PASSWORD_HASH: &str = "$argon2id$v=19$m=65536,t=3,p=4$cmV3cml0ZS1maXh0dXJlLXNhbHQ$tgdYDN8ijOk+7HZgF2oUm50+O8nOMKGggRmynxNu4ko";

/// Exercises the public authoring-import JSON shapes and semantic validators.
/// This is compiled only for the separate fuzzing workspace.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub fn exercise_authoring_import_parser(input: &[u8]) {
    authoring::exercise_document_parser(input);
}
