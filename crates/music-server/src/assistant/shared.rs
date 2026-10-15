//! Wire enums, scope types and helpers shared by the assistant routes.

use axum::Json;
use axum::http::StatusCode;
use music_application::ErrorChain;
use music_application::assistant::{
    AnalysisReviewDecision, AssistantService, AssistantServiceError, Confidence, EnergyCurve,
    LocalAnalysisError, LocalAnalysisService,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::analysis::{ContextScopeKind, ContextScopeParameters};
use crate::error::ApiError;
use crate::http::HttpState;
use crate::jobs::{BackgroundJobResponse, job_response, map_job_error};
use crate::provider_api::{map_model_quality_error, map_provider_error};

use super::schema::{context_scope_kind_schema, context_track_ids_schema};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(super) enum ConfidenceWire {
    High,
    Medium,
    Low,
}

impl From<Confidence> for ConfidenceWire {
    fn from(value: Confidence) -> Self {
        match value {
            Confidence::High => Self::High,
            Confidence::Medium => Self::Medium,
            Confidence::Low => Self::Low,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(super) enum ReviewStatusWire {
    #[default]
    Pending,
    Accepted,
    Rejected,
}

impl From<ReviewStatusWire> for AnalysisReviewDecision {
    fn from(value: ReviewStatusWire) -> Self {
        match value {
            ReviewStatusWire::Pending => Self::Pending,
            ReviewStatusWire::Accepted => Self::Accepted,
            ReviewStatusWire::Rejected => Self::Rejected,
        }
    }
}

impl From<AnalysisReviewDecision> for ReviewStatusWire {
    fn from(value: AnalysisReviewDecision) -> Self {
        match value {
            AnalysisReviewDecision::Pending => Self::Pending,
            AnalysisReviewDecision::Accepted => Self::Accepted,
            AnalysisReviewDecision::Rejected => Self::Rejected,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(super) enum EnergyCurveWire {
    #[default]
    Steady,
    Rising,
    Falling,
    Arc,
}

impl From<EnergyCurveWire> for EnergyCurve {
    fn from(value: EnergyCurveWire) -> Self {
        match value {
            EnergyCurveWire::Steady => Self::Steady,
            EnergyCurveWire::Rising => Self::Rising,
            EnergyCurveWire::Falling => Self::Falling,
            EnergyCurveWire::Arc => Self::Arc,
        }
    }
}

impl From<EnergyCurve> for EnergyCurveWire {
    fn from(value: EnergyCurve) -> Self {
        match value {
            EnergyCurve::Steady => Self::Steady,
            EnergyCurve::Rising => Self::Rising,
            EnergyCurve::Falling => Self::Falling,
            EnergyCurve::Arc => Self::Arc,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
#[schema(as = ModelTaggingScope)]
pub(crate) struct ModelTaggingScopeWire {
    #[serde(rename = "type")]
    #[schema(required = false, schema_with = context_scope_kind_schema)]
    pub(super) kind: ContextScopeKindWire,
    #[schema(required = false, max_length = 1024, default = "")]
    pub(super) path: String,
    #[schema(required = false, default = true)]
    pub(super) recursive: bool,
    #[schema(required = false, schema_with = context_track_ids_schema)]
    pub(super) track_ids: Vec<i64>,
}

impl Default for ModelTaggingScopeWire {
    fn default() -> Self {
        Self {
            kind: ContextScopeKindWire::All,
            path: String::new(),
            recursive: true,
            track_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum ContextScopeKindWire {
    #[default]
    All,
    Folder,
    Tracks,
}

impl ModelTaggingScopeWire {
    pub(super) fn into_parameters(mut self) -> Result<ContextScopeParameters, ApiError> {
        if self.path.len() > 1_024 || self.track_ids.len() > 5_000 {
            return Err(ApiError::validation());
        }
        let raw_path = self.path.trim();
        if raw_path.starts_with(['/', '\\'])
            || raw_path
                .as_bytes()
                .get(1)
                .is_some_and(|value| *value == b':')
        {
            return Err(ApiError::validation());
        }
        let normalized_path = raw_path.replace('\\', "/").trim_matches('/').to_owned();
        if normalized_path
            .split('/')
            .filter(|part| !part.is_empty())
            .any(|part| matches!(part, "." | ".."))
        {
            return Err(ApiError::validation());
        }
        let mut seen = std::collections::BTreeSet::new();
        self.track_ids.retain(|value| seen.insert(*value));
        if self.track_ids.iter().any(|value| *value <= 0) {
            return Err(ApiError::validation());
        }
        let kind = match self.kind {
            ContextScopeKindWire::All => ContextScopeKind::All,
            ContextScopeKindWire::Folder => ContextScopeKind::Folder,
            ContextScopeKindWire::Tracks => ContextScopeKind::Tracks,
        };
        match kind {
            ContextScopeKind::Tracks if self.track_ids.is_empty() => {
                return Err(ApiError::validation());
            }
            ContextScopeKind::All | ContextScopeKind::Folder if !self.track_ids.is_empty() => {
                return Err(ApiError::validation());
            }
            ContextScopeKind::All | ContextScopeKind::Tracks if !normalized_path.is_empty() => {
                return Err(ApiError::validation());
            }
            _ => {}
        }
        Ok(ContextScopeParameters {
            kind,
            path: normalized_path,
            recursive: self.recursive,
            track_ids: self.track_ids,
        })
    }
}

pub(super) struct ModelRoleAvailability {
    pub(super) reason_code: Option<String>,
    pub(super) connection_name: Option<String>,
    pub(super) model_id: Option<String>,
    pub(super) inference_fingerprint: Option<String>,
    pub(super) execution: Option<music_application::assistant::ResolvedRoleExecution>,
}

pub(super) async fn model_role_availability(
    state: &HttpState,
    role_id: &str,
    evaluation_id: &str,
) -> Result<ModelRoleAvailability, ApiError> {
    let providers = state
        .providers
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)?;
    let role = providers
        .provider_service()
        .list_model_roles()
        .await
        .map_err(map_provider_error)?
        .into_iter()
        .find(|role| role.role_id == role_id);
    let (reason_code, inference_fingerprint, execution) = match providers
        .quality_service()
        .prepare_quality_gated_role_execution(role_id, evaluation_id)
        .await
    {
        Ok(execution) => (
            None,
            Some(execution.inference_fingerprint.clone()),
            Some(execution),
        ),
        Err(error)
            if error.kind()
                == music_application::assistant::ProviderServiceErrorKind::Dependency =>
        {
            return Err(map_model_quality_error(error));
        }
        Err(error) => {
            let fingerprint = providers
                .provider_service()
                .current_role_review_identity(role_id)
                .await
                .map_err(map_provider_error)?;
            (
                Some(error.code().to_owned()),
                fingerprint.map(|identity| identity.inference_fingerprint),
                None,
            )
        }
    };
    Ok(ModelRoleAvailability {
        connection_name: role.as_ref().and_then(|role| role.connection_name.clone()),
        model_id: role
            .filter(|role| role.configuration_available)
            .map(|role| role.model_id),
        reason_code,
        inference_fingerprint,
        execution,
    })
}

pub(super) async fn enqueue_model_feature(
    state: &HttpState,
    job_kind: &'static str,
    parameters: Value,
    conflict_code: &'static str,
    conflict_message: &'static str,
) -> Result<(StatusCode, Json<BackgroundJobResponse>), ApiError> {
    let expected = parameters
        .as_object()
        .cloned()
        .ok_or_else(ApiError::validation)?;
    let jobs = state
        .jobs
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)?;
    let (job, created) = jobs
        .enqueue_unique_active(job_kind, Value::Object(expected.clone()))
        .await
        .map_err(map_job_error)?;
    if !created && job.parameters != expected {
        return Err(ApiError::coded_conflict(conflict_code, conflict_message));
    }
    Ok((StatusCode::ACCEPTED, Json(job_response(job)?)))
}

pub(super) fn service(state: &HttpState) -> Result<&AssistantService, ApiError> {
    state
        .assistant
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)
}

pub(super) fn analysis_service(state: &HttpState) -> Result<&LocalAnalysisService, ApiError> {
    state
        .local_analysis
        .as_deref()
        .ok_or_else(ApiError::service_unavailable)
}

pub(super) fn map_local_analysis_error(error: LocalAnalysisError) -> ApiError {
    tracing::error!(error = %ErrorChain(&error), "Local analysis request failed");
    ApiError::internal()
}

pub(super) fn map_assistant_error(error: AssistantServiceError) -> ApiError {
    match error {
        AssistantServiceError::Validation(_) | AssistantServiceError::Vocabulary(_) => {
            ApiError::validation()
        }
        AssistantServiceError::NotFound(message) => ApiError::not_found_message(message),
        AssistantServiceError::Conflict(message) | AssistantServiceError::Stale(message) => {
            ApiError::conflict_message(message)
        }
        AssistantServiceError::InvalidCleanupSelection(message) => {
            ApiError::unprocessable_message(message)
        }
        AssistantServiceError::Dependency(source) => {
            tracing::error!(error = %ErrorChain(source.as_ref()), "Assistant request failed");
            ApiError::internal()
        }
    }
}

pub(super) const fn default_target_minutes() -> u16 {
    60
}
pub(super) const fn default_candidate_limit() -> u16 {
    40
}
pub(super) const fn default_page_limit() -> usize {
    50
}
pub(super) const fn default_true() -> bool {
    true
}

pub(super) fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
