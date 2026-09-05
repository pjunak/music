use std::fmt::{self, Display, Formatter};
use std::path::Path;
use std::sync::Arc;

use music_application::assistant::{
    MODEL_PLAYLIST_ENGINE_ID, ModelPlaylistTask, ModelTaskError, PlaylistQualityEvaluationResult,
    PlaylistQualitySuite, PlaylistSuggestion, ProviderConnectionPolicy, ProviderCredentialSource,
    ProviderRepository, ProviderService, ResolvedRoleExecution,
};
use music_storage::{SqliteStorage, SqliteStorageOptions};

use crate::AppConfig;
use crate::provider_api::provider_runtime_contract_digest;
use crate::provider_credentials::RuntimeCredentialStore;
use crate::provider_transport::ProviderNetworkBoundary;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ConfiguredPlaylistEvaluationError {
    code: String,
}

impl ConfiguredPlaylistEvaluationError {
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    fn new(code: impl Into<String>) -> Self {
        Self { code: code.into() }
    }
}

impl Display for ConfiguredPlaylistEvaluationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.code)
    }
}

impl std::error::Error for ConfiguredPlaylistEvaluationError {}

pub async fn evaluate_configured_playlist_suite(
    config: &AppConfig,
    database_path: &Path,
    suite: &PlaylistQualitySuite,
) -> Result<PlaylistQualityEvaluationResult, ConfiguredPlaylistEvaluationError> {
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(database_path))
            .await
            .map_err(|_| ConfiguredPlaylistEvaluationError::new("provider_storage_failed"))?,
    );
    let credentials = Arc::new(RuntimeCredentialStore::new(config));
    let network = Arc::new(ProviderNetworkBoundary::new());
    let repository: Arc<dyn ProviderRepository> = storage.clone();
    let credential_source: Arc<dyn ProviderCredentialSource> = credentials;
    let policy: Arc<dyn ProviderConnectionPolicy> = network.clone();
    let providers = ProviderService::new(
        repository,
        credential_source,
        policy,
        provider_runtime_contract_digest(),
    )
    .with_role_contract_digests(crate::provider_api::provider_role_contract_digests());
    let role = providers
        .prepare_role_execution("playlist_planner")
        .await
        .map_err(|error| ConfiguredPlaylistEvaluationError::new(error.code()))?;

    let mut cases = Vec::with_capacity(suite.cases.len());
    for case in &suite.cases {
        let task = case
            .task()
            .map_err(|error| ConfiguredPlaylistEvaluationError::new(error.code))?;
        let first = execute_task(&network, &role, &task).await;
        let repeated = if case.requires_repeat() {
            Some(execute_task(&network, &role, &task).await)
        } else {
            None
        };
        cases.push(case.assess(first, repeated));
    }
    PlaylistQualityEvaluationResult::from_cases(suite, MODEL_PLAYLIST_ENGINE_ID, cases)
        .map_err(|error| ConfiguredPlaylistEvaluationError::new(error.code))
}

async fn execute_task(
    network: &ProviderNetworkBoundary,
    role: &ResolvedRoleExecution,
    task: &ModelPlaylistTask,
) -> Result<PlaylistSuggestion, ModelTaskError> {
    if let Some(result) = task.immediate_result() {
        return Ok(result);
    }
    let request = task
        .request()
        .ok_or_else(|| ModelTaskError::new("playlist_model_task_incomplete"))?;
    let result = network
        .execute_structured_model_request(&role.execution, &request)
        .await;
    task.finish(result)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use music_application::assistant::playlist_quality_suite;

    use super::*;

    #[tokio::test]
    async fn refuses_an_unconfigured_playlist_role_before_any_network_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("music.db");
        let config = AppConfig::from_values(&BTreeMap::from([(
            "DATABASE_URL".to_owned(),
            format!("sqlite:///{}", database.display()),
        )]))?;
        let error =
            evaluate_configured_playlist_suite(&config, &database, &playlist_quality_suite()?)
                .await
                .err()
                .ok_or("configured evaluation unexpectedly succeeded")?;
        assert_eq!(error.code(), "role_not_enabled");
        Ok(())
    }
}
