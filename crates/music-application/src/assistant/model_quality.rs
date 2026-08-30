use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use super::{
    AssistantDependencyError, AssistantFuture, ProviderService, ProviderServiceError,
    ProviderServiceErrorKind, ResolvedRoleExecution, model_role,
};

pub const PLAYLIST_QUALITY_EVALUATION_ID: &str = "playlist-quality-v1";
pub const TAGGING_QUALITY_EVALUATION_ID: &str = "music-tagging-quality-v1";
pub const TAG_CLEANUP_QUALITY_EVALUATION_ID: &str = "tag-cleanup-quality-v1";
pub const EQ_QUALITY_EVALUATION_ID: &str = "eq-quality-v1";

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ModelEvaluationDefinition {
    pub id: &'static str,
    pub role_id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub suite_id: &'static str,
    pub job_kind: &'static str,
}

pub const MODEL_EVALUATIONS: &[ModelEvaluationDefinition] = &[
    ModelEvaluationDefinition {
        id: PLAYLIST_QUALITY_EVALUATION_ID,
        role_id: "playlist_planner",
        label: "Playlist planning quality",
        description: "Runs fixed synthetic D&D playlist scenarios through this model. No songs or live library data are sent.",
        suite_id: "model-dnd-playlist-quality-v5",
        job_kind: "assistant.model-evaluation.playlist-quality-v1",
    },
    ModelEvaluationDefinition {
        id: TAGGING_QUALITY_EVALUATION_ID,
        role_id: "music_tagger",
        label: "Mood tagging quality",
        description: "Runs fixed synthetic metadata and signal-evidence cases against the server-owned tag vocabulary. No songs or live library data are sent.",
        suite_id: "controlled-vocabulary-tagging-baseline-v18",
        job_kind: "assistant.model-evaluation.music-tagging-quality-v1",
    },
    ModelEvaluationDefinition {
        id: TAG_CLEANUP_QUALITY_EVALUATION_ID,
        role_id: "tag_cleanup",
        label: "Mood-tag cleanup quality",
        description: "Runs fixed synthetic canonical-ID and no-match tag-cleanup cases through this model. No songs or live library data are sent.",
        suite_id: "controlled-vocabulary-cleanup-baseline-v6",
        job_kind: "assistant.model-evaluation.tag-cleanup-quality-v1",
    },
    ModelEvaluationDefinition {
        id: EQ_QUALITY_EVALUATION_ID,
        role_id: "eq_assistant",
        label: "EQ draft quality",
        description: "Runs fixed synthetic sound goals through this model and checks bounded, conservative graphic-EQ behavior. No songs or live presets are sent.",
        suite_id: "graphic-eq-safety-baseline-v4",
        job_kind: "assistant.model-evaluation.eq-quality-v1",
    },
];

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ModelEvaluationRecord {
    pub role_id: String,
    pub evaluation_id: String,
    pub role_fingerprint: String,
    pub status: String,
    pub suite_id: String,
    pub engine_id: String,
    pub passed_cases: u32,
    pub total_cases: u32,
    pub job_id: String,
    pub evaluated_at_unix_seconds: i64,
}

pub trait ModelEvaluationRepository: std::fmt::Debug + Send + Sync {
    fn model_evaluations<'a>(
        &'a self,
        role_id: &'a str,
    ) -> AssistantFuture<'a, Vec<ModelEvaluationRecord>>;
    fn save_model_evaluation<'a>(
        &'a self,
        evaluation: &'a ModelEvaluationWrite,
    ) -> AssistantFuture<'a, ModelEvaluationWriteOutcome>;
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ModelEvaluationWrite {
    pub role_id: String,
    pub evaluation_id: String,
    pub expected_role_configuration_fingerprint: String,
    pub expected_connection_fingerprint: String,
    pub role_fingerprint: String,
    pub status: String,
    pub suite_id: String,
    pub engine_id: String,
    pub passed_cases: u32,
    pub total_cases: u32,
    pub job_id: String,
    pub job_kind: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ModelEvaluationWriteOutcome {
    Applied,
    RoleChanged,
    JobInactive,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ModelEvaluationStatus {
    Never,
    Passed,
    Failed,
    Stale,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ModelEvaluationView {
    pub evaluation_id: String,
    pub role_id: String,
    pub label: String,
    pub description: String,
    pub status: ModelEvaluationStatus,
    pub suite_id: String,
    pub passed_cases: u32,
    pub total_cases: u32,
    pub last_job_id: Option<String>,
    pub last_evaluated_at_unix_seconds: Option<i64>,
}

#[derive(Debug)]
pub enum ModelQualityError {
    Provider(ProviderServiceError),
    Dependency(AssistantDependencyError),
}

impl ModelQualityError {
    #[must_use]
    pub const fn kind(&self) -> ProviderServiceErrorKind {
        match self {
            Self::Provider(error) => error.kind(),
            Self::Dependency(_) => ProviderServiceErrorKind::Dependency,
        }
    }

    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Provider(error) => error.code(),
            Self::Dependency(_) => "provider_storage_failed",
        }
    }

    #[must_use]
    pub const fn message(&self) -> &'static str {
        match self {
            Self::Provider(error) => error.message(),
            Self::Dependency(_) => "Provider storage failed.",
        }
    }
}

impl Display for ModelQualityError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for ModelQualityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Provider(error) => Some(error),
            Self::Dependency(error) => Some(error.as_ref()),
        }
    }
}

impl From<ProviderServiceError> for ModelQualityError {
    fn from(error: ProviderServiceError) -> Self {
        Self::Provider(error)
    }
}

#[derive(Debug)]
pub struct ModelQualityService {
    repository: Arc<dyn ModelEvaluationRepository>,
    providers: Arc<ProviderService>,
}

#[derive(Debug)]
pub struct ModelEvaluationExecution {
    pub definition: ModelEvaluationDefinition,
    pub role: ResolvedRoleExecution,
}

impl ModelQualityService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn ModelEvaluationRepository>,
        providers: Arc<ProviderService>,
    ) -> Self {
        Self {
            repository,
            providers,
        }
    }

    pub async fn list_role_evaluations(
        &self,
        role_id: &str,
    ) -> Result<Vec<ModelEvaluationView>, ModelQualityError> {
        if model_role(role_id).is_none() {
            return Err(self.providers.role_not_found_error().into());
        }
        let definitions = MODEL_EVALUATIONS
            .iter()
            .filter(|definition| definition.role_id == role_id)
            .collect::<Vec<_>>();
        if definitions.is_empty() {
            return Ok(Vec::new());
        }
        let current_fingerprint = self
            .providers
            .current_role_runtime_fingerprint(role_id)
            .await?;
        let records = self
            .repository
            .model_evaluations(role_id)
            .await
            .map_err(ModelQualityError::Dependency)?;
        Ok(definitions
            .into_iter()
            .map(|definition| {
                let record = records
                    .iter()
                    .find(|record| record.evaluation_id == definition.id);
                evaluation_view(definition, record, current_fingerprint.as_deref())
            })
            .collect())
    }

    pub fn evaluation_definition(
        &self,
        role_id: &str,
        evaluation_id: &str,
    ) -> Result<ModelEvaluationDefinition, ModelQualityError> {
        if model_role(role_id).is_none() {
            return Err(self.providers.role_not_found_error().into());
        }
        MODEL_EVALUATIONS
            .iter()
            .copied()
            .find(|definition| definition.role_id == role_id && definition.id == evaluation_id)
            .ok_or_else(|| {
                ProviderServiceError::public(
                    ProviderServiceErrorKind::NotFound,
                    "evaluation_not_found",
                    "Model quality evaluation not found for this role.",
                )
                .into()
            })
    }

    pub async fn prepare_evaluation_execution(
        &self,
        role_id: &str,
        evaluation_id: &str,
    ) -> Result<ModelEvaluationExecution, ModelQualityError> {
        let definition = self.evaluation_definition(role_id, evaluation_id)?;
        let role = self.providers.prepare_role_execution(role_id).await?;
        Ok(ModelEvaluationExecution { definition, role })
    }

    pub async fn prepare_quality_gated_role_execution(
        &self,
        role_id: &str,
        evaluation_id: &str,
    ) -> Result<ResolvedRoleExecution, ModelQualityError> {
        let execution = self
            .prepare_evaluation_execution(role_id, evaluation_id)
            .await?;
        let records = self
            .repository
            .model_evaluations(role_id)
            .await
            .map_err(ModelQualityError::Dependency)?;
        let passed = records.iter().any(|record| {
            record.evaluation_id == evaluation_id
                && record.status == "passed"
                && record.suite_id == execution.definition.suite_id
                && record.role_fingerprint == execution.role.fingerprint
        });
        if !passed {
            return Err(ProviderServiceError::public(
                ProviderServiceErrorKind::Conflict,
                "model_quality_not_passed",
                "Run and pass the current model quality check before using live library data.",
            )
            .into());
        }
        Ok(execution.role)
    }

    pub async fn prepare_failed_scenario_retest(
        &self,
        role_id: &str,
        evaluation_id: &str,
    ) -> Result<(ModelEvaluationExecution, ModelEvaluationRecord), ModelQualityError> {
        if evaluation_id != TAGGING_QUALITY_EVALUATION_ID {
            return Err(ProviderServiceError::public(
                ProviderServiceErrorKind::Conflict,
                "evaluation_partial_retest_unavailable",
                "Failed-scenario retesting is currently available for mood tagging.",
            )
            .into());
        }
        let execution = self
            .prepare_evaluation_execution(role_id, evaluation_id)
            .await?;
        let record = self
            .repository
            .model_evaluations(role_id)
            .await
            .map_err(ModelQualityError::Dependency)?
            .into_iter()
            .find(|record| {
                record.evaluation_id == evaluation_id
                    && record.suite_id == execution.definition.suite_id
                    && record.role_fingerprint == execution.role.fingerprint
                    && !record.job_id.is_empty()
            })
            .ok_or_else(|| {
                ProviderServiceError::public(
                    ProviderServiceErrorKind::Conflict,
                    "evaluation_retest_baseline_unavailable",
                    "Run the complete current quality suite before rechecking failures.",
                )
            })?;
        Ok((execution, record))
    }

    pub async fn record_evaluation(
        &self,
        execution: &ModelEvaluationExecution,
        job_id: &str,
        engine_id: &str,
        passed: bool,
        passed_cases: u32,
        total_cases: u32,
    ) -> Result<(), ModelQualityError> {
        let outcome = self
            .repository
            .save_model_evaluation(&ModelEvaluationWrite {
                role_id: execution.role.role_id.clone(),
                evaluation_id: execution.definition.id.to_owned(),
                expected_role_configuration_fingerprint: execution
                    .role
                    .role_configuration_fingerprint
                    .clone(),
                expected_connection_fingerprint: execution.role.connection_fingerprint.clone(),
                role_fingerprint: execution.role.fingerprint.clone(),
                status: if passed {
                    "passed".to_owned()
                } else {
                    "failed".to_owned()
                },
                suite_id: execution.definition.suite_id.to_owned(),
                engine_id: engine_id.to_owned(),
                passed_cases,
                total_cases,
                job_id: job_id.to_owned(),
                job_kind: execution.definition.job_kind.to_owned(),
            })
            .await
            .map_err(ModelQualityError::Dependency)?;
        match outcome {
            ModelEvaluationWriteOutcome::Applied => Ok(()),
            ModelEvaluationWriteOutcome::RoleChanged => Err(ProviderServiceError::public(
                ProviderServiceErrorKind::Conflict,
                "role_changed",
                "The model role changed during evaluation. Run it again.",
            )
            .into()),
            ModelEvaluationWriteOutcome::JobInactive => Err(ProviderServiceError::public(
                ProviderServiceErrorKind::Conflict,
                "evaluation_job_inactive",
                "The quality job is no longer active and cannot certify this result.",
            )
            .into()),
        }
    }
}

fn evaluation_view(
    definition: &ModelEvaluationDefinition,
    record: Option<&ModelEvaluationRecord>,
    current_fingerprint: Option<&str>,
) -> ModelEvaluationView {
    let status = match record {
        None => ModelEvaluationStatus::Never,
        Some(record)
            if current_fingerprint != Some(record.role_fingerprint.as_str())
                || record.suite_id != definition.suite_id =>
        {
            ModelEvaluationStatus::Stale
        }
        Some(record) if record.status == "passed" => ModelEvaluationStatus::Passed,
        Some(record) if record.status == "failed" => ModelEvaluationStatus::Failed,
        Some(_) => ModelEvaluationStatus::Never,
    };
    ModelEvaluationView {
        evaluation_id: definition.id.to_owned(),
        role_id: definition.role_id.to_owned(),
        label: definition.label.to_owned(),
        description: definition.description.to_owned(),
        status,
        suite_id: definition.suite_id.to_owned(),
        passed_cases: record.map_or(0, |record| record.passed_cases),
        total_cases: record.map_or(0, |record| record.total_cases),
        last_job_id: record.map(|record| record.job_id.clone()),
        last_evaluated_at_unix_seconds: record.map(|record| record.evaluated_at_unix_seconds),
    }
}

#[cfg(test)]
mod tests {
    use super::{MODEL_EVALUATIONS, ModelEvaluationRecord, ModelEvaluationStatus, evaluation_view};

    fn record(status: &str, fingerprint: &str, suite_id: &str) -> ModelEvaluationRecord {
        ModelEvaluationRecord {
            role_id: "music_tagger".to_owned(),
            evaluation_id: "music-tagging-quality-v1".to_owned(),
            role_fingerprint: fingerprint.to_owned(),
            status: status.to_owned(),
            suite_id: suite_id.to_owned(),
            engine_id: "model-context-tagger/v6".to_owned(),
            passed_cases: 11,
            total_cases: 13,
            job_id: "1234567890abcdef1234567890abcdef".to_owned(),
            evaluated_at_unix_seconds: 1_700_000_000,
        }
    }

    #[test]
    fn evaluation_state_is_bound_to_runtime_and_suite_fingerprints() -> Result<(), &'static str> {
        let definition = MODEL_EVALUATIONS
            .iter()
            .find(|definition| definition.role_id == "music_tagger")
            .ok_or("missing tagging evaluation definition")?;

        let never = evaluation_view(definition, None, Some("current"));
        assert_eq!(never.status, ModelEvaluationStatus::Never);
        assert_eq!(never.passed_cases, 0);
        assert!(never.last_job_id.is_none());

        let passed_record = record("passed", "current", definition.suite_id);
        let passed = evaluation_view(definition, Some(&passed_record), Some("current"));
        assert_eq!(passed.status, ModelEvaluationStatus::Passed);
        assert_eq!(passed.passed_cases, 11);
        assert_eq!(passed.total_cases, 13);

        let stale_runtime = evaluation_view(definition, Some(&passed_record), Some("changed"));
        assert_eq!(stale_runtime.status, ModelEvaluationStatus::Stale);

        let stale_suite_record = record("failed", "current", "older-suite");
        let stale_suite = evaluation_view(definition, Some(&stale_suite_record), Some("current"));
        assert_eq!(stale_suite.status, ModelEvaluationStatus::Stale);

        let failed_record = record("failed", "current", definition.suite_id);
        let failed = evaluation_view(definition, Some(&failed_record), Some("current"));
        assert_eq!(failed.status, ModelEvaluationStatus::Failed);
        Ok(())
    }

    #[test]
    fn every_quality_definition_targets_a_known_unique_role_evaluation_pair() {
        let pairs = MODEL_EVALUATIONS
            .iter()
            .map(|definition| (definition.role_id, definition.id))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(pairs.len(), MODEL_EVALUATIONS.len());
        assert!(MODEL_EVALUATIONS.iter().all(|definition| {
            super::model_role(definition.role_id).is_some()
                && !definition.suite_id.is_empty()
                && definition
                    .job_kind
                    .starts_with("assistant.model-evaluation.")
        }));
    }
}
