use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::io;
use std::time::Duration;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{
    AnyOfBuilder, Array, ArrayBuilder, KnownFormat, Object, ObjectBuilder, Schema, SchemaFormat,
    SchemaType, Type,
};

use crate::config::ConfigError;

#[derive(Debug)]
pub enum RuntimeError {
    Config(ConfigError),
    Authentication(music_application::auth::AuthServiceError),
    Storage(music_storage::StorageError),
    Playback(music_application::playback::PlaybackActorError),
    Library(music_application::library::LibraryCoordinatorError),
    MediaRoot(music_media::RootedPathError),
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
            Self::Authentication(error) => Display::fmt(error, formatter),
            Self::Storage(error) => Display::fmt(error, formatter),
            Self::Playback(error) => Display::fmt(error, formatter),
            Self::Library(error) => Display::fmt(error, formatter),
            Self::MediaRoot(error) => Display::fmt(error, formatter),
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
            Self::Authentication(source) => Some(source),
            Self::Storage(source) => Some(source),
            Self::Playback(source) => Some(source),
            Self::Library(source) => Some(source),
            Self::MediaRoot(source) => Some(source),
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

impl From<music_application::auth::AuthServiceError> for RuntimeError {
    fn from(error: music_application::auth::AuthServiceError) -> Self {
        Self::Authentication(error)
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

impl From<music_application::library::LibraryCoordinatorError> for RuntimeError {
    fn from(error: music_application::library::LibraryCoordinatorError) -> Self {
        Self::Library(error)
    }
}

impl From<music_media::RootedPathError> for RuntimeError {
    fn from(error: music_media::RootedPathError) -> Self {
        Self::MediaRoot(error)
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

#[derive(Debug, Clone, Eq, PartialEq, Serialize, ToSchema)]
pub struct PlainErrorBody {
    pub detail: &'static str,
}

/// FastAPI-compatible validation envelope retained as a stable HTTP contract.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, ToSchema)]
#[schema(as = HTTPValidationError)]
pub struct HttpValidationErrorBody {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detail: Vec<ValidationErrorDetail>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, ToSchema)]
#[schema(as = ValidationError)]
pub struct ValidationErrorDetail {
    #[schema(schema_with = validation_location_schema)]
    pub loc: Vec<Value>,
    pub msg: &'static str,
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(schema_with = any_value_schema)]
    pub input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(schema_with = generic_object_schema)]
    pub ctx: Option<Value>,
}

fn validation_location_schema() -> Array {
    ArrayBuilder::new()
        .items(
            AnyOfBuilder::new()
                .item(openapi_string())
                .item(openapi_integer()),
        )
        .build()
}

fn any_value_schema() -> Object {
    ObjectBuilder::new()
        .schema_type(SchemaType::AnyValue)
        .build()
}

fn generic_object_schema() -> Object {
    ObjectBuilder::new().schema_type(Type::Object).build()
}

pub(crate) fn openapi_integer() -> RefOr<Schema> {
    openapi_primitive(Type::Integer)
}

pub(crate) fn openapi_number() -> RefOr<Schema> {
    openapi_primitive(Type::Number)
}

pub(crate) fn openapi_nullable_integer() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(openapi_integer())
            .item(openapi_primitive(Type::Null))
            .build(),
    )
    .into()
}

pub(crate) fn openapi_datetime() -> RefOr<Schema> {
    Schema::Object(
        ObjectBuilder::new()
            .schema_type(Type::String)
            .format(Some(SchemaFormat::KnownFormat(KnownFormat::DateTime)))
            .build(),
    )
    .into()
}

pub(crate) fn openapi_nullable_datetime() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(openapi_datetime())
            .item(openapi_primitive(Type::Null))
            .build(),
    )
    .into()
}

pub(crate) fn openapi_nullable_string() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(openapi_string())
            .item(openapi_primitive(Type::Null))
            .build(),
    )
    .into()
}

fn openapi_string() -> RefOr<Schema> {
    openapi_primitive(Type::String)
}

fn openapi_primitive(kind: Type) -> RefOr<Schema> {
    Schema::Object(ObjectBuilder::new().schema_type(kind).build()).into()
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum ApiErrorPayload {
    Coded(PublicErrorDetail),
    Plain(&'static str),
    Validation,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ApiError {
    status: StatusCode,
    payload: ApiErrorPayload,
}

impl ApiError {
    #[must_use]
    pub const fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            payload: ApiErrorPayload::Coded(PublicErrorDetail {
                code: "not_found",
                message: "The requested resource was not found.",
            }),
        }
    }

    #[must_use]
    pub const fn service_unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            payload: ApiErrorPayload::Coded(PublicErrorDetail {
                code: "service_unavailable",
                message: "The service is not ready for this request.",
            }),
        }
    }

    #[must_use]
    pub const fn validation() -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            payload: ApiErrorPayload::Validation,
        }
    }

    #[must_use]
    pub const fn internal() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            payload: ApiErrorPayload::Coded(PublicErrorDetail {
                code: "internal_error",
                message: "The request could not be completed.",
            }),
        }
    }

    #[must_use]
    pub const fn unauthorized(detail: &'static str) -> Self {
        Self::plain(StatusCode::UNAUTHORIZED, detail)
    }

    #[must_use]
    pub const fn too_many_requests(detail: &'static str) -> Self {
        Self::plain(StatusCode::TOO_MANY_REQUESTS, detail)
    }

    #[must_use]
    pub const fn bad_request(detail: &'static str) -> Self {
        Self::plain(StatusCode::BAD_REQUEST, detail)
    }

    #[must_use]
    pub const fn payload_too_large(detail: &'static str) -> Self {
        Self::plain(StatusCode::PAYLOAD_TOO_LARGE, detail)
    }

    #[must_use]
    pub const fn plain_not_found(detail: &'static str) -> Self {
        Self::plain(StatusCode::NOT_FOUND, detail)
    }

    #[must_use]
    pub const fn gone(detail: &'static str) -> Self {
        Self::plain(StatusCode::GONE, detail)
    }

    #[must_use]
    pub const fn conflict(detail: &'static str) -> Self {
        Self::plain(StatusCode::CONFLICT, detail)
    }

    const fn plain(status: StatusCode, detail: &'static str) -> Self {
        Self {
            status,
            payload: ApiErrorPayload::Plain(detail),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self.payload {
            ApiErrorPayload::Coded(detail) => {
                (self.status, Json(PublicErrorBody { detail })).into_response()
            }
            ApiErrorPayload::Plain(detail) => {
                (self.status, Json(PlainErrorBody { detail })).into_response()
            }
            ApiErrorPayload::Validation => (
                self.status,
                Json(HttpValidationErrorBody {
                    detail: vec![ValidationErrorDetail {
                        loc: vec![Value::String("body".to_owned())],
                        msg: "The request parameters are invalid.",
                        kind: "value_error",
                        input: None,
                        ctx: None,
                    }],
                }),
            )
                .into_response(),
        }
    }
}
