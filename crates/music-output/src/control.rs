use std::net::IpAddr;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderName, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};

use crate::runtime::OutputRuntime;

const MAX_CONTROL_BODY_BYTES: usize = 16 * 1024;
const CONTROL_TOKEN_HEADER: &str = "x-control-token";

#[derive(Debug, Clone)]
struct ControlState {
    runtime: Arc<OutputRuntime>,
    token: Option<Arc<str>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlUpdate {
    on: Option<bool>,
    volume: Option<f64>,
}

#[derive(Debug, Serialize)]
struct ControlErrorBody {
    error: &'static str,
}

#[derive(Debug)]
struct ControlError {
    status: StatusCode,
    message: &'static str,
}

impl IntoResponse for ControlError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ControlErrorBody {
                error: self.message,
            }),
        )
            .into_response()
    }
}

pub fn control_router(
    runtime: Arc<OutputRuntime>,
    token: Option<String>,
    expose_cors: bool,
) -> Router {
    let state = ControlState {
        runtime,
        token: token.map(Arc::from),
    };
    let mut router = Router::new()
        .route(
            "/control",
            get(control_status).post(update_control).options(preflight),
        )
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(MAX_CONTROL_BODY_BYTES))
        .with_state(state);
    if expose_cors {
        let token_header = HeaderName::from_static(CONTROL_TOKEN_HEADER);
        router = router.layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
                .allow_headers([axum::http::header::CONTENT_TYPE, token_header]),
        );
    }
    router
}

#[must_use]
pub fn bind_is_loopback(bind: &str) -> bool {
    bind.eq_ignore_ascii_case("localhost")
        || bind
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

async fn control_status(
    State(state): State<ControlState>,
    headers: HeaderMap,
) -> Result<Json<crate::runtime::ControlStatus>, ControlError> {
    authorize(&headers, state.token.as_deref())?;
    Ok(Json(state.runtime.control_status().await))
}

async fn update_control(
    State(state): State<ControlState>,
    headers: HeaderMap,
    payload: Result<Json<ControlUpdate>, JsonRejection>,
) -> Result<Json<crate::runtime::ControlStatus>, ControlError> {
    authorize(&headers, state.token.as_deref())?;
    let Json(update) = payload.map_err(|rejection| ControlError {
        status: rejection.status(),
        message: if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            "request body too large"
        } else {
            "invalid JSON"
        },
    })?;
    if update.volume.is_some_and(|volume| !volume.is_finite()) {
        return Err(ControlError {
            status: StatusCode::BAD_REQUEST,
            message: "volume must be a finite number",
        });
    }
    state
        .runtime
        .set_local(update.on, update.volume)
        .await
        .map_err(|_| ControlError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "audio output unavailable",
        })?;
    Ok(Json(state.runtime.control_status().await))
}

async fn preflight() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn not_found() -> ControlError {
    ControlError {
        status: StatusCode::NOT_FOUND,
        message: "not found",
    }
}

fn authorize(headers: &HeaderMap, expected: Option<&str>) -> Result<(), ControlError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let supplied = headers
        .get(CONTROL_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if constant_time_equal(supplied.as_bytes(), expected.as_bytes()) {
        Ok(())
    } else {
        Err(ControlError {
            status: StatusCode::UNAUTHORIZED,
            message: "unauthorized",
        })
    }
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let maximum = left.len().max(right.len());
    let mut difference = left.len() ^ right.len();
    for index in 0..maximum {
        difference |= usize::from(left.get(index).copied().unwrap_or(0))
            ^ usize::from(right.get(index).copied().unwrap_or(0));
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_loopback_bindings() {
        assert!(bind_is_loopback("127.0.0.1"));
        assert!(bind_is_loopback("::1"));
        assert!(bind_is_loopback("localhost"));
        assert!(!bind_is_loopback("0.0.0.0"));
        assert!(!bind_is_loopback("192.168.1.20"));
    }

    #[test]
    fn token_comparison_rejects_length_and_content_mismatches() {
        assert!(constant_time_equal(b"secret", b"secret"));
        assert!(!constant_time_equal(b"secret", b"secrex"));
        assert!(!constant_time_equal(b"secret", b"secret-longer"));
    }
}
