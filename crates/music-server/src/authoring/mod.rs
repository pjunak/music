mod model;
mod service;

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::HeaderMap;
use music_application::auth::SessionTouch;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{AdditionalProperties, ObjectBuilder, Schema, Type};
use utoipa::{PartialSchema, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use model::{
    AuthoringDocumentCommitRequest, AuthoringDocumentPreviewRequest, AuthoringImportCommitRequest,
    AuthoringImportPreview, AuthoringImportPreviewRequest, AuthoringImportResult,
};
use service::ImportSourceSpec;

use crate::auth::current_session;
use crate::error::{ApiError, HttpValidationErrorBody};
use crate::http::HttpState;

pub(crate) fn authoring_router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default()
        .routes(routes!(get_document_schema))
        .routes(routes!(preview_mode_import))
        .routes(routes!(commit_mode_import))
        .routes(routes!(preview_document_import))
        .routes(routes!(commit_document_import))
}

#[cfg(feature = "fuzzing")]
pub(crate) fn exercise_document_parser(input: &[u8]) {
    if let Ok(payload) = serde_json::from_slice::<AuthoringDocumentPreviewRequest>(input) {
        let _ = payload.validate();
    }
    if let Ok(payload) = serde_json::from_slice::<AuthoringDocumentCommitRequest>(input) {
        let _ = payload.validate();
    }
}

struct DocumentSchemaContract;

impl PartialSchema for DocumentSchemaContract {
    fn schema() -> RefOr<Schema> {
        ObjectBuilder::new()
            .schema_type(Type::Object)
            .additional_properties(Some(AdditionalProperties::FreeForm(true)))
            .into()
    }
}

impl ToSchema for DocumentSchemaContract {}

#[utoipa::path(
    get,
    path = "/authoring/import/document/schema",
    responses((status = 200, description = "Successful Response", body = DocumentSchemaContract)),
    tag = "authoring"
)]
async fn get_document_schema(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    authenticate(&state, &headers).await?;
    model::public_document_schema()
        .map(Json)
        .map_err(|_| ApiError::internal())
}

#[utoipa::path(
    post,
    path = "/authoring/import/preview",
    request_body = AuthoringImportPreviewRequest,
    responses(
        (status = 200, description = "Successful Response", body = AuthoringImportPreview),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "authoring"
)]
async fn preview_mode_import(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<AuthoringImportPreviewRequest>, JsonRejection>,
) -> Result<Json<AuthoringImportPreview>, ApiError> {
    authenticate(&state, &headers).await?;
    let Json(payload) = json_payload(payload)?;
    payload.validate().map_err(|_| ApiError::validation())?;
    let source = ImportSourceSpec::Mode(payload.source_mode_id);
    service::preview(&state, &payload.target_mode_id, &source)
        .await
        .map(Json)
}

#[utoipa::path(
    post,
    path = "/authoring/import/commit",
    request_body = AuthoringImportCommitRequest,
    responses(
        (status = 200, description = "Successful Response", body = AuthoringImportResult),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "authoring"
)]
async fn commit_mode_import(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<AuthoringImportCommitRequest>, JsonRejection>,
) -> Result<Json<AuthoringImportResult>, ApiError> {
    authenticate(&state, &headers).await?;
    let Json(payload) = json_payload(payload)?;
    payload.validate().map_err(|_| ApiError::validation())?;
    let source = ImportSourceSpec::Mode(payload.source_mode_id);
    service::commit(&state, &payload.target_mode_id, &source, &payload.items)
        .await
        .map(Json)
}

#[utoipa::path(
    post,
    path = "/authoring/import/document/preview",
    request_body = AuthoringDocumentPreviewRequest,
    responses(
        (status = 200, description = "Successful Response", body = AuthoringImportPreview),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "authoring"
)]
async fn preview_document_import(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<AuthoringDocumentPreviewRequest>, JsonRejection>,
) -> Result<Json<AuthoringImportPreview>, ApiError> {
    authenticate(&state, &headers).await?;
    let Json(payload) = json_payload(payload)?;
    payload.validate().map_err(|_| ApiError::validation())?;
    let source = ImportSourceSpec::Document {
        document: payload.document,
        source_name: payload.source_name,
    };
    service::preview(&state, &payload.target_mode_id, &source)
        .await
        .map(Json)
}

#[utoipa::path(
    post,
    path = "/authoring/import/document/commit",
    request_body = AuthoringDocumentCommitRequest,
    responses(
        (status = 200, description = "Successful Response", body = AuthoringImportResult),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "authoring"
)]
async fn commit_document_import(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<AuthoringDocumentCommitRequest>, JsonRejection>,
) -> Result<Json<AuthoringImportResult>, ApiError> {
    authenticate(&state, &headers).await?;
    let Json(payload) = json_payload(payload)?;
    payload.validate().map_err(|_| ApiError::validation())?;
    let source = ImportSourceSpec::Document {
        document: payload.document,
        source_name: payload.source_name,
    };
    service::commit(&state, &payload.target_mode_id, &source, &payload.items)
        .await
        .map(Json)
}

async fn authenticate(state: &HttpState, headers: &HeaderMap) -> Result<(), ApiError> {
    current_session(state, headers, SessionTouch::UpdateLastSeen)
        .await
        .map(|_| ())
}

fn json_payload<T>(payload: Result<Json<T>, JsonRejection>) -> Result<Json<T>, ApiError> {
    payload.map_err(|_| ApiError::validation())
}
