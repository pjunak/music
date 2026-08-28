use std::collections::BTreeMap;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use music_application::auth::SessionTouch;
use serde::Serialize;
use utoipa::ToSchema;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{AnyOfBuilder, ObjectBuilder, Schema, Type};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::current_session;
use crate::error::{ApiError, openapi_integer, openapi_number};
use crate::http::HttpState;

#[derive(Debug, Clone, Default, PartialEq, Serialize, ToSchema)]
struct LoaderStatus {
    #[schema(required = true, schema_with = nullable_number_schema)]
    last_load_at: Option<f64>,
    loaded_ids: Vec<String>,
    #[schema(schema_with = string_map_schema)]
    errors: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
struct DiagnosticsResponse {
    #[schema(schema_with = openapi_integer)]
    track_count: i64,
    #[schema(required = true, schema_with = nullable_number_schema)]
    last_scan_at: Option<f64>,
    modes: LoaderStatus,
    #[schema(schema_with = openapi_integer)]
    connected_device_count: i64,
    #[schema(schema_with = openapi_integer)]
    state_revision: i64,
}

pub(crate) fn diagnostics_router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default().routes(routes!(get_diagnostics))
}

#[utoipa::path(
    get,
    path = "/diagnostics",
    responses((status = 200, description = "Successful Response", body = DiagnosticsResponse)),
    tag = "diagnostics"
)]
async fn get_diagnostics(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<DiagnosticsResponse>, ApiError> {
    let _current = current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let library = state
        .library
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?;
    let playback = state
        .playback
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?;
    let library_status = library.coordinator.status();
    let snapshot = playback.snapshot().await.map_err(|error| {
        tracing::error!(error = %error, "diagnostics playback snapshot failed");
        ApiError::internal()
    })?;

    Ok(Json(DiagnosticsResponse {
        track_count: checked_count(library_status.discovered_tracks)?,
        last_scan_at: library_status
            .last_scan_at_unix_seconds
            .map(|timestamp| timestamp as f64),
        // The mode coordinator will replace this truthful not-yet-loaded
        // projection when Phase 6 publishes its immutable catalog status.
        modes: LoaderStatus::default(),
        connected_device_count: checked_count(snapshot.connected_clients.len())?,
        state_revision: checked_count(snapshot.state.publication_revision)?,
    }))
}

fn checked_count(value: impl TryInto<i64>) -> Result<i64, ApiError> {
    value.try_into().map_err(|_| ApiError::internal())
}

fn nullable_number_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(openapi_number())
            .item(ObjectBuilder::new().schema_type(Type::Null).build())
            .build(),
    )
    .into()
}

fn string_map_schema() -> RefOr<Schema> {
    Schema::Object(
        ObjectBuilder::new()
            .schema_type(Type::Object)
            .additional_properties(Some(ObjectBuilder::new().schema_type(Type::String)))
            .build(),
    )
    .into()
}
