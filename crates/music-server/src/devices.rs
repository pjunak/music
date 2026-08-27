use std::collections::BTreeSet;
use std::sync::Arc;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use music_application::auth::SessionTouch;
use music_application::devices::{DeviceServiceError, RememberedDevice, RememberedDeviceService};
use music_storage::SqliteStorage;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::current_session;
use crate::error::{ApiError, HttpValidationErrorBody, openapi_nullable_string};
use crate::http::HttpState;

#[derive(Debug, Clone)]
pub(crate) struct RuntimeDevices {
    service: RememberedDeviceService<SqliteStorage>,
}

impl RuntimeDevices {
    #[must_use]
    pub(crate) fn new(storage: Arc<SqliteStorage>) -> Self {
        Self {
            service: RememberedDeviceService::new(storage),
        }
    }

    pub(crate) async fn find(
        &self,
        client_id: &str,
    ) -> Result<Option<RememberedDevice>, DeviceServiceError> {
        self.service.find(client_id).await
    }
}

#[derive(Debug, Deserialize, ToSchema)]
struct DevicePut {
    #[schema(min_length = 1, max_length = 128)]
    name: String,
    #[serde(default)]
    #[schema(default = false)]
    is_output: bool,
}

#[derive(Debug, Serialize, ToSchema)]
struct DeviceOut {
    client_id: String,
    name: String,
    is_output: bool,
    connected: bool,
    #[schema(schema_with = openapi_nullable_string)]
    added_at: Option<String>,
}

impl DeviceOut {
    fn from_device(device: RememberedDevice, connected: bool) -> Self {
        Self {
            client_id: device.client_id,
            name: device.name,
            is_output: device.is_output,
            connected,
            added_at: device.added_at,
        }
    }
}

pub(crate) fn device_router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default()
        .routes(routes!(list_devices))
        .routes(routes!(save_device))
        .routes(routes!(forget_device))
}

#[utoipa::path(
    get,
    path = "/devices",
    responses(
        (status = 200, description = "Successful Response", body = [DeviceOut])
    ),
    tag = "devices"
)]
async fn list_devices(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<DeviceOut>>, ApiError> {
    let _current = current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let devices = state
        .devices
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?;
    let connected = connected_ids(&state).await?;
    let rows = devices.service.list().await.map_err(device_internal)?;
    Ok(Json(
        rows.into_iter()
            .map(|device| {
                let is_connected = connected.contains(&device.client_id);
                DeviceOut::from_device(device, is_connected)
            })
            .collect(),
    ))
}

#[utoipa::path(
    put,
    path = "/devices/{client_id}",
    params(("client_id" = String, Path, description = "Stable client identity")),
    request_body = DevicePut,
    responses(
        (status = 200, description = "Successful Response", body = DeviceOut),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "devices"
)]
async fn save_device(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
    payload: Result<Json<DevicePut>, JsonRejection>,
) -> Result<Json<DeviceOut>, ApiError> {
    let _current = current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if !(1..=128).contains(&payload.name.chars().count()) {
        return Err(ApiError::validation());
    }
    let devices = state
        .devices
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?;
    let device = devices
        .service
        .save(&client_id, &payload.name, payload.is_output)
        .await
        .map_err(map_device_error)?;
    if let Some(playback) = &state.playback {
        playback
            .refresh_client_metadata(
                client_id.clone(),
                Some(device.name.clone()),
                device.is_output,
            )
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "could not refresh connected device metadata");
                ApiError::service_unavailable()
            })?;
    }
    let connected = connected_ids(&state).await?.contains(&client_id);
    Ok(Json(DeviceOut::from_device(device, connected)))
}

#[utoipa::path(
    delete,
    path = "/devices/{client_id}",
    params(("client_id" = String, Path, description = "Stable client identity")),
    responses(
        (status = 204, description = "Successful Response"),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "devices"
)]
async fn forget_device(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
) -> Result<Response, ApiError> {
    let _current = current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let devices = state
        .devices
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?;
    let deleted = devices
        .service
        .forget(&client_id)
        .await
        .map_err(map_device_error)?;
    if !deleted {
        return Err(ApiError::plain_not_found("device not found"));
    }
    if let Some(playback) = &state.playback {
        playback
            .refresh_client_metadata(client_id, None, false)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "could not clear connected device metadata");
                ApiError::service_unavailable()
            })?;
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn connected_ids(state: &HttpState) -> Result<BTreeSet<String>, ApiError> {
    let playback = state
        .playback
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?;
    playback
        .snapshot()
        .await
        .map(|publication| {
            publication
                .connected_clients
                .iter()
                .map(|client| client.client_id.clone())
                .collect()
        })
        .map_err(|_| ApiError::service_unavailable())
}

fn map_device_error(error: DeviceServiceError) -> ApiError {
    match error {
        DeviceServiceError::InvalidClientId | DeviceServiceError::InvalidName => {
            ApiError::validation()
        }
        error @ DeviceServiceError::Dependency { .. } => device_internal(error),
    }
}

fn device_internal(error: DeviceServiceError) -> ApiError {
    tracing::error!(error = %error, "remembered-device operation failed");
    ApiError::internal()
}
