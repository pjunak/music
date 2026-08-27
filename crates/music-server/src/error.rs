use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::io;
use std::time::Duration;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

use crate::config::ConfigError;

#[derive(Debug)]
pub enum RuntimeError {
    Config(ConfigError),
    Storage(music_storage::StorageError),
    Playback(music_application::playback::PlaybackActorError),
    Io {
        operation: &'static str,
        source: io::Error,
    },
    TracingInitialization,
    TaskAdmissionClosed,
    SupervisorPoisoned,
    CriticalTaskFailed {
        task: &'static str,
    },
    ShutdownTimedOut {
        timeout: Duration,
    },
}

impl RuntimeError {
    #[must_use]
    pub fn io(operation: &'static str, source: io::Error) -> Self {
        Self::Io { operation, source }
    }
}

impl Display for RuntimeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => Display::fmt(error, formatter),
            Self::Storage(error) => Display::fmt(error, formatter),
            Self::Playback(error) => Display::fmt(error, formatter),
            Self::Io { operation, .. } => write!(formatter, "failed to {operation}"),
            Self::TracingInitialization => {
                formatter.write_str("failed to initialize structured tracing")
            }
            Self::TaskAdmissionClosed => {
                formatter.write_str("runtime task admission is already closed")
            }
            Self::SupervisorPoisoned => {
                formatter.write_str("runtime task supervisor state is unavailable")
            }
            Self::CriticalTaskFailed { task } => {
                write!(formatter, "critical runtime task failed: {task}")
            }
            Self::ShutdownTimedOut { timeout } => {
                write!(formatter, "runtime shutdown exceeded {timeout:?}")
            }
        }
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Config(source) => Some(source),
            Self::Storage(source) => Some(source),
            Self::Playback(source) => Some(source),
            Self::Io { source, .. } => Some(source),
            Self::TracingInitialization
            | Self::TaskAdmissionClosed
            | Self::SupervisorPoisoned
            | Self::CriticalTaskFailed { .. }
            | Self::ShutdownTimedOut { .. } => None,
        }
    }
}

impl From<ConfigError> for RuntimeError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<music_storage::StorageError> for RuntimeError {
    fn from(error: music_storage::StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<music_application::playback::PlaybackActorError> for RuntimeError {
    fn from(error: music_application::playback::PlaybackActorError) -> Self {
        Self::Playback(error)
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, ToSchema)]
pub struct PublicErrorDetail {
    pub code: &'static str,
    pub message: &'static str,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, ToSchema)]
pub struct PublicErrorBody {
    pub detail: PublicErrorDetail,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ApiError {
    status: StatusCode,
    detail: PublicErrorDetail,
}

impl ApiError {
    #[must_use]
    pub const fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            detail: PublicErrorDetail {
                code: "not_found",
                message: "The requested resource was not found.",
            },
        }
    }

    #[must_use]
    pub const fn service_unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            detail: PublicErrorDetail {
                code: "service_unavailable",
                message: "The service is not ready for this request.",
            },
        }
    }

    #[must_use]
    pub const fn validation() -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            detail: PublicErrorDetail {
                code: "validation_error",
                message: "The request parameters are invalid.",
            },
        }
    }

    #[must_use]
    pub const fn internal() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            detail: PublicErrorDetail {
                code: "internal_error",
                message: "The request could not be completed.",
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(PublicErrorBody {
                detail: self.detail,
            }),
        )
            .into_response()
    }
}
