use std::net::SocketAddr;
use std::sync::Arc;

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{ConnectInfo, Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use music_application::assistant::{
    MODEL_ROLES, ModelConformanceStatus, ModelRoleUpdate, ModelRoleView, PROVIDER_ADAPTERS,
    PROVIDER_CAPABILITIES, ProviderConnectionCreate, ProviderConnectionPatch,
    ProviderConnectionView, ProviderCredentialSource, ProviderRepository, ProviderSecret,
    ProviderService, ProviderServiceError, ProviderServiceErrorKind, ProviderVerificationStatus,
    ThinkingMode,
};
use music_application::auth::{SessionTouch, UnixSeconds};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{AnyOfBuilder, ObjectBuilder, Schema, Type};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use zeroize::Zeroizing;

use crate::auth::{PasswordConfirmationError, current_session, format_rfc3339};
use crate::error::{ApiError, HttpValidationErrorBody, openapi_datetime};
use crate::http::HttpState;
use crate::provider_credentials::{
    CredentialStorageReset, CredentialStorageSource, CredentialStorageStatus, CredentialStoreError,
    RuntimeCredentialStore,
};

#[derive(Debug)]
pub(crate) struct RuntimeProviders {
    service: ProviderService,
    repository: Arc<dyn ProviderRepository>,
    credentials: Arc<RuntimeCredentialStore>,
}

impl RuntimeProviders {
    pub(crate) fn new(
        repository: Arc<dyn ProviderRepository>,
        credentials: Arc<RuntimeCredentialStore>,
        policy: Arc<dyn music_application::assistant::ProviderConnectionPolicy>,
        executable_contract_digest: String,
    ) -> Self {
        let credential_source: Arc<dyn ProviderCredentialSource> = credentials.clone();
        Self {
            service: ProviderService::new(
                Arc::clone(&repository),
                credential_source,
                policy,
                executable_contract_digest,
            ),
            repository,
            credentials,
        }
    }

    async fn storage_status(&self) -> Result<CredentialStorageStatus, ApiError> {
        let saved = self
            .repository
            .saved_provider_credentials_exist()
            .await
            .map_err(|source| {
                tracing::error!(error = %source, "provider credential status lookup failed");
                ApiError::internal()
            })?;
        Ok(self.credentials.status(saved).await)
    }

    async fn initialize_storage(&self) -> Result<CredentialStorageStatus, ApiError> {
        let saved = self
            .repository
            .saved_provider_credentials_exist()
            .await
            .map_err(|source| {
                tracing::error!(error = %source, "provider credential status lookup failed");
                ApiError::internal()
            })?;
        self.credentials
            .initialize(saved)
            .await
            .map_err(map_initialization_error)
    }

    async fn reset_storage(&self) -> Result<CredentialStorageReset, ApiError> {
        self.credentials
            .reset(self.repository.as_ref())
            .await
            .map_err(map_reset_error)
    }
}

pub(crate) fn provider_runtime_contract_digest() -> String {
    format!(
        "{:x}",
        Sha256::digest(b"music-rust-provider-runtime/pending-v1")
    )
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProviderCapabilityOut)]
struct ProviderCapabilityResponse {
    id: String,
    label: String,
    description: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProviderAdapterOut)]
struct ProviderAdapterResponse {
    id: String,
    label: String,
    description: String,
    capability_ids: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelRoleDefinitionOut)]
struct ModelRoleDefinitionResponse {
    id: String,
    label: String,
    description: String,
    required_capability_ids: Vec<String>,
    configuration_available: bool,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum CredentialStorageSourceResponse {
    Environment,
    File,
}

impl From<CredentialStorageSource> for CredentialStorageSourceResponse {
    fn from(value: CredentialStorageSource) -> Self {
        match value {
            CredentialStorageSource::Environment => Self::Environment,
            CredentialStorageSource::File => Self::File,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProviderFrameworkStatusOut)]
struct ProviderFrameworkStatusResponse {
    credential_storage_ready: bool,
    #[schema(required = true, schema_with = nullable_string_schema)]
    credential_storage_error: Option<String>,
    #[schema(required = true, schema_with = nullable_credential_source_schema)]
    credential_storage_source: Option<CredentialStorageSourceResponse>,
    #[schema(required = true, schema_with = nullable_string_schema)]
    credential_storage_key_id: Option<String>,
    #[schema(required = true, schema_with = nullable_string_schema)]
    credential_storage_key_file_path: Option<String>,
    #[schema(required = true, schema_with = nullable_string_schema)]
    credential_storage_host_directory_hint: Option<String>,
    credential_storage_can_initialize: bool,
    #[schema(required = true, schema_with = nullable_string_schema)]
    credential_storage_initialization_error: Option<String>,
    capabilities: Vec<ProviderCapabilityResponse>,
    adapters: Vec<ProviderAdapterResponse>,
    roles: Vec<ModelRoleDefinitionResponse>,
}

impl From<CredentialStorageStatus> for ProviderFrameworkStatusResponse {
    fn from(value: CredentialStorageStatus) -> Self {
        Self {
            credential_storage_ready: value.ready,
            credential_storage_error: value.error,
            credential_storage_source: value.source.map(Into::into),
            credential_storage_key_id: value.key_id,
            credential_storage_key_file_path: value.key_file_path,
            credential_storage_host_directory_hint: value.host_directory_hint,
            credential_storage_can_initialize: value.can_initialize,
            credential_storage_initialization_error: value.initialization_error,
            capabilities: PROVIDER_CAPABILITIES
                .iter()
                .map(|definition| ProviderCapabilityResponse {
                    id: definition.id.to_owned(),
                    label: definition.label.to_owned(),
                    description: definition.description.to_owned(),
                })
                .collect(),
            adapters: PROVIDER_ADAPTERS
                .iter()
                .map(|definition| ProviderAdapterResponse {
                    id: definition.id.to_owned(),
                    label: definition.label.to_owned(),
                    description: definition.description.to_owned(),
                    capability_ids: definition
                        .capability_ids
                        .iter()
                        .map(|value| (*value).to_owned())
                        .collect(),
                })
                .collect(),
            roles: MODEL_ROLES
                .iter()
                .map(|definition| ModelRoleDefinitionResponse {
                    id: definition.id.to_owned(),
                    label: definition.label.to_owned(),
                    description: definition.description.to_owned(),
                    required_capability_ids: definition
                        .required_capability_ids
                        .iter()
                        .map(|value| (*value).to_owned())
                        .collect(),
                    configuration_available: definition.configuration_available,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProviderCredentialStorageReset)]
struct ProviderCredentialStorageResetRequest {
    #[schema(min_length = 1, max_length = 256, format = Password, write_only)]
    current_password: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProviderCredentialStorageResetOut)]
struct ProviderCredentialStorageResetResponse {
    #[schema(schema_with = nonnegative_integer_schema)]
    deleted_credentials: u64,
    master_key_removed: bool,
    #[schema(required = true, schema_with = nullable_string_schema)]
    master_key_removal_error: Option<String>,
    status: ProviderFrameworkStatusResponse,
}

impl From<CredentialStorageReset> for ProviderCredentialStorageResetResponse {
    fn from(value: CredentialStorageReset) -> Self {
        Self {
            deleted_credentials: value.deleted_credentials,
            master_key_removed: value.master_key_removed,
            master_key_removal_error: value.master_key_removal_error,
            status: value.status.into(),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProviderConnectionCreate)]
struct ProviderConnectionCreateRequest {
    #[schema(min_length = 1, max_length = 128)]
    name: String,
    #[schema(min_length = 1, max_length = 64)]
    adapter_id: String,
    #[schema(min_length = 1, max_length = 2048)]
    base_url: String,
    #[schema(min_length = 1, max_length = 4096, format = Password, write_only)]
    api_key: String,
    #[serde(default)]
    #[schema(default = false)]
    allow_private_network: bool,
}

impl From<ProviderConnectionCreateRequest> for ProviderConnectionCreate {
    fn from(value: ProviderConnectionCreateRequest) -> Self {
        Self {
            name: value.name,
            adapter_id: value.adapter_id,
            base_url: value.base_url,
            api_key: ProviderSecret::new(value.api_key),
            allow_private_network: value.allow_private_network,
        }
    }
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProviderConnectionUpdate)]
struct ProviderConnectionUpdateRequest {
    #[schema(required = false, schema_with = nullable_name_schema)]
    name: Option<String>,
    #[schema(required = false, schema_with = nullable_adapter_id_schema)]
    adapter_id: Option<String>,
    #[schema(required = false, schema_with = nullable_base_url_schema)]
    base_url: Option<String>,
    #[schema(required = false, schema_with = nullable_api_key_schema)]
    api_key: Option<String>,
    #[schema(required = false, schema_with = nullable_boolean_schema)]
    allow_private_network: Option<bool>,
}

impl From<ProviderConnectionUpdateRequest> for ProviderConnectionPatch {
    fn from(value: ProviderConnectionUpdateRequest) -> Self {
        Self {
            name: value.name,
            adapter_id: value.adapter_id,
            base_url: value.base_url,
            api_key: value.api_key.map(ProviderSecret::new),
            allow_private_network: value.allow_private_network,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum ProviderVerificationStatusResponse {
    Never,
    Verified,
    Failed,
}

impl From<ProviderVerificationStatus> for ProviderVerificationStatusResponse {
    fn from(value: ProviderVerificationStatus) -> Self {
        match value {
            ProviderVerificationStatus::Never => Self::Never,
            ProviderVerificationStatus::Verified => Self::Verified,
            ProviderVerificationStatus::Failed => Self::Failed,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProviderConnectionOut)]
struct ProviderConnectionResponse {
    id: String,
    name: String,
    adapter_id: String,
    base_url: String,
    credential_saved: bool,
    #[schema(required = true, schema_with = nullable_string_schema)]
    key_hint: Option<String>,
    allow_private_network: bool,
    #[schema(schema_with = verification_status_schema)]
    verification_status: ProviderVerificationStatusResponse,
    #[schema(required = true, schema_with = nullable_string_schema)]
    verification_error_code: Option<String>,
    verified_models: Vec<String>,
    verified_capability_ids: Vec<String>,
    #[schema(required = true, schema_with = nullable_datetime_schema)]
    last_verified_at: Option<String>,
    #[schema(schema_with = openapi_datetime)]
    created_at: String,
    #[schema(schema_with = openapi_datetime)]
    updated_at: String,
}

impl TryFrom<ProviderConnectionView> for ProviderConnectionResponse {
    type Error = ApiError;

    fn try_from(value: ProviderConnectionView) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            name: value.name,
            adapter_id: value.adapter_id,
            base_url: value.base_url,
            credential_saved: value.credential_saved,
            key_hint: value.key_hint,
            allow_private_network: value.allow_private_network,
            verification_status: value.verification_status.into(),
            verification_error_code: value.verification_error_code,
            verified_models: value.verified_models,
            verified_capability_ids: value.verified_capability_ids,
            last_verified_at: value
                .last_verified_at_unix_seconds
                .map(UnixSeconds::new)
                .map(format_rfc3339)
                .transpose()?,
            created_at: format_rfc3339(UnixSeconds::new(value.created_at_unix_seconds))?,
            updated_at: format_rfc3339(UnixSeconds::new(value.updated_at_unix_seconds))?,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum ThinkingModeWire {
    #[default]
    ProviderDefault,
    Enabled,
    Disabled,
}

impl From<ThinkingModeWire> for ThinkingMode {
    fn from(value: ThinkingModeWire) -> Self {
        match value {
            ThinkingModeWire::ProviderDefault => Self::ProviderDefault,
            ThinkingModeWire::Enabled => Self::Enabled,
            ThinkingModeWire::Disabled => Self::Disabled,
        }
    }
}

impl From<ThinkingMode> for ThinkingModeWire {
    fn from(value: ThinkingMode) -> Self {
        match value {
            ThinkingMode::ProviderDefault => Self::ProviderDefault,
            ThinkingMode::Enabled => Self::Enabled,
            ThinkingMode::Disabled => Self::Disabled,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelRoleUpdate)]
struct ModelRoleUpdateRequest {
    #[schema(min_length = 1, max_length = 32)]
    connection_id: String,
    #[schema(min_length = 1, max_length = 256)]
    model_id: String,
    #[serde(default)]
    #[schema(default = false)]
    enabled: bool,
    #[serde(default = "default_timeout_seconds")]
    #[schema(schema_with = timeout_seconds_schema)]
    timeout_seconds: u16,
    #[serde(default = "default_max_output_tokens")]
    #[schema(schema_with = max_output_tokens_schema)]
    max_output_tokens: u32,
    #[serde(default)]
    #[schema(schema_with = thinking_mode_default_schema)]
    thinking_mode: ThinkingModeWire,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum ModelConformanceStatusResponse {
    Never,
    Passed,
    Failed,
}

impl From<ModelConformanceStatus> for ModelConformanceStatusResponse {
    fn from(value: ModelConformanceStatus) -> Self {
        match value {
            ModelConformanceStatus::Never => Self::Never,
            ModelConformanceStatus::Passed => Self::Passed,
            ModelConformanceStatus::Failed => Self::Failed,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ModelRoleOut)]
struct ModelRoleResponse {
    role_id: String,
    label: String,
    description: String,
    required_capability_ids: Vec<String>,
    configuration_available: bool,
    #[schema(required = true, schema_with = nullable_string_schema)]
    connection_id: Option<String>,
    #[schema(required = true, schema_with = nullable_string_schema)]
    connection_name: Option<String>,
    model_id: String,
    enabled: bool,
    effective_enabled: bool,
    #[schema(schema_with = integer_schema)]
    timeout_seconds: u16,
    #[schema(schema_with = integer_schema)]
    max_output_tokens: u32,
    #[schema(schema_with = thinking_mode_schema)]
    thinking_mode: ThinkingModeWire,
    #[schema(required = true, schema_with = nullable_verification_status_schema)]
    verification_status: Option<ProviderVerificationStatusResponse>,
    #[schema(schema_with = conformance_status_schema)]
    conformance_status: ModelConformanceStatusResponse,
    #[schema(required = true, schema_with = nullable_string_schema)]
    conformance_error_code: Option<String>,
    #[schema(required = true, schema_with = nullable_datetime_schema)]
    last_conformance_at: Option<String>,
    #[schema(required = true, schema_with = nullable_datetime_schema)]
    updated_at: Option<String>,
}

impl TryFrom<ModelRoleView> for ModelRoleResponse {
    type Error = ApiError;

    fn try_from(value: ModelRoleView) -> Result<Self, Self::Error> {
        Ok(Self {
            role_id: value.role_id,
            label: value.label,
            description: value.description,
            required_capability_ids: value.required_capability_ids,
            configuration_available: value.configuration_available,
            connection_id: value.connection_id,
            connection_name: value.connection_name,
            model_id: value.model_id,
            enabled: value.enabled,
            effective_enabled: value.effective_enabled,
            timeout_seconds: value.timeout_seconds,
            max_output_tokens: value.max_output_tokens,
            thinking_mode: value.thinking_mode.into(),
            verification_status: value.verification_status.map(Into::into),
            conformance_status: value.conformance_status.into(),
            conformance_error_code: value.conformance_error_code,
            last_conformance_at: value
                .last_conformance_at_unix_seconds
                .map(UnixSeconds::new)
                .map(format_rfc3339)
                .transpose()?,
            updated_at: value
                .updated_at_unix_seconds
                .map(UnixSeconds::new)
                .map(format_rfc3339)
                .transpose()?,
        })
    }
}

pub(crate) fn provider_router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default()
        .routes(routes!(framework_status))
        .routes(routes!(initialize_credential_storage))
        .routes(routes!(reset_credential_storage))
        .routes(routes!(list_connections, create_connection))
        .routes(routes!(update_connection, delete_connection))
        .routes(routes!(delete_connection_credential))
        .routes(routes!(list_roles))
        .routes(routes!(update_role, delete_role))
}

#[utoipa::path(
    get,
    path = "/assistant/providers/status",
    operation_id = "get_framework_status_api_assistant_providers_status_get",
    responses((status = 200, description = "Successful Response", body = ProviderFrameworkStatusResponse)),
    tag = "assistant"
)]
async fn framework_status(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<ProviderFrameworkStatusResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let status = providers(&state)?.storage_status().await?;
    Ok(Json(status.into()))
}

#[utoipa::path(
    post,
    path = "/assistant/providers/credential-storage/initialize",
    operation_id = "initialize_provider_storage_api_assistant_providers_credential_storage_initialize_post",
    responses((status = 201, description = "Successful Response", body = ProviderFrameworkStatusResponse)),
    tag = "assistant"
)]
async fn initialize_credential_storage(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<ProviderFrameworkStatusResponse>), ApiError> {
    authorize(&state, &headers).await?;
    let status = providers(&state)?.initialize_storage().await?;
    Ok((StatusCode::CREATED, Json(status.into())))
}

#[utoipa::path(
    post,
    path = "/assistant/providers/credential-storage/reset",
    operation_id = "reset_provider_storage_api_assistant_providers_credential_storage_reset_post",
    request_body = ProviderCredentialStorageResetRequest,
    responses(
        (status = 200, description = "Successful Response", body = ProviderCredentialStorageResetResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
async fn reset_credential_storage(
    State(state): State<HttpState>,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    payload: Result<Json<ProviderCredentialStorageResetRequest>, JsonRejection>,
) -> Result<Json<ProviderCredentialStorageResetResponse>, ApiError> {
    let current = current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if !(1..=256).contains(&payload.current_password.chars().count()) {
        return Err(ApiError::validation());
    }
    let password = Zeroizing::new(payload.current_password);
    let throttle_key = peer.map_or_else(
        || "unknown".to_owned(),
        |Extension(ConnectInfo(address))| address.ip().to_string(),
    );
    let auth = state
        .auth
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)?;
    match auth
        .reauthenticate(&throttle_key, &current.user.username, &password)
        .await
    {
        Ok(()) => {}
        Err(PasswordConfirmationError::Throttled) => {
            return Err(ApiError::coded(
                StatusCode::TOO_MANY_REQUESTS,
                "password_confirmation_throttled",
                "Too many password attempts; try again shortly.",
            ));
        }
        Err(PasswordConfirmationError::Invalid) => {
            return Err(ApiError::coded(
                StatusCode::FORBIDDEN,
                "current_password_invalid",
                "The current account password is incorrect.",
            ));
        }
        Err(PasswordConfirmationError::Internal(error)) => {
            tracing::error!(error = %error, "provider credential reset authentication failed");
            return Err(ApiError::internal());
        }
    }
    Ok(Json(providers(&state)?.reset_storage().await?.into()))
}

#[utoipa::path(
    get,
    path = "/assistant/providers/connections",
    operation_id = "get_connections_api_assistant_providers_connections_get",
    responses((status = 200, description = "Successful Response", body = [ProviderConnectionResponse])),
    tag = "assistant"
)]
async fn list_connections(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ProviderConnectionResponse>>, ApiError> {
    authorize(&state, &headers).await?;
    let values = providers(&state)?
        .service
        .list_connections()
        .await
        .map_err(map_provider_error)?;
    Ok(Json(
        values
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<_, _>>()?,
    ))
}

#[utoipa::path(
    post,
    path = "/assistant/providers/connections",
    operation_id = "add_connection_api_assistant_providers_connections_post",
    request_body = ProviderConnectionCreateRequest,
    responses(
        (status = 201, description = "Successful Response", body = ProviderConnectionResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
async fn create_connection(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<ProviderConnectionCreateRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ProviderConnectionResponse>), ApiError> {
    authorize(&state, &headers).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let value = providers(&state)?
        .service
        .create_connection(payload.into())
        .await
        .map_err(map_provider_error)?;
    Ok((StatusCode::CREATED, Json(value.try_into()?)))
}

#[utoipa::path(
    put,
    path = "/assistant/providers/connections/{connection_id}",
    operation_id = "edit_connection_api_assistant_providers_connections__connection_id__put",
    params(("connection_id" = String, Path, description = "Connection Id")),
    request_body = ProviderConnectionUpdateRequest,
    responses(
        (status = 200, description = "Successful Response", body = ProviderConnectionResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
async fn update_connection(
    State(state): State<HttpState>,
    headers: HeaderMap,
    connection_id: Result<Path<String>, PathRejection>,
    payload: Result<Json<ProviderConnectionUpdateRequest>, JsonRejection>,
) -> Result<Json<ProviderConnectionResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Path(connection_id) = connection_id.map_err(|_| ApiError::validation())?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let value = providers(&state)?
        .service
        .update_connection(&connection_id, payload.into())
        .await
        .map_err(map_provider_error)?;
    Ok(Json(value.try_into()?))
}

#[utoipa::path(
    delete,
    path = "/assistant/providers/connections/{connection_id}",
    operation_id = "remove_connection_api_assistant_providers_connections__connection_id__delete",
    params(("connection_id" = String, Path, description = "Connection Id")),
    responses(
        (status = 204, description = "Successful Response"),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
async fn delete_connection(
    State(state): State<HttpState>,
    headers: HeaderMap,
    connection_id: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    authorize(&state, &headers).await?;
    let Path(connection_id) = connection_id.map_err(|_| ApiError::validation())?;
    providers(&state)?
        .service
        .delete_connection(&connection_id)
        .await
        .map_err(map_provider_error)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[utoipa::path(
    delete,
    path = "/assistant/providers/connections/{connection_id}/credential",
    operation_id = "remove_connection_credential_api_assistant_providers_connections__connection_id__credential_delete",
    params(("connection_id" = String, Path, description = "Connection Id")),
    responses(
        (status = 200, description = "Successful Response", body = ProviderConnectionResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
async fn delete_connection_credential(
    State(state): State<HttpState>,
    headers: HeaderMap,
    connection_id: Result<Path<String>, PathRejection>,
) -> Result<Json<ProviderConnectionResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Path(connection_id) = connection_id.map_err(|_| ApiError::validation())?;
    let value = providers(&state)?
        .service
        .delete_connection_credential(&connection_id)
        .await
        .map_err(map_provider_error)?;
    Ok(Json(value.try_into()?))
}

#[utoipa::path(
    get,
    path = "/assistant/providers/roles",
    operation_id = "get_roles_api_assistant_providers_roles_get",
    responses((status = 200, description = "Successful Response", body = [ModelRoleResponse])),
    tag = "assistant"
)]
async fn list_roles(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ModelRoleResponse>>, ApiError> {
    authorize(&state, &headers).await?;
    let values = providers(&state)?
        .service
        .list_model_roles()
        .await
        .map_err(map_provider_error)?;
    Ok(Json(
        values
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<_, _>>()?,
    ))
}

#[utoipa::path(
    put,
    path = "/assistant/providers/roles/{role_id}",
    operation_id = "set_role_api_assistant_providers_roles__role_id__put",
    params(("role_id" = String, Path, description = "Role Id")),
    request_body = ModelRoleUpdateRequest,
    responses(
        (status = 200, description = "Successful Response", body = ModelRoleResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
async fn update_role(
    State(state): State<HttpState>,
    headers: HeaderMap,
    role_id: Result<Path<String>, PathRejection>,
    payload: Result<Json<ModelRoleUpdateRequest>, JsonRejection>,
) -> Result<Json<ModelRoleResponse>, ApiError> {
    authorize(&state, &headers).await?;
    let Path(role_id) = role_id.map_err(|_| ApiError::validation())?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    let value = providers(&state)?
        .service
        .update_model_role(
            &role_id,
            ModelRoleUpdate {
                connection_id: payload.connection_id,
                model_id: payload.model_id,
                enabled: payload.enabled,
                timeout_seconds: payload.timeout_seconds,
                max_output_tokens: payload.max_output_tokens,
                thinking_mode: payload.thinking_mode.into(),
            },
        )
        .await
        .map_err(map_provider_error)?;
    Ok(Json(value.try_into()?))
}

#[utoipa::path(
    delete,
    path = "/assistant/providers/roles/{role_id}",
    operation_id = "remove_role_api_assistant_providers_roles__role_id__delete",
    params(("role_id" = String, Path, description = "Role Id")),
    responses(
        (status = 204, description = "Successful Response"),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "assistant"
)]
async fn delete_role(
    State(state): State<HttpState>,
    headers: HeaderMap,
    role_id: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    authorize(&state, &headers).await?;
    let Path(role_id) = role_id.map_err(|_| ApiError::validation())?;
    providers(&state)?
        .service
        .delete_model_role(&role_id)
        .await
        .map_err(map_provider_error)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

fn providers(state: &HttpState) -> Result<&RuntimeProviders, ApiError> {
    state
        .providers
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)
}

async fn authorize(state: &HttpState, headers: &HeaderMap) -> Result<(), ApiError> {
    current_session(state, headers, SessionTouch::UpdateLastSeen)
        .await
        .map(|_| ())
}

fn map_provider_error(error: ProviderServiceError) -> ApiError {
    if error.kind() == ProviderServiceErrorKind::Dependency {
        tracing::error!(error = %error, "provider request failed");
        return ApiError::internal();
    }
    let status = match error.kind() {
        ProviderServiceErrorKind::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        ProviderServiceErrorKind::NotFound => StatusCode::NOT_FOUND,
        ProviderServiceErrorKind::Conflict => StatusCode::CONFLICT,
        ProviderServiceErrorKind::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        ProviderServiceErrorKind::Dependency => StatusCode::INTERNAL_SERVER_ERROR,
    };
    ApiError::coded(status, error.code(), error.message())
}

fn map_initialization_error(error: CredentialStoreError) -> ApiError {
    let status = if matches!(
        error.code(),
        "master_key_already_configured"
            | "master_key_file_exists"
            | "master_key_managed_by_environment"
            | "saved_credentials_require_existing_key"
    ) {
        StatusCode::CONFLICT
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let code = stable_credential_code(error.code());
    ApiError::coded(
        status,
        code,
        "Encrypted provider-key storage could not be initialized safely.",
    )
}

fn map_reset_error(error: CredentialStoreError) -> ApiError {
    let status = if matches!(
        error.code(),
        "master_key_managed_by_environment" | "master_key_file_not_configured" | "model_job_active"
    ) {
        StatusCode::CONFLICT
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let code = stable_credential_code(error.code());
    let message = if error.code() == "model_job_active" {
        "Cancel or wait for active model jobs before resetting AI storage."
    } else {
        "This credential store cannot be reset safely from the browser."
    };
    ApiError::coded(status, code, message)
}

fn stable_credential_code(code: &str) -> &'static str {
    match code {
        "master_key_not_configured" => "master_key_not_configured",
        "invalid_master_key" => "invalid_master_key",
        "master_key_file_unreadable" => "master_key_file_unreadable",
        "master_key_file_unsafe" => "master_key_file_unsafe",
        "master_key_file_permissions" => "master_key_file_permissions",
        "master_key_already_configured" => "master_key_already_configured",
        "master_key_file_exists" => "master_key_file_exists",
        "master_key_managed_by_environment" => "master_key_managed_by_environment",
        "saved_credentials_require_existing_key" => "saved_credentials_require_existing_key",
        "master_key_file_not_configured" => "master_key_file_not_configured",
        "master_key_file_path_not_absolute" => "master_key_file_path_not_absolute",
        "master_key_directory_unavailable" => "master_key_directory_unavailable",
        "master_key_directory_unsafe" => "master_key_directory_unsafe",
        "master_key_directory_permissions" => "master_key_directory_permissions",
        "master_key_directory_not_writable" => "master_key_directory_not_writable",
        "master_key_file_write_failed" => "master_key_file_write_failed",
        "master_key_initialization_failed" => "master_key_initialization_failed",
        "master_key_generation_failed" => "master_key_generation_failed",
        "master_key_file_delete_failed" => "master_key_file_delete_failed",
        "master_key_storage_changed" => "master_key_storage_changed",
        "model_job_active" => "model_job_active",
        _ => "credential_storage_unavailable",
    }
}

fn nullable_datetime_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(openapi_datetime())
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_string_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(ObjectBuilder::new().schema_type(Type::String))
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_boolean_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(ObjectBuilder::new().schema_type(Type::Boolean))
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_bounded_string_schema(minimum: usize, maximum: usize) -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .min_length(Some(minimum))
                    .max_length(Some(maximum)),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn nullable_name_schema() -> RefOr<Schema> {
    nullable_bounded_string_schema(1, 128)
}

fn nullable_adapter_id_schema() -> RefOr<Schema> {
    nullable_bounded_string_schema(1, 64)
}

fn nullable_base_url_schema() -> RefOr<Schema> {
    nullable_bounded_string_schema(1, 2_048)
}

fn nullable_api_key_schema() -> RefOr<Schema> {
    let mut password = ObjectBuilder::new()
        .schema_type(Type::String)
        .min_length(Some(1))
        .max_length(Some(4_096))
        .build();
    password.format = Some(utoipa::openapi::schema::SchemaFormat::KnownFormat(
        utoipa::openapi::schema::KnownFormat::Password,
    ));
    password.write_only = Some(true);
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(password)
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn verification_status_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["never", "verified", "failed"]))
        .into()
}

fn nullable_verification_status_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(verification_status_schema())
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn conformance_status_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["never", "passed", "failed"]))
        .into()
}

fn thinking_mode_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["provider_default", "enabled", "disabled"]))
        .into()
}

fn thinking_mode_default_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["provider_default", "enabled", "disabled"]))
        .default(Some(serde_json::json!("provider_default")))
        .into()
}

fn nullable_credential_source_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .enum_values(Some(["environment", "file"])),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

fn integer_schema() -> RefOr<Schema> {
    ObjectBuilder::new().schema_type(Type::Integer).into()
}

fn nonnegative_integer_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(0))
        .into()
}

fn timeout_seconds_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(5))
        .maximum(Some(300))
        .default(Some(serde_json::json!(30)))
        .into()
}

fn max_output_tokens_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(128))
        .maximum(Some(65_536))
        .default(Some(serde_json::json!(2_000)))
        .into()
}

const fn default_timeout_seconds() -> u16 {
    30
}

const fn default_max_output_tokens() -> u32 {
    2_000
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::error::Error;
    use std::path::Path;

    use axum::body::{Body, to_bytes};
    use axum::http::header::{CONTENT_TYPE, COOKIE, SET_COOKIE};
    use axum::http::{Request, StatusCode};
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE;
    use music_application::auth::UnixSeconds;
    use music_storage::{SqliteStorage, SqliteStorageOptions, hash_password};
    use serde_json::Value;
    use tempfile::tempdir;
    use tower::ServiceExt;

    use crate::{AppConfig, AppRuntime};

    fn runtime_config(root: &Path) -> Result<AppConfig, crate::ConfigError> {
        AppConfig::from_values(&BTreeMap::from([
            (
                "DATABASE_URL".to_owned(),
                format!("sqlite:///{}", root.join("app.db").display()),
            ),
            (
                "MUSIC_DIR".to_owned(),
                root.join("music").display().to_string(),
            ),
            (
                "SFX_LIBRARY_DIR".to_owned(),
                root.join("sfx").display().to_string(),
            ),
            (
                "MODES_DIR".to_owned(),
                root.join("modes").display().to_string(),
            ),
            (
                "DEVICES_FILE".to_owned(),
                root.join("devices.json").display().to_string(),
            ),
            (
                "STATIC_DIR".to_owned(),
                root.join("missing-static").display().to_string(),
            ),
            ("SESSION_COOKIE_SECURE".to_owned(), "false".to_owned()),
            ("SESSION_COOKIE_NAME".to_owned(), "test_session".to_owned()),
            (
                "ASSISTANT_CREDENTIAL_KEY".to_owned(),
                URL_SAFE.encode([7_u8; 32]),
            ),
        ]))
    }

    async fn body_json(
        response: axum::response::Response,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        let body = to_bytes(response.into_body(), 1024 * 1024).await?;
        Ok(serde_json::from_slice(&body)?)
    }

    #[tokio::test]
    async fn authenticated_provider_management_never_returns_secret_material()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let config = runtime_config(directory.path())?;
        let storage = SqliteStorage::open(SqliteStorageOptions::new(&config.database_path)).await?;
        storage
            .create_user(
                "operator",
                &hash_password("correct-password")?,
                UnixSeconds::new(1_800_000_000),
            )
            .await?;
        storage.close().await;
        drop(storage);

        let runtime = AppRuntime::start(config).await?;
        let router = runtime.router()?;
        let login = router
            .clone()
            .oneshot(
                Request::post("/api/auth/login")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"username":"operator","password":"correct-password"}"#,
                    ))?,
            )
            .await?;
        assert_eq!(login.status(), StatusCode::OK);
        let cookie = login
            .headers()
            .get(SET_COOKIE)
            .ok_or("missing session cookie")?
            .to_str()?
            .split(';')
            .next()
            .ok_or("empty session cookie")?
            .to_owned();

        let status = router
            .clone()
            .oneshot(
                Request::get("/api/assistant/providers/status")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(status.status(), StatusCode::OK);
        let status = body_json(status).await?;
        assert_eq!(status["credential_storage_ready"], true);
        assert_eq!(status["credential_storage_source"], "environment");
        assert_eq!(status["adapters"].as_array().map(Vec::len), Some(5));

        let created = router
            .clone()
            .oneshot(
                Request::post("/api/assistant/providers/connections")
                    .header(CONTENT_TYPE, "application/json")
                    .header(COOKIE, &cookie)
                    .body(Body::from(
                        r#"{"name":"Local fixture","adapter_id":"openai-compatible/v1","base_url":"http://127.0.0.1:11434/v1/","api_key":"provider-secret-value","allow_private_network":true}"#,
                    ))?,
            )
            .await?;
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = body_json(created).await?;
        assert_eq!(created["base_url"], "http://127.0.0.1:11434/v1");
        assert_eq!(created["credential_saved"], true);
        assert_eq!(created["key_hint"], "••••alue");
        assert!(created.get("api_key").is_none());
        assert!(!created.to_string().contains("provider-secret-value"));
        let connection_id = created["id"].as_str().ok_or("missing ID")?;

        let configured = router
            .clone()
            .oneshot(
                Request::put("/api/assistant/providers/roles/music_tagger")
                    .header(CONTENT_TYPE, "application/json")
                    .header(COOKIE, &cookie)
                    .body(Body::from(format!(
                        r#"{{"connection_id":"{connection_id}","model_id":"fixture-model"}}"#
                    )))?,
            )
            .await?;
        assert_eq!(configured.status(), StatusCode::OK);
        let configured = body_json(configured).await?;
        assert_eq!(configured["enabled"], false);
        assert_eq!(configured["effective_enabled"], false);

        let in_use = router
            .clone()
            .oneshot(
                Request::delete(format!(
                    "/api/assistant/providers/connections/{connection_id}"
                ))
                .header(COOKIE, &cookie)
                .body(Body::empty())?,
            )
            .await?;
        assert_eq!(in_use.status(), StatusCode::CONFLICT);
        assert_eq!(
            body_json(in_use).await?["detail"]["code"],
            "connection_in_use"
        );

        let reset = router
            .clone()
            .oneshot(
                Request::post("/api/assistant/providers/credential-storage/reset")
                    .header(CONTENT_TYPE, "application/json")
                    .header(COOKIE, &cookie)
                    .body(Body::from(r#"{"current_password":"correct-password"}"#))?,
            )
            .await?;
        assert_eq!(reset.status(), StatusCode::CONFLICT);
        assert_eq!(
            body_json(reset).await?["detail"]["code"],
            "master_key_managed_by_environment"
        );

        runtime.shutdown().await?;
        Ok(())
    }
}
