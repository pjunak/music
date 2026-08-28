use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::HeaderMap;
use music_application::auth::SessionTouch;
use music_application::cleanup::{CleanupAnalysis, CleanupError, CleanupScope};
use music_domain::{
    CleanupFolderSuggestion, CleanupRule, CleanupRuleSet, CleanupSuggestion, CleanupTrackPlan,
    CleanupValue, DEFAULT_CLEANUP_RULES, LibraryPath, TrackId,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{AnyOfBuilder, ArrayBuilder, ObjectBuilder, Schema, Type};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::error::{ApiError, HttpValidationErrorBody, openapi_integer, openapi_nullable_string};
use crate::http::HttpState;

const MAX_SCOPE_TRACKS: usize = 5_000;

pub(crate) fn cleanup_router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default().routes(routes!(analyze))
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CleanupScopeKind {
    All,
    Folder,
    Tracks,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CleanupRuleRequest {
    StripTrackNumbers,
    StripArtist,
    StripAlbum,
    StripJunk,
    NormalizeSeparators,
    NormalizeCase,
    TagTitle,
    TagArtist,
    TagAlbum,
    TagNumber,
    TagYear,
    RenameFolders,
}

impl From<CleanupRuleRequest> for CleanupRule {
    fn from(value: CleanupRuleRequest) -> Self {
        match value {
            CleanupRuleRequest::StripTrackNumbers => Self::StripTrackNumbers,
            CleanupRuleRequest::StripArtist => Self::StripArtist,
            CleanupRuleRequest::StripAlbum => Self::StripAlbum,
            CleanupRuleRequest::StripJunk => Self::StripJunk,
            CleanupRuleRequest::NormalizeSeparators => Self::NormalizeSeparators,
            CleanupRuleRequest::NormalizeCase => Self::NormalizeCase,
            CleanupRuleRequest::TagTitle => Self::TagTitle,
            CleanupRuleRequest::TagArtist => Self::TagArtist,
            CleanupRuleRequest::TagAlbum => Self::TagAlbum,
            CleanupRuleRequest::TagNumber => Self::TagNumber,
            CleanupRuleRequest::TagYear => Self::TagYear,
            CleanupRuleRequest::RenameFolders => Self::RenameFolders,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = CleanupScope)]
struct CleanupScopeRequest {
    #[serde(rename = "type")]
    #[schema(rename = "type", schema_with = cleanup_scope_kind_schema)]
    kind: CleanupScopeKind,
    #[serde(default)]
    #[schema(required = false, schema_with = cleanup_scope_path_schema)]
    path: String,
    #[serde(default = "default_recursive")]
    #[schema(required = false, schema_with = cleanup_recursive_schema)]
    recursive: bool,
    #[serde(default)]
    #[schema(required = false, schema_with = cleanup_scope_track_ids_schema)]
    track_ids: Vec<i64>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = AnalyzeRequest)]
struct AnalyzeRequest {
    scope: CleanupScopeRequest,
    /// Enabled rule ids; omit for the default set.
    #[schema(required = false, schema_with = nullable_cleanup_rules_schema)]
    rules: Option<Vec<CleanupRuleRequest>>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum CleanupValueResponse {
    Number(u32),
    Text(String),
}

impl From<CleanupValue> for CleanupValueResponse {
    fn from(value: CleanupValue) -> Self {
        match value {
            CleanupValue::Number(value) => Self::Number(value),
            CleanupValue::Text(value) => Self::Text(value),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = CleanupOpOut)]
struct CleanupOperationResponse {
    op_id: String,
    #[schema(schema_with = openapi_integer)]
    track_id: i64,
    #[schema(schema_with = cleanup_operation_kind_schema)]
    kind: String,
    #[schema(required = true, schema_with = openapi_nullable_string)]
    field: Option<String>,
    #[schema(required = true, schema_with = cleanup_value_schema)]
    old: Option<CleanupValueResponse>,
    #[schema(required = true, schema_with = cleanup_value_schema)]
    new: Option<CleanupValueResponse>,
    rules: Vec<String>,
    #[schema(schema_with = cleanup_confidence_schema)]
    confidence: String,
    #[schema(required = false, default = false)]
    verified: bool,
}

impl From<CleanupSuggestion> for CleanupOperationResponse {
    fn from(operation: CleanupSuggestion) -> Self {
        Self {
            op_id: operation.operation_id(),
            track_id: operation.track_id.get(),
            kind: operation.kind.as_str().to_owned(),
            field: operation.field.map(|field| field.as_str().to_owned()),
            old: operation.old.map(Into::into),
            new: operation.new.map(Into::into),
            rules: operation.rules,
            confidence: operation.confidence.as_str().to_owned(),
            verified: operation.verified,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = CleanupTrackPlanOut)]
struct CleanupTrackPlanResponse {
    #[schema(schema_with = openapi_integer)]
    track_id: i64,
    path: String,
    ops: Vec<CleanupOperationResponse>,
    notes: Vec<String>,
}

impl From<CleanupTrackPlan> for CleanupTrackPlanResponse {
    fn from(plan: CleanupTrackPlan) -> Self {
        Self {
            track_id: plan.track_id.get(),
            path: plan.path,
            ops: plan.operations.into_iter().map(Into::into).collect(),
            notes: plan.notes,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = CleanupFolderOut)]
struct CleanupFolderResponse {
    op_id: String,
    path: String,
    old: String,
    new: String,
    rules: Vec<String>,
    #[schema(schema_with = cleanup_confidence_schema)]
    confidence: String,
}

impl From<CleanupFolderSuggestion> for CleanupFolderResponse {
    fn from(folder: CleanupFolderSuggestion) -> Self {
        Self {
            op_id: folder.operation_id(),
            path: folder.path,
            old: folder.old,
            new: folder.new,
            rules: folder.rules,
            confidence: folder.confidence.as_str().to_owned(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = AnalyzeResult)]
struct AnalyzeResponse {
    #[schema(schema_with = openapi_integer)]
    scanned: usize,
    plans: Vec<CleanupTrackPlanResponse>,
    folders: Vec<CleanupFolderResponse>,
    pending_lookups: Vec<String>,
}

impl From<CleanupAnalysis> for AnalyzeResponse {
    fn from(analysis: CleanupAnalysis) -> Self {
        Self {
            scanned: analysis.scanned,
            plans: analysis
                .plans
                .into_iter()
                .filter(|plan| !plan.operations.is_empty() || !plan.notes.is_empty())
                .map(Into::into)
                .collect(),
            folders: analysis.folders.into_iter().map(Into::into).collect(),
            pending_lookups: analysis.pending_lookups,
        }
    }
}

#[utoipa::path(
    post,
    path = "/library/cleanup/analyze",
    operation_id = "analyze_api_library_cleanup_analyze_post",
    summary = "Analyze",
    request_body = AnalyzeRequest,
    responses(
        (status = 200, description = "Successful Response", body = AnalyzeResponse),
        (status = 422, description = "Validation Error", body = HttpValidationErrorBody)
    ),
    tag = "library-cleanup"
)]
async fn analyze(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Result<Json<AnalyzeRequest>, JsonRejection>,
) -> Result<Json<AnalyzeResponse>, ApiError> {
    crate::auth::current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let Json(payload) = payload.map_err(|_| ApiError::validation())?;
    if payload.scope.track_ids.len() > MAX_SCOPE_TRACKS {
        return Err(ApiError::validation());
    }
    let scope = cleanup_scope(payload.scope)?;
    let rules = payload.rules.map_or(DEFAULT_CLEANUP_RULES, |rules| {
        rules
            .into_iter()
            .map(Into::into)
            .collect::<CleanupRuleSet>()
    });
    let analysis = crate::library::library(&state)?
        .cleanup
        .analyze(scope, rules)
        .await
        .map_err(map_cleanup_error)?;
    Ok(Json(analysis.into()))
}

fn cleanup_scope(scope: CleanupScopeRequest) -> Result<CleanupScope, ApiError> {
    match scope.kind {
        CleanupScopeKind::All => Ok(CleanupScope::All),
        CleanupScopeKind::Folder => {
            let normalized = scope.path.trim_matches('/').replace('\\', "/");
            let normalized = normalized.trim_matches('/');
            let path = if normalized.is_empty() {
                None
            } else {
                Some(
                    LibraryPath::parse(normalized.to_owned())
                        .map_err(|_| ApiError::bad_request("invalid cleanup folder path"))?,
                )
            };
            Ok(CleanupScope::Folder {
                path,
                recursive: scope.recursive,
            })
        }
        CleanupScopeKind::Tracks => {
            if scope.track_ids.is_empty() {
                return Err(ApiError::bad_request(
                    "scope.track_ids must be non-empty for a tracks scope",
                ));
            }
            let track_ids = scope
                .track_ids
                .into_iter()
                .map(|track_id| {
                    TrackId::new(track_id)
                        .map_err(|_| ApiError::bad_request("invalid cleanup track id"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CleanupScope::Tracks(track_ids))
        }
    }
}

fn map_cleanup_error(error: CleanupError) -> ApiError {
    match error {
        CleanupError::EmptyTrackScope => {
            ApiError::bad_request("scope.track_ids must be non-empty for a tracks scope")
        }
        CleanupError::Dependency { .. } => {
            tracing::error!(error = %error, "library cleanup analysis failed");
            ApiError::internal()
        }
    }
}

const fn default_recursive() -> bool {
    true
}

fn cleanup_scope_kind_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["all", "folder", "tracks"]))
        .into()
}

fn cleanup_scope_path_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .default(Some(serde_json::json!("")))
        .into()
}

fn cleanup_recursive_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Boolean)
        .default(Some(serde_json::json!(true)))
        .into()
}

fn cleanup_scope_track_ids_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(openapi_integer())
        .max_items(Some(MAX_SCOPE_TRACKS))
        .into()
}

fn nullable_cleanup_rules_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ArrayBuilder::new().items(
                    ObjectBuilder::new()
                        .schema_type(Type::String)
                        .enum_values(Some([
                            "strip_track_numbers",
                            "strip_artist",
                            "strip_album",
                            "strip_junk",
                            "normalize_separators",
                            "normalize_case",
                            "tag_title",
                            "tag_artist",
                            "tag_album",
                            "tag_number",
                            "tag_year",
                            "rename_folders",
                        ])),
                ),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .description(Some("Enabled rule ids; omit for the default set"))
            .build(),
    )
    .into()
}

fn cleanup_operation_kind_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["rename", "tag"]))
        .into()
}

fn cleanup_confidence_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["high", "low"]))
        .into()
}

fn cleanup_value_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(openapi_integer())
            .item(ObjectBuilder::new().schema_type(Type::String))
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}
