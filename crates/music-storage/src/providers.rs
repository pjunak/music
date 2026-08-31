use std::collections::BTreeSet;

use music_application::assistant::{
    AssistantFuture, ModelEvaluationRecord, ModelEvaluationRepository, ModelEvaluationWrite,
    ModelEvaluationWriteOutcome, ModelRoleRecord, ProviderConformanceWrite,
    ProviderConformanceWriteOutcome, ProviderConnectionPreparation, ProviderConnectionRecord,
    ProviderCredentialResetOutcome, ProviderMutationOutcome, ProviderRepository,
    ProviderRolePreparation, ProviderRoleRuntimeRecord, ProviderVerificationWrite,
    ProviderVerificationWriteOutcome,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::sqlite::SqliteRow;
use sqlx::{AssertSqlSafe, Row, Sqlite, Transaction};

use crate::{CredentialVault, EncryptedCredential, SqliteStorage, StorageError};

const CONNECTION_SELECT: &str = "SELECT id, name, adapter_id, base_url, encrypted_api_key, \
    api_key_nonce, api_key_hint, allow_private_network, verification_status, \
    verification_error_code, verified_models_json, verified_capabilities_json, \
    CAST(strftime('%s', last_verified_at) AS INTEGER) AS last_verified_at_unix_seconds, \
    CAST(strftime('%s', created_at) AS INTEGER) AS created_at_unix_seconds, \
    CAST(strftime('%s', updated_at) AS INTEGER) AS updated_at_unix_seconds \
    FROM assistant_provider_connections";

const ROLE_SELECT: &str = "SELECT role_id, connection_id, model_id, enabled, timeout_seconds, \
    max_output_tokens, thinking_mode, conformance_status, conformance_error_code, \
    conformance_fingerprint, \
    CAST(strftime('%s', last_conformance_at) AS INTEGER) AS last_conformance_at_unix_seconds, \
    CAST(strftime('%s', updated_at) AS INTEGER) AS updated_at_unix_seconds \
    FROM assistant_model_roles";

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderCredentialAudit {
    pub key_id: String,
    pub total_connections: u64,
    pub saved_credentials: u64,
    pub connections_without_credentials: u64,
    pub unreadable_credentials: u64,
}

impl ProviderCredentialAudit {
    #[must_use]
    pub const fn healthy(&self) -> bool {
        self.unreadable_credentials == 0
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProviderCredentialRotationOutcome {
    Applied { rotated_credentials: u64 },
    UnreadableCredentials { unreadable_credentials: u64 },
    ModelJobActive,
    ChangedDuringPreflight,
}

#[derive(Debug)]
struct CredentialRotation {
    id: String,
    original_ciphertext: String,
    original_nonce: String,
    replacement: EncryptedCredential,
}

impl SqliteStorage {
    pub async fn audit_provider_credentials(
        &self,
        vault: &CredentialVault,
    ) -> Result<ProviderCredentialAudit, StorageError> {
        let rows = sqlx::query(
            "SELECT id, encrypted_api_key, api_key_nonce FROM assistant_provider_connections \
             ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        let total_connections = u64::try_from(rows.len()).map_err(|_| {
            StorageError::InvalidAssistantRecord("invalid provider connection count")
        })?;
        let mut saved_credentials = 0_u64;
        let mut connections_without_credentials = 0_u64;
        let mut unreadable_credentials = 0_u64;
        for row in rows {
            let id: String = row.try_get("id")?;
            let ciphertext: String = row.try_get("encrypted_api_key")?;
            let nonce: String = row.try_get("api_key_nonce")?;
            if ciphertext.is_empty() && nonce.is_empty() {
                connections_without_credentials += 1;
            } else if ciphertext.is_empty()
                || nonce.is_empty()
                || vault.decrypt(&id, &ciphertext, &nonce).is_err()
            {
                unreadable_credentials += 1;
            } else {
                saved_credentials += 1;
            }
        }
        Ok(ProviderCredentialAudit {
            key_id: vault.key_id().to_owned(),
            total_connections,
            saved_credentials,
            connections_without_credentials,
            unreadable_credentials,
        })
    }

    pub async fn rotate_provider_credentials(
        &self,
        current: &CredentialVault,
        replacement: &CredentialVault,
    ) -> Result<ProviderCredentialRotationOutcome, StorageError> {
        let rows = sqlx::query(
            "SELECT id, encrypted_api_key, api_key_nonce FROM assistant_provider_connections \
             ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut unreadable_credentials = 0_u64;
        let mut rotations = Vec::new();
        for row in rows {
            let id: String = row.try_get("id")?;
            let ciphertext: String = row.try_get("encrypted_api_key")?;
            let nonce: String = row.try_get("api_key_nonce")?;
            if ciphertext.is_empty() && nonce.is_empty() {
                continue;
            }
            if ciphertext.is_empty() || nonce.is_empty() {
                unreadable_credentials += 1;
                continue;
            }
            let Ok(secret) = current.decrypt(&id, &ciphertext, &nonce) else {
                unreadable_credentials += 1;
                continue;
            };
            let encrypted = replacement
                .encrypt(&id, secret.expose_secret())
                .map_err(StorageError::CredentialCrypto)?;
            rotations.push(CredentialRotation {
                id,
                original_ciphertext: ciphertext,
                original_nonce: nonce,
                replacement: encrypted,
            });
        }
        if unreadable_credentials > 0 {
            return Ok(ProviderCredentialRotationOutcome::UnreadableCredentials {
                unreadable_credentials,
            });
        }

        let _admission = self.write_gate.lock().await;
        let mut transaction = self.pool.begin().await?;
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM background_jobs \
             WHERE kind LIKE 'assistant.model%' \
             AND status IN ('queued', 'running', 'cancel_requested') LIMIT 1)",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if active {
            transaction.rollback().await?;
            return Ok(ProviderCredentialRotationOutcome::ModelJobActive);
        }
        let current_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM assistant_provider_connections \
             WHERE encrypted_api_key != '' OR api_key_nonce != ''",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if usize::try_from(current_count).ok() != Some(rotations.len()) {
            transaction.rollback().await?;
            return Ok(ProviderCredentialRotationOutcome::ChangedDuringPreflight);
        }
        for rotation in &rotations {
            let result = sqlx::query(
                "UPDATE assistant_provider_connections SET encrypted_api_key = ?, \
                 api_key_nonce = ?, api_key_hint = ?, verification_status = 'never', \
                 verification_error_code = NULL, verified_models_json = '[]', \
                 verified_capabilities_json = '[]', last_verified_at = NULL, \
                 updated_at = CURRENT_TIMESTAMP \
                 WHERE id = ? AND encrypted_api_key = ? AND api_key_nonce = ?",
            )
            .bind(&rotation.replacement.ciphertext)
            .bind(&rotation.replacement.nonce)
            .bind(&rotation.replacement.hint)
            .bind(&rotation.id)
            .bind(&rotation.original_ciphertext)
            .bind(&rotation.original_nonce)
            .execute(&mut *transaction)
            .await?;
            if result.rows_affected() != 1 {
                transaction.rollback().await?;
                return Ok(ProviderCredentialRotationOutcome::ChangedDuringPreflight);
            }
        }
        sqlx::query("DELETE FROM assistant_model_evaluations")
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "UPDATE assistant_model_roles SET conformance_status = 'never', \
             conformance_error_code = NULL, conformance_fingerprint = NULL, \
             last_conformance_at = NULL, updated_at = CURRENT_TIMESTAMP",
        )
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        let rotated_credentials = u64::try_from(rotations.len()).map_err(|_| {
            StorageError::InvalidAssistantRecord("invalid rotated credential count")
        })?;
        Ok(ProviderCredentialRotationOutcome::Applied {
            rotated_credentials,
        })
    }
}

impl ModelEvaluationRepository for SqliteStorage {
    fn model_evaluations<'a>(
        &'a self,
        role_id: &'a str,
    ) -> AssistantFuture<'a, Vec<ModelEvaluationRecord>> {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT role_id, evaluation_id, role_fingerprint, status, suite_id, engine_id, \
                 passed_cases, total_cases, job_id, \
                 CAST(strftime('%s', evaluated_at) AS INTEGER) AS evaluated_at_unix_seconds \
                 FROM assistant_model_evaluations WHERE role_id = ? ORDER BY evaluation_id",
            )
            .bind(role_id)
            .fetch_all(&self.pool)
            .await
            .map_err(box_storage)?;
            rows.iter()
                .map(evaluation_from_row)
                .collect::<Result<Vec<_>, _>>()
                .map_err(box_storage)
        })
    }

    fn save_model_evaluation<'a>(
        &'a self,
        evaluation: &'a ModelEvaluationWrite,
    ) -> AssistantFuture<'a, ModelEvaluationWriteOutcome> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let Some(role) = load_role_tx(&mut transaction, &evaluation.role_id)
                .await
                .map_err(box_storage)?
            else {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ModelEvaluationWriteOutcome::RoleChanged);
            };
            if role.configuration_fingerprint()
                != evaluation.expected_role_configuration_fingerprint
            {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ModelEvaluationWriteOutcome::RoleChanged);
            }
            let Some(connection) = load_connection_tx(&mut transaction, &role.connection_id)
                .await
                .map_err(box_storage)?
            else {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ModelEvaluationWriteOutcome::RoleChanged);
            };
            if connection.fingerprint() != evaluation.expected_connection_fingerprint {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ModelEvaluationWriteOutcome::RoleChanged);
            }
            let job = sqlx::query(
                "SELECT kind, status, parameters_json FROM background_jobs WHERE id = ?",
            )
            .bind(&evaluation.job_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(box_storage)?;
            let job_is_current = job.is_some_and(|row| {
                row.try_get::<String, _>("kind").ok().as_deref()
                    == Some(evaluation.job_kind.as_str())
                    && row.try_get::<String, _>("status").ok().as_deref() == Some("running")
                    && row
                        .try_get::<String, _>("parameters_json")
                        .ok()
                        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                        .is_some_and(|parameters| {
                            parameters.get("role_id").and_then(Value::as_str)
                                == Some(evaluation.role_id.as_str())
                                && parameters.get("evaluation_id").and_then(Value::as_str)
                                    == Some(evaluation.evaluation_id.as_str())
                                && parameters.get("role_fingerprint").and_then(Value::as_str)
                                    == Some(evaluation.role_fingerprint.as_str())
                        })
            });
            if !job_is_current {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ModelEvaluationWriteOutcome::JobInactive);
            }
            sqlx::query(
                "INSERT INTO assistant_model_evaluations \
                 (role_id, evaluation_id, role_fingerprint, status, suite_id, engine_id, \
                  passed_cases, total_cases, job_id, evaluated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP) \
                 ON CONFLICT(role_id, evaluation_id) DO UPDATE SET \
                  role_fingerprint = excluded.role_fingerprint, status = excluded.status, \
                  suite_id = excluded.suite_id, engine_id = excluded.engine_id, \
                  passed_cases = excluded.passed_cases, total_cases = excluded.total_cases, \
                  job_id = excluded.job_id, evaluated_at = CURRENT_TIMESTAMP",
            )
            .bind(&evaluation.role_id)
            .bind(&evaluation.evaluation_id)
            .bind(&evaluation.role_fingerprint)
            .bind(&evaluation.status)
            .bind(&evaluation.suite_id)
            .bind(&evaluation.engine_id)
            .bind(i64::from(evaluation.passed_cases))
            .bind(i64::from(evaluation.total_cases))
            .bind(&evaluation.job_id)
            .execute(&mut *transaction)
            .await
            .map_err(box_storage)?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(ModelEvaluationWriteOutcome::Applied)
        })
    }
}

impl ProviderRepository for SqliteStorage {
    fn saved_provider_credentials_exist(&self) -> AssistantFuture<'_, bool> {
        Box::pin(async move {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM assistant_provider_connections \
                 WHERE encrypted_api_key != '' OR api_key_nonce != '' LIMIT 1)",
            )
            .fetch_one(&self.pool)
            .await
            .map_err(box_storage)
        })
    }

    fn reset_provider_credentials(&self) -> AssistantFuture<'_, ProviderCredentialResetOutcome> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let active: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM background_jobs \
                 WHERE kind LIKE 'assistant.model%' \
                 AND status IN ('queued', 'running', 'cancel_requested') LIMIT 1)",
            )
            .fetch_one(&mut *transaction)
            .await
            .map_err(box_storage)?;
            if active {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderCredentialResetOutcome::ModelJobActive);
            }
            let deleted_credentials = u64::try_from(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM assistant_provider_connections \
                     WHERE encrypted_api_key != '' AND api_key_nonce != ''",
                )
                .fetch_one(&mut *transaction)
                .await
                .map_err(box_storage)?,
            )
            .map_err(|_| {
                box_storage(StorageError::InvalidAssistantRecord(
                    "invalid provider credential count",
                ))
            })?;
            sqlx::query(
                "UPDATE assistant_provider_connections SET encrypted_api_key = '', \
                 api_key_nonce = '', api_key_hint = '', verification_status = 'never', \
                 verification_error_code = NULL, verified_models_json = '[]', \
                 verified_capabilities_json = '[]', last_verified_at = NULL, \
                 updated_at = CURRENT_TIMESTAMP",
            )
            .execute(&mut *transaction)
            .await
            .map_err(box_storage)?;
            sqlx::query("DELETE FROM assistant_model_evaluations")
                .execute(&mut *transaction)
                .await
                .map_err(box_storage)?;
            sqlx::query(
                "UPDATE assistant_model_roles SET conformance_status = 'never', \
                 conformance_error_code = NULL, conformance_fingerprint = NULL, \
                 last_conformance_at = NULL, updated_at = CURRENT_TIMESTAMP",
            )
            .execute(&mut *transaction)
            .await
            .map_err(box_storage)?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(ProviderCredentialResetOutcome::Applied {
                deleted_credentials,
            })
        })
    }

    fn provider_connections(&self) -> AssistantFuture<'_, Vec<ProviderConnectionRecord>> {
        Box::pin(async move {
            let query = format!("{CONNECTION_SELECT} ORDER BY lower(name), id");
            let rows = sqlx::query(AssertSqlSafe(query))
                .fetch_all(&self.pool)
                .await
                .map_err(box_storage)?;
            rows.iter()
                .map(connection_from_row)
                .collect::<Result<Vec<_>, _>>()
                .map_err(box_storage)
        })
    }

    fn provider_connection<'a>(
        &'a self,
        connection_id: &'a str,
    ) -> AssistantFuture<'a, Option<ProviderConnectionRecord>> {
        Box::pin(async move {
            let query = format!("{CONNECTION_SELECT} WHERE id = ?");
            let row = sqlx::query(AssertSqlSafe(query))
                .bind(connection_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(box_storage)?;
            row.as_ref()
                .map(connection_from_row)
                .transpose()
                .map_err(box_storage)
        })
    }

    fn prepare_provider_connection<'a>(
        &'a self,
        connection_id: &'a str,
    ) -> AssistantFuture<'a, ProviderConnectionPreparation> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let Some(connection) = load_connection_tx(&mut transaction, connection_id)
                .await
                .map_err(box_storage)?
            else {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderConnectionPreparation::NotFound);
            };
            if connection_has_active_model_job(&mut transaction, connection_id)
                .await
                .map_err(box_storage)?
            {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderConnectionPreparation::ModelJobActive);
            }
            transaction.commit().await.map_err(box_storage)?;
            Ok(ProviderConnectionPreparation::Ready(Box::new(connection)))
        })
    }

    fn finish_provider_verification<'a>(
        &'a self,
        verification: &'a ProviderVerificationWrite,
    ) -> AssistantFuture<'a, ProviderVerificationWriteOutcome> {
        Box::pin(async move {
            let models = encode_string_list(&verification.models).map_err(box_storage)?;
            let capabilities =
                encode_string_list(&verification.capability_ids).map_err(box_storage)?;
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let Some(current) = load_connection_tx(&mut transaction, &verification.connection_id)
                .await
                .map_err(box_storage)?
            else {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderVerificationWriteOutcome::NotFound);
            };
            if current.fingerprint() != verification.expected_fingerprint {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderVerificationWriteOutcome::Changed);
            }
            if connection_has_active_model_job(&mut transaction, &verification.connection_id)
                .await
                .map_err(box_storage)?
            {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderVerificationWriteOutcome::ModelJobActive);
            }
            sqlx::query(
                "UPDATE assistant_provider_connections SET verification_status = ?, \
                 verification_error_code = ?, verified_models_json = ?, \
                 verified_capabilities_json = ?, last_verified_at = CURRENT_TIMESTAMP, \
                 updated_at = CURRENT_TIMESTAMP WHERE id = ?",
            )
            .bind(if verification.verified {
                "verified"
            } else {
                "failed"
            })
            .bind(&verification.error_code)
            .bind(models)
            .bind(capabilities)
            .bind(&verification.connection_id)
            .execute(&mut *transaction)
            .await
            .map_err(box_storage)?;
            reset_roles_for_connection(&mut transaction, &verification.connection_id)
                .await
                .map_err(box_storage)?;
            let connection = load_connection_tx(&mut transaction, &verification.connection_id)
                .await
                .map_err(box_storage)?
                .ok_or_else(|| {
                    box_storage(StorageError::InvalidAssistantRecord(
                        "provider connection disappeared during verification",
                    ))
                })?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(ProviderVerificationWriteOutcome::Applied(Box::new(
                connection,
            )))
        })
    }

    fn create_provider_connection<'a>(
        &'a self,
        connection: &'a ProviderConnectionRecord,
    ) -> AssistantFuture<'a, ProviderMutationOutcome> {
        Box::pin(async move {
            let models = encode_string_list(&connection.verified_models).map_err(box_storage)?;
            let capabilities =
                encode_string_list(&connection.verified_capability_ids).map_err(box_storage)?;
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            if duplicate_name(&mut transaction, &connection.name, None)
                .await
                .map_err(box_storage)?
            {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderMutationOutcome::DuplicateName);
            }
            sqlx::query(
                "INSERT INTO assistant_provider_connections \
                 (id, name, adapter_id, base_url, encrypted_api_key, api_key_nonce, api_key_hint, \
                  allow_private_network, verification_status, verification_error_code, \
                  verified_models_json, verified_capabilities_json, last_verified_at, \
                  created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime(?, 'unixepoch'), \
                         CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            )
            .bind(&connection.id)
            .bind(&connection.name)
            .bind(&connection.adapter_id)
            .bind(&connection.base_url)
            .bind(&connection.encrypted_api_key)
            .bind(&connection.api_key_nonce)
            .bind(&connection.api_key_hint)
            .bind(connection.allow_private_network)
            .bind(&connection.verification_status)
            .bind(&connection.verification_error_code)
            .bind(models)
            .bind(capabilities)
            .bind(connection.last_verified_at_unix_seconds)
            .execute(&mut *transaction)
            .await
            .map_err(box_storage)?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(ProviderMutationOutcome::Applied)
        })
    }

    fn replace_provider_connection<'a>(
        &'a self,
        expected_fingerprint: &'a str,
        connection: &'a ProviderConnectionRecord,
        reset_dependents: bool,
    ) -> AssistantFuture<'a, ProviderMutationOutcome> {
        Box::pin(async move {
            let models = encode_string_list(&connection.verified_models).map_err(box_storage)?;
            let capabilities =
                encode_string_list(&connection.verified_capability_ids).map_err(box_storage)?;
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let Some(current) = load_connection_tx(&mut transaction, &connection.id)
                .await
                .map_err(box_storage)?
            else {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderMutationOutcome::NotFound);
            };
            if current.fingerprint() != expected_fingerprint {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderMutationOutcome::Changed);
            }
            if duplicate_name(&mut transaction, &connection.name, Some(&connection.id))
                .await
                .map_err(box_storage)?
            {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderMutationOutcome::DuplicateName);
            }
            if reset_dependents
                && connection_has_active_model_job(&mut transaction, &connection.id)
                    .await
                    .map_err(box_storage)?
            {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderMutationOutcome::ConnectionModelJobActive);
            }
            sqlx::query(
                "UPDATE assistant_provider_connections SET name = ?, adapter_id = ?, \
                 base_url = ?, encrypted_api_key = ?, api_key_nonce = ?, api_key_hint = ?, \
                 allow_private_network = ?, verification_status = ?, \
                 verification_error_code = ?, verified_models_json = ?, \
                 verified_capabilities_json = ?, last_verified_at = datetime(?, 'unixepoch'), \
                 updated_at = CURRENT_TIMESTAMP WHERE id = ?",
            )
            .bind(&connection.name)
            .bind(&connection.adapter_id)
            .bind(&connection.base_url)
            .bind(&connection.encrypted_api_key)
            .bind(&connection.api_key_nonce)
            .bind(&connection.api_key_hint)
            .bind(connection.allow_private_network)
            .bind(&connection.verification_status)
            .bind(&connection.verification_error_code)
            .bind(models)
            .bind(capabilities)
            .bind(connection.last_verified_at_unix_seconds)
            .bind(&connection.id)
            .execute(&mut *transaction)
            .await
            .map_err(box_storage)?;
            if reset_dependents {
                reset_roles_for_connection(&mut transaction, &connection.id)
                    .await
                    .map_err(box_storage)?;
            }
            transaction.commit().await.map_err(box_storage)?;
            Ok(ProviderMutationOutcome::Applied)
        })
    }

    fn delete_provider_connection<'a>(
        &'a self,
        connection_id: &'a str,
    ) -> AssistantFuture<'a, ProviderMutationOutcome> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM assistant_provider_connections WHERE id = ?)",
            )
            .bind(connection_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(box_storage)?;
            if !exists {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderMutationOutcome::NotFound);
            }
            let assigned: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM assistant_model_roles WHERE connection_id = ?)",
            )
            .bind(connection_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(box_storage)?;
            if assigned {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderMutationOutcome::ConnectionInUse);
            }
            sqlx::query("DELETE FROM assistant_provider_connections WHERE id = ?")
                .bind(connection_id)
                .execute(&mut *transaction)
                .await
                .map_err(box_storage)?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(ProviderMutationOutcome::Applied)
        })
    }

    fn clear_provider_credential<'a>(
        &'a self,
        connection_id: &'a str,
    ) -> AssistantFuture<'a, ProviderMutationOutcome> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM assistant_provider_connections WHERE id = ?)",
            )
            .bind(connection_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(box_storage)?;
            if !exists {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderMutationOutcome::NotFound);
            }
            if connection_has_active_model_job(&mut transaction, connection_id)
                .await
                .map_err(box_storage)?
            {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderMutationOutcome::ConnectionModelJobActive);
            }
            sqlx::query(
                "UPDATE assistant_provider_connections SET encrypted_api_key = '', \
                 api_key_nonce = '', api_key_hint = '', verification_status = 'never', \
                 verification_error_code = NULL, verified_models_json = '[]', \
                 verified_capabilities_json = '[]', last_verified_at = NULL, \
                 updated_at = CURRENT_TIMESTAMP WHERE id = ?",
            )
            .bind(connection_id)
            .execute(&mut *transaction)
            .await
            .map_err(box_storage)?;
            reset_roles_for_connection(&mut transaction, connection_id)
                .await
                .map_err(box_storage)?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(ProviderMutationOutcome::Applied)
        })
    }

    fn model_roles(&self) -> AssistantFuture<'_, Vec<ModelRoleRecord>> {
        Box::pin(async move {
            let query = format!("{ROLE_SELECT} ORDER BY role_id");
            let rows = sqlx::query(AssertSqlSafe(query))
                .fetch_all(&self.pool)
                .await
                .map_err(box_storage)?;
            rows.iter()
                .map(role_from_row)
                .collect::<Result<Vec<_>, _>>()
                .map_err(box_storage)
        })
    }

    fn prepare_model_role<'a>(
        &'a self,
        role_id: &'a str,
    ) -> AssistantFuture<'a, ProviderRolePreparation> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let Some(role) = load_role_tx(&mut transaction, role_id)
                .await
                .map_err(box_storage)?
            else {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderRolePreparation::NotConfigured);
            };
            if role_has_active_model_job(&mut transaction, role_id)
                .await
                .map_err(box_storage)?
            {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderRolePreparation::ModelJobActive);
            }
            let Some(connection) = load_connection_tx(&mut transaction, &role.connection_id)
                .await
                .map_err(box_storage)?
            else {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderRolePreparation::ConnectionNotFound);
            };
            transaction.commit().await.map_err(box_storage)?;
            Ok(ProviderRolePreparation::Ready(Box::new(
                ProviderRoleRuntimeRecord { role, connection },
            )))
        })
    }

    fn finish_role_conformance<'a>(
        &'a self,
        conformance: &'a ProviderConformanceWrite,
    ) -> AssistantFuture<'a, ProviderConformanceWriteOutcome> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let Some(role) = load_role_tx(&mut transaction, &conformance.role_id)
                .await
                .map_err(box_storage)?
            else {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderConformanceWriteOutcome::RoleChanged);
            };
            if role.configuration_fingerprint()
                != conformance.expected_role_configuration_fingerprint
            {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderConformanceWriteOutcome::RoleChanged);
            }
            let Some(connection) = load_connection_tx(&mut transaction, &role.connection_id)
                .await
                .map_err(box_storage)?
            else {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderConformanceWriteOutcome::ConnectionChanged);
            };
            if connection.fingerprint() != conformance.expected_connection_fingerprint {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderConformanceWriteOutcome::ConnectionChanged);
            }
            if role_has_active_model_job(&mut transaction, &conformance.role_id)
                .await
                .map_err(box_storage)?
            {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderConformanceWriteOutcome::ModelJobActive);
            }
            sqlx::query(
                "UPDATE assistant_model_roles SET conformance_status = ?, \
                 conformance_error_code = ?, conformance_fingerprint = ?, \
                 last_conformance_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP \
                 WHERE role_id = ?",
            )
            .bind(if conformance.passed {
                "passed"
            } else {
                "failed"
            })
            .bind(&conformance.error_code)
            .bind(&conformance.runtime_fingerprint)
            .bind(&conformance.role_id)
            .execute(&mut *transaction)
            .await
            .map_err(box_storage)?;
            let role = load_role_tx(&mut transaction, &conformance.role_id)
                .await
                .map_err(box_storage)?
                .ok_or_else(|| {
                    box_storage(StorageError::InvalidAssistantRecord(
                        "model role disappeared during conformance update",
                    ))
                })?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(ProviderConformanceWriteOutcome::Applied(Box::new(
                ProviderRoleRuntimeRecord { role, connection },
            )))
        })
    }

    fn save_model_role<'a>(
        &'a self,
        expected_connection_fingerprint: &'a str,
        role: &'a ModelRoleRecord,
        reset_evaluations: bool,
    ) -> AssistantFuture<'a, ProviderMutationOutcome> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let Some(connection) = load_connection_tx(&mut transaction, &role.connection_id)
                .await
                .map_err(box_storage)?
            else {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderMutationOutcome::NotFound);
            };
            if connection.fingerprint() != expected_connection_fingerprint {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderMutationOutcome::Changed);
            }
            if role_has_active_model_job(&mut transaction, &role.role_id)
                .await
                .map_err(box_storage)?
            {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderMutationOutcome::RoleModelJobActive);
            }
            sqlx::query(
                "INSERT INTO assistant_model_roles \
                 (role_id, connection_id, model_id, enabled, timeout_seconds, max_output_tokens, \
                  thinking_mode, conformance_status, conformance_error_code, \
                  conformance_fingerprint, last_conformance_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime(?, 'unixepoch'), CURRENT_TIMESTAMP) \
                 ON CONFLICT(role_id) DO UPDATE SET connection_id = excluded.connection_id, \
                  model_id = excluded.model_id, enabled = excluded.enabled, \
                  timeout_seconds = excluded.timeout_seconds, \
                  max_output_tokens = excluded.max_output_tokens, \
                  thinking_mode = excluded.thinking_mode, \
                  conformance_status = excluded.conformance_status, \
                  conformance_error_code = excluded.conformance_error_code, \
                  conformance_fingerprint = excluded.conformance_fingerprint, \
                  last_conformance_at = excluded.last_conformance_at, \
                  updated_at = CURRENT_TIMESTAMP",
            )
            .bind(&role.role_id)
            .bind(&role.connection_id)
            .bind(&role.model_id)
            .bind(role.enabled)
            .bind(i64::from(role.timeout_seconds))
            .bind(i64::from(role.max_output_tokens))
            .bind(&role.thinking_mode)
            .bind(&role.conformance_status)
            .bind(&role.conformance_error_code)
            .bind(&role.conformance_fingerprint)
            .bind(role.last_conformance_at_unix_seconds)
            .execute(&mut *transaction)
            .await
            .map_err(box_storage)?;
            if reset_evaluations {
                sqlx::query("DELETE FROM assistant_model_evaluations WHERE role_id = ?")
                    .bind(&role.role_id)
                    .execute(&mut *transaction)
                    .await
                    .map_err(box_storage)?;
            }
            transaction.commit().await.map_err(box_storage)?;
            Ok(ProviderMutationOutcome::Applied)
        })
    }

    fn delete_model_role<'a>(
        &'a self,
        role_id: &'a str,
    ) -> AssistantFuture<'a, ProviderMutationOutcome> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM assistant_model_roles WHERE role_id = ?)",
            )
            .bind(role_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(box_storage)?;
            if !exists {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderMutationOutcome::NotFound);
            }
            if role_has_active_model_job(&mut transaction, role_id)
                .await
                .map_err(box_storage)?
            {
                transaction.rollback().await.map_err(box_storage)?;
                return Ok(ProviderMutationOutcome::RoleModelJobActive);
            }
            sqlx::query("DELETE FROM assistant_model_roles WHERE role_id = ?")
                .bind(role_id)
                .execute(&mut *transaction)
                .await
                .map_err(box_storage)?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(ProviderMutationOutcome::Applied)
        })
    }
}

async fn load_connection_tx(
    transaction: &mut Transaction<'_, Sqlite>,
    connection_id: &str,
) -> Result<Option<ProviderConnectionRecord>, StorageError> {
    let query = format!("{CONNECTION_SELECT} WHERE id = ?");
    let row = sqlx::query(AssertSqlSafe(query))
        .bind(connection_id)
        .fetch_optional(&mut **transaction)
        .await?;
    row.as_ref().map(connection_from_row).transpose()
}

async fn load_role_tx(
    transaction: &mut Transaction<'_, Sqlite>,
    role_id: &str,
) -> Result<Option<ModelRoleRecord>, StorageError> {
    let query = format!("{ROLE_SELECT} WHERE role_id = ?");
    let row = sqlx::query(AssertSqlSafe(query))
        .bind(role_id)
        .fetch_optional(&mut **transaction)
        .await?;
    row.as_ref().map(role_from_row).transpose()
}

fn connection_from_row(row: &SqliteRow) -> Result<ProviderConnectionRecord, StorageError> {
    Ok(ProviderConnectionRecord {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        adapter_id: row.try_get("adapter_id")?,
        base_url: row.try_get("base_url")?,
        encrypted_api_key: row.try_get("encrypted_api_key")?,
        api_key_nonce: row.try_get("api_key_nonce")?,
        api_key_hint: row.try_get("api_key_hint")?,
        allow_private_network: row.try_get("allow_private_network")?,
        verification_status: row.try_get("verification_status")?,
        verification_error_code: row.try_get("verification_error_code")?,
        verified_models: parse_bounded_string_list(row.try_get("verified_models_json")?, 200),
        verified_capability_ids: parse_bounded_string_list(
            row.try_get("verified_capabilities_json")?,
            32,
        ),
        last_verified_at_unix_seconds: row.try_get("last_verified_at_unix_seconds")?,
        created_at_unix_seconds: required_timestamp(row, "created_at_unix_seconds")?,
        updated_at_unix_seconds: required_timestamp(row, "updated_at_unix_seconds")?,
    })
}

fn role_from_row(row: &SqliteRow) -> Result<ModelRoleRecord, StorageError> {
    Ok(ModelRoleRecord {
        role_id: row.try_get("role_id")?,
        connection_id: row.try_get("connection_id")?,
        model_id: row.try_get("model_id")?,
        enabled: row.try_get("enabled")?,
        timeout_seconds: u16::try_from(row.try_get::<i64, _>("timeout_seconds")?)
            .map_err(|_| StorageError::InvalidAssistantRecord("invalid model timeout"))?,
        max_output_tokens: u32::try_from(row.try_get::<i64, _>("max_output_tokens")?)
            .map_err(|_| StorageError::InvalidAssistantRecord("invalid model token limit"))?,
        thinking_mode: row.try_get("thinking_mode")?,
        conformance_status: row.try_get("conformance_status")?,
        conformance_error_code: row.try_get("conformance_error_code")?,
        conformance_fingerprint: row.try_get("conformance_fingerprint")?,
        last_conformance_at_unix_seconds: row.try_get("last_conformance_at_unix_seconds")?,
        updated_at_unix_seconds: required_timestamp(row, "updated_at_unix_seconds")?,
    })
}

fn evaluation_from_row(row: &SqliteRow) -> Result<ModelEvaluationRecord, StorageError> {
    Ok(ModelEvaluationRecord {
        role_id: row.try_get("role_id")?,
        evaluation_id: row.try_get("evaluation_id")?,
        role_fingerprint: row.try_get("role_fingerprint")?,
        status: row.try_get("status")?,
        suite_id: row.try_get("suite_id")?,
        engine_id: row.try_get("engine_id")?,
        passed_cases: u32::try_from(row.try_get::<i64, _>("passed_cases")?)
            .map_err(|_| StorageError::InvalidAssistantRecord("invalid passed case count"))?,
        total_cases: u32::try_from(row.try_get::<i64, _>("total_cases")?)
            .map_err(|_| StorageError::InvalidAssistantRecord("invalid total case count"))?,
        job_id: row.try_get("job_id")?,
        evaluated_at_unix_seconds: required_timestamp(row, "evaluated_at_unix_seconds")?,
    })
}

fn required_timestamp(row: &SqliteRow, column: &str) -> Result<i64, StorageError> {
    row.try_get::<Option<i64>, _>(column)?
        .ok_or(StorageError::InvalidTimestamp)
}

fn encode_string_list(values: &[String]) -> Result<String, StorageError> {
    serde_json::to_string(values).map_err(StorageError::AssistantSerialization)
}

fn parse_bounded_string_list(raw: &str, limit: usize) -> Vec<String> {
    let Ok(Value::Array(values)) = serde_json::from_str(raw) else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .filter(|value| !value.is_empty() && value.chars().count() <= 256)
        .filter(|value| seen.insert(value.clone()))
        .take(limit)
        .collect()
}

async fn duplicate_name(
    transaction: &mut Transaction<'_, Sqlite>,
    name: &str,
    excluding_id: Option<&str>,
) -> Result<bool, StorageError> {
    let rows = sqlx::query("SELECT id, name FROM assistant_provider_connections")
        .fetch_all(&mut **transaction)
        .await?;
    let folded = name.to_lowercase();
    for row in rows {
        let id: String = row.try_get("id")?;
        let existing: String = row.try_get("name")?;
        if excluding_id != Some(id.as_str()) && existing.to_lowercase() == folded {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn reset_roles_for_connection(
    transaction: &mut Transaction<'_, Sqlite>,
    connection_id: &str,
) -> Result<(), StorageError> {
    sqlx::query(
        "DELETE FROM assistant_model_evaluations WHERE role_id IN \
         (SELECT role_id FROM assistant_model_roles WHERE connection_id = ?)",
    )
    .bind(connection_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE assistant_model_roles SET conformance_status = 'never', \
         conformance_error_code = NULL, conformance_fingerprint = NULL, \
         last_conformance_at = NULL, updated_at = CURRENT_TIMESTAMP WHERE connection_id = ?",
    )
    .bind(connection_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn role_has_active_model_job(
    transaction: &mut Transaction<'_, Sqlite>,
    role_id: &str,
) -> Result<bool, StorageError> {
    let rows = sqlx::query(
        "SELECT parameters_json FROM background_jobs WHERE kind LIKE 'assistant.model%' \
         AND status IN ('queued', 'running', 'cancel_requested')",
    )
    .fetch_all(&mut **transaction)
    .await?;
    Ok(rows.iter().any(|row| {
        row.try_get::<String, _>("parameters_json")
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|value| {
                value
                    .get("role_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .is_some_and(|candidate| candidate == role_id)
    }))
}

async fn connection_has_active_model_job(
    transaction: &mut Transaction<'_, Sqlite>,
    connection_id: &str,
) -> Result<bool, StorageError> {
    let role_rows =
        sqlx::query("SELECT role_id FROM assistant_model_roles WHERE connection_id = ?")
            .bind(connection_id)
            .fetch_all(&mut **transaction)
            .await?;
    let role_ids = role_rows
        .iter()
        .map(|row| row.try_get::<String, _>("role_id"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    if role_ids.is_empty() {
        return Ok(false);
    }
    let rows = sqlx::query(
        "SELECT parameters_json FROM background_jobs WHERE kind LIKE 'assistant.model%' \
         AND status IN ('queued', 'running', 'cancel_requested')",
    )
    .fetch_all(&mut **transaction)
    .await?;
    Ok(rows.iter().any(|row| {
        row.try_get::<String, _>("parameters_json")
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|value| {
                value
                    .get("role_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .is_some_and(|candidate| role_ids.contains(&candidate))
    }))
}

type ProviderDependencyError = Box<dyn std::error::Error + Send + Sync>;

fn box_storage(source: impl Into<StorageError>) -> ProviderDependencyError {
    Box::new(source.into())
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Arc;

    use music_application::assistant::{
        ModelRoleRecord, ProviderConformanceWrite, ProviderConformanceWriteOutcome,
        ProviderConnectionPreparation, ProviderConnectionRecord, ProviderMutationOutcome,
        ProviderRepository, ProviderRolePreparation, ProviderVerificationWrite,
        ProviderVerificationWriteOutcome,
    };
    use tempfile::TempDir;

    use super::*;
    use crate::SqliteStorageOptions;

    async fn storage() -> Result<(TempDir, SqliteStorage), Box<dyn Error + Send + Sync>> {
        let directory = tempfile::tempdir()?;
        let storage =
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("music.db")))
                .await?;
        Ok((directory, storage))
    }

    fn connection(id: &str, name: &str) -> ProviderConnectionRecord {
        ProviderConnectionRecord {
            id: id.to_owned(),
            name: name.to_owned(),
            adapter_id: "openai-compatible/v1".to_owned(),
            base_url: "https://example.test/v1".to_owned(),
            encrypted_api_key: "ciphertext".to_owned(),
            api_key_nonce: "nonce".to_owned(),
            api_key_hint: "••••test".to_owned(),
            allow_private_network: false,
            verification_status: "verified".to_owned(),
            verification_error_code: None,
            verified_models: vec!["fixture-model".to_owned()],
            verified_capability_ids: vec!["structured-text/v1".to_owned()],
            last_verified_at_unix_seconds: Some(1_800_000_000),
            created_at_unix_seconds: 0,
            updated_at_unix_seconds: 0,
        }
    }

    fn role(connection_id: &str) -> ModelRoleRecord {
        ModelRoleRecord {
            role_id: "music_tagger".to_owned(),
            connection_id: connection_id.to_owned(),
            model_id: "fixture-model".to_owned(),
            enabled: true,
            timeout_seconds: 30,
            max_output_tokens: 2_000,
            thinking_mode: "provider_default".to_owned(),
            conformance_status: "passed".to_owned(),
            conformance_error_code: None,
            conformance_fingerprint: Some("f".repeat(64)),
            last_conformance_at_unix_seconds: Some(1_800_000_001),
            updated_at_unix_seconds: 0,
        }
    }

    #[tokio::test]
    async fn provider_mutations_reset_dependent_gates_atomically()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        let original = connection("aabbccddeeff00112233445566778899", "Fixture");
        assert_eq!(
            storage.create_provider_connection(&original).await?,
            ProviderMutationOutcome::Applied
        );
        assert_eq!(
            storage
                .create_provider_connection(&connection(
                    "11223344556677889900aabbccddeeff",
                    "fixture"
                ))
                .await?,
            ProviderMutationOutcome::DuplicateName
        );
        let role = role(&original.id);
        assert_eq!(
            storage
                .save_model_role(&original.fingerprint(), &role, false)
                .await?,
            ProviderMutationOutcome::Applied
        );
        sqlx::query(
            "INSERT INTO background_jobs \
             (id, kind, status, parameters_json, result_json, error, progress_current, \
              progress_total, progress_phase, progress_message, attempts, retry_of_id, \
              created_at, updated_at, lane, schema_version, restartable, checkpoint_policy) \
             VALUES ('finished-job', 'assistant.model.test', 'succeeded', '{}', '{}', NULL, 1, 1, \
                     'Done', 'Done', 1, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, 'provider', 1, 0, 'replace')",
        )
        .execute(&storage.pool)
        .await?;
        sqlx::query(
            "INSERT INTO assistant_model_evaluations \
             (role_id, evaluation_id, role_fingerprint, status, suite_id, engine_id, \
              passed_cases, total_cases, job_id, evaluated_at) \
             VALUES ('music_tagger', 'eval', ?, 'passed', 'suite', 'engine', 1, 1, \
                     'finished-job', CURRENT_TIMESTAMP)",
        )
        .bind("f".repeat(64))
        .execute(&storage.pool)
        .await?;

        let mut replacement = original.clone();
        replacement.base_url = "https://second.example.test/v1".to_owned();
        replacement.verification_status = "never".to_owned();
        replacement.verified_models.clear();
        replacement.verified_capability_ids.clear();
        replacement.last_verified_at_unix_seconds = None;
        assert_eq!(
            storage
                .replace_provider_connection(&original.fingerprint(), &replacement, true)
                .await?,
            ProviderMutationOutcome::Applied
        );
        let stored_role = storage.model_roles().await?.pop().ok_or("missing role")?;
        assert_eq!(stored_role.conformance_status, "never");
        assert!(stored_role.conformance_fingerprint.is_none());
        let evaluations: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM assistant_model_evaluations")
                .fetch_one(&storage.pool)
                .await?;
        assert_eq!(evaluations, 0);
        Ok(())
    }

    #[tokio::test]
    async fn provider_verification_publishes_atomically_and_rejects_stale_results()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        let original = connection("aabbccddeeff00112233445566778899", "Fixture");
        storage.create_provider_connection(&original).await?;
        storage
            .save_model_role(&original.fingerprint(), &role(&original.id), false)
            .await?;
        let prepared = storage.prepare_provider_connection(&original.id).await?;
        assert!(matches!(
            prepared,
            ProviderConnectionPreparation::Ready(ref value) if value.id == original.id
        ));
        let verification = ProviderVerificationWrite {
            connection_id: original.id.clone(),
            expected_fingerprint: original.fingerprint(),
            verified: true,
            error_code: None,
            models: vec!["new-model".to_owned()],
            capability_ids: vec!["structured-text/v1".to_owned()],
        };
        let applied = storage.finish_provider_verification(&verification).await?;
        let ProviderVerificationWriteOutcome::Applied(applied) = applied else {
            return Err(format!("unexpected verification outcome: {applied:?}").into());
        };
        assert_eq!(applied.verification_status, "verified");
        assert_eq!(applied.verified_models, ["new-model"]);
        assert!(applied.last_verified_at_unix_seconds.is_some());
        assert_eq!(
            storage
                .model_roles()
                .await?
                .pop()
                .ok_or("missing role")?
                .conformance_status,
            "never"
        );

        let stale_fingerprint = applied.fingerprint();
        let mut changed = applied.clone();
        changed.base_url = "https://changed.example.test/v1".to_owned();
        assert_eq!(
            storage
                .replace_provider_connection(&stale_fingerprint, &changed, true)
                .await?,
            ProviderMutationOutcome::Applied
        );
        let mut stale = verification;
        stale.expected_fingerprint = stale_fingerprint;
        assert_eq!(
            storage.finish_provider_verification(&stale).await?,
            ProviderVerificationWriteOutcome::Changed
        );
        Ok(())
    }

    #[tokio::test]
    async fn role_conformance_is_fingerprint_bound_and_atomic()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        let connection = connection("aabbccddeeff00112233445566778899", "Fixture");
        storage.create_provider_connection(&connection).await?;
        let role = role(&connection.id);
        storage
            .save_model_role(&connection.fingerprint(), &role, false)
            .await?;
        assert!(matches!(
            storage.prepare_model_role(&role.role_id).await?,
            ProviderRolePreparation::Ready(ref runtime)
                if runtime.role.model_id == "fixture-model"
                    && runtime.connection.id == connection.id
        ));
        let conformance = ProviderConformanceWrite {
            role_id: role.role_id.clone(),
            expected_role_configuration_fingerprint: role.configuration_fingerprint(),
            expected_connection_fingerprint: connection.fingerprint(),
            runtime_fingerprint: "a".repeat(64),
            passed: true,
            error_code: None,
        };
        let applied = storage.finish_role_conformance(&conformance).await?;
        let ProviderConformanceWriteOutcome::Applied(runtime) = applied else {
            return Err(format!("unexpected conformance outcome: {applied:?}").into());
        };
        assert_eq!(runtime.role.conformance_status, "passed");
        assert_eq!(
            runtime.role.conformance_fingerprint.as_deref(),
            Some("a".repeat(64).as_str())
        );
        assert!(runtime.role.last_conformance_at_unix_seconds.is_some());

        let mut changed_role = runtime.role.clone();
        changed_role.model_id = "changed-model".to_owned();
        assert_eq!(
            storage
                .save_model_role(&connection.fingerprint(), &changed_role, true)
                .await?,
            ProviderMutationOutcome::Applied
        );
        assert_eq!(
            storage.finish_role_conformance(&conformance).await?,
            ProviderConformanceWriteOutcome::RoleChanged
        );
        Ok(())
    }

    #[tokio::test]
    async fn model_evaluation_publish_rechecks_job_role_and_connection_atomically()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        let connection = connection("aabbccddeeff00112233445566778899", "Fixture");
        storage.create_provider_connection(&connection).await?;
        let role = role(&connection.id);
        storage
            .save_model_role(&connection.fingerprint(), &role, false)
            .await?;

        let runtime_fingerprint = "9".repeat(64);
        sqlx::query(
            "INSERT INTO background_jobs \
             (id, kind, status, parameters_json, result_json, error, progress_current, \
              progress_total, progress_phase, progress_message, attempts, retry_of_id, \
              created_at, updated_at, lane, schema_version, restartable, checkpoint_policy) \
             VALUES ('evaluation-job', 'assistant.model-evaluation.music-tagging-quality-v1', \
                     'running', ?, NULL, NULL, 0, NULL, '', '', 1, NULL, CURRENT_TIMESTAMP, \
                     CURRENT_TIMESTAMP, 'provider', 1, 0, 'replace')",
        )
        .bind(
            serde_json::json!({
                "role_id": role.role_id,
                "evaluation_id": "music-tagging-quality-v1",
                "role_fingerprint": runtime_fingerprint,
            })
            .to_string(),
        )
        .execute(&storage.pool)
        .await?;
        let evaluation = ModelEvaluationWrite {
            role_id: role.role_id.clone(),
            evaluation_id: "music-tagging-quality-v1".to_owned(),
            expected_role_configuration_fingerprint: role.configuration_fingerprint(),
            expected_connection_fingerprint: connection.fingerprint(),
            role_fingerprint: runtime_fingerprint,
            status: "passed".to_owned(),
            suite_id: "controlled-vocabulary-tagging-baseline-v19".to_owned(),
            engine_id: "model-context-tagger/v6".to_owned(),
            passed_cases: 4,
            total_cases: 4,
            job_id: "evaluation-job".to_owned(),
            job_kind: "assistant.model-evaluation.music-tagging-quality-v1".to_owned(),
        };
        assert_eq!(
            storage.save_model_evaluation(&evaluation).await?,
            ModelEvaluationWriteOutcome::Applied
        );
        let stored = storage.model_evaluations(&role.role_id).await?;
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].status, "passed");
        assert_eq!(stored[0].passed_cases, 4);

        sqlx::query("UPDATE background_jobs SET status = 'succeeded' WHERE id = 'evaluation-job'")
            .execute(&storage.pool)
            .await?;
        let mut replacement = evaluation.clone();
        replacement.status = "failed".to_owned();
        replacement.passed_cases = 0;
        assert_eq!(
            storage.save_model_evaluation(&replacement).await?,
            ModelEvaluationWriteOutcome::JobInactive
        );
        assert_eq!(
            storage.model_evaluations(&role.role_id).await?[0].status,
            "passed"
        );

        sqlx::query("UPDATE background_jobs SET status = 'running' WHERE id = 'evaluation-job'")
            .execute(&storage.pool)
            .await?;
        sqlx::query(
            "UPDATE assistant_model_roles SET model_id = 'changed-model' WHERE role_id = ?",
        )
        .bind(&role.role_id)
        .execute(&storage.pool)
        .await?;
        assert_eq!(
            storage.save_model_evaluation(&replacement).await?,
            ModelEvaluationWriteOutcome::RoleChanged
        );
        assert_eq!(
            storage.model_evaluations(&role.role_id).await?[0].status,
            "passed"
        );
        Ok(())
    }

    #[tokio::test]
    async fn active_model_jobs_block_connection_and_role_mutations()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        let connection = connection("aabbccddeeff00112233445566778899", "Fixture");
        storage.create_provider_connection(&connection).await?;
        storage
            .save_model_role(&connection.fingerprint(), &role(&connection.id), false)
            .await?;
        sqlx::query(
            "INSERT INTO background_jobs \
             (id, kind, status, parameters_json, result_json, error, progress_current, \
              progress_total, progress_phase, progress_message, attempts, retry_of_id, \
              created_at, updated_at, lane, schema_version, restartable, checkpoint_policy) \
             VALUES ('active-job', 'assistant.model.tags', 'queued', \
                     '{\"role_id\":\"music_tagger\"}', NULL, NULL, 0, NULL, '', '', 0, NULL, \
                     CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, 'provider', 1, 0, 'replace')",
        )
        .execute(&storage.pool)
        .await?;
        assert_eq!(
            storage.prepare_provider_connection(&connection.id).await?,
            ProviderConnectionPreparation::ModelJobActive
        );
        assert_eq!(
            storage.prepare_model_role("music_tagger").await?,
            ProviderRolePreparation::ModelJobActive
        );
        assert_eq!(
            storage
                .finish_provider_verification(&ProviderVerificationWrite {
                    connection_id: connection.id.clone(),
                    expected_fingerprint: connection.fingerprint(),
                    verified: false,
                    error_code: Some("network_error".to_owned()),
                    models: Vec::new(),
                    capability_ids: Vec::new(),
                })
                .await?,
            ProviderVerificationWriteOutcome::ModelJobActive
        );
        assert_eq!(
            storage
                .finish_role_conformance(&ProviderConformanceWrite {
                    role_id: "music_tagger".to_owned(),
                    expected_role_configuration_fingerprint: role(&connection.id)
                        .configuration_fingerprint(),
                    expected_connection_fingerprint: connection.fingerprint(),
                    runtime_fingerprint: "a".repeat(64),
                    passed: false,
                    error_code: Some("network_error".to_owned()),
                })
                .await?,
            ProviderConformanceWriteOutcome::ModelJobActive
        );
        assert_eq!(
            storage.clear_provider_credential(&connection.id).await?,
            ProviderMutationOutcome::ConnectionModelJobActive
        );
        assert_eq!(
            storage
                .save_model_role(&connection.fingerprint(), &role(&connection.id), true)
                .await?,
            ProviderMutationOutcome::RoleModelJobActive
        );
        assert_eq!(
            storage.delete_model_role("music_tagger").await?,
            ProviderMutationOutcome::RoleModelJobActive
        );
        assert_eq!(
            storage.reset_provider_credentials().await?,
            ProviderCredentialResetOutcome::ModelJobActive
        );
        sqlx::query("UPDATE background_jobs SET status = 'succeeded' WHERE id = 'active-job'")
            .execute(&storage.pool)
            .await?;
        assert_eq!(
            storage.reset_provider_credentials().await?,
            ProviderCredentialResetOutcome::Applied {
                deleted_credentials: 1
            }
        );
        let cleared = storage
            .provider_connection(&connection.id)
            .await?
            .ok_or("missing connection")?;
        assert!(!cleared.credential_saved());
        assert_eq!(
            storage
                .model_roles()
                .await?
                .pop()
                .ok_or("missing role")?
                .conformance_status,
            "never"
        );
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_runtime_updates_have_one_winner() -> Result<(), Box<dyn Error + Send + Sync>>
    {
        let (_directory, storage) = storage().await?;
        let original = connection("aabbccddeeff00112233445566778899", "Fixture");
        storage.create_provider_connection(&original).await?;
        let expected = original.fingerprint();
        let storage = Arc::new(storage);
        let mut tasks = Vec::new();
        for suffix in ["one", "two"] {
            let storage = Arc::clone(&storage);
            let mut replacement = original.clone();
            replacement.base_url = format!("https://{suffix}.example.test/v1");
            let expected = expected.clone();
            tasks.push(tokio::spawn(async move {
                storage
                    .replace_provider_connection(&expected, &replacement, true)
                    .await
            }));
        }
        let mut applied = 0;
        let mut changed = 0;
        for task in tasks {
            match task.await?? {
                ProviderMutationOutcome::Applied => applied += 1,
                ProviderMutationOutcome::Changed => changed += 1,
                outcome => return Err(format!("unexpected outcome: {outcome:?}").into()),
            }
        }
        assert_eq!((applied, changed), (1, 1));
        Ok(())
    }

    #[tokio::test]
    async fn audits_and_rotates_credentials_as_one_offline_transaction()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        let current = CredentialVault::from_key([7; 32])?;
        let replacement = CredentialVault::from_key([9; 32])?;
        let encrypted = current.encrypt("aabbccddeeff00112233445566778899", "fixture-secret")?;
        let mut saved = connection("aabbccddeeff00112233445566778899", "Fixture");
        saved.encrypted_api_key = encrypted.ciphertext;
        saved.api_key_nonce = encrypted.nonce;
        saved.api_key_hint = encrypted.hint;
        storage.create_provider_connection(&saved).await?;
        storage
            .save_model_role(&saved.fingerprint(), &role(&saved.id), false)
            .await?;
        sqlx::query(
            "INSERT INTO background_jobs \
             (id, kind, status, parameters_json, result_json, error, progress_current, \
              progress_total, progress_phase, progress_message, attempts, retry_of_id, \
              created_at, updated_at, lane, schema_version, restartable, checkpoint_policy) \
             VALUES ('rotation-job', 'assistant.model.test', 'running', '{}', NULL, NULL, 0, 1, \
                     'Running', 'Running', 1, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, \
                     'provider', 1, 0, 'replace')",
        )
        .execute(&storage.pool)
        .await?;

        let audit = storage.audit_provider_credentials(&current).await?;
        assert!(audit.healthy());
        assert_eq!(audit.saved_credentials, 1);
        assert_eq!(
            storage
                .audit_provider_credentials(&CredentialVault::from_key([8; 32])?)
                .await?
                .unreadable_credentials,
            1
        );

        assert_eq!(
            storage
                .rotate_provider_credentials(&current, &replacement)
                .await?,
            ProviderCredentialRotationOutcome::ModelJobActive
        );
        sqlx::query(
            "UPDATE background_jobs SET status = 'succeeded', result_json = '{}' \
             WHERE id = 'rotation-job'",
        )
        .execute(&storage.pool)
        .await?;
        sqlx::query(
            "INSERT INTO assistant_model_evaluations \
             (role_id, evaluation_id, role_fingerprint, status, suite_id, engine_id, \
              passed_cases, total_cases, job_id, evaluated_at) \
             VALUES ('music_tagger', 'rotation-evaluation', 'fixture', 'passed', 'suite', \
                     'engine', 1, 1, 'rotation-job', CURRENT_TIMESTAMP)",
        )
        .execute(&storage.pool)
        .await?;

        assert_eq!(
            storage
                .rotate_provider_credentials(&current, &replacement)
                .await?,
            ProviderCredentialRotationOutcome::Applied {
                rotated_credentials: 1
            }
        );
        assert_eq!(
            storage
                .audit_provider_credentials(&current)
                .await?
                .unreadable_credentials,
            1
        );
        assert!(
            storage
                .audit_provider_credentials(&replacement)
                .await?
                .healthy()
        );
        let rotated = storage
            .provider_connection(&saved.id)
            .await?
            .ok_or("missing rotated connection")?;
        assert_eq!(rotated.verification_status, "never");
        assert_eq!(
            storage
                .model_roles()
                .await?
                .pop()
                .ok_or("missing rotated role")?
                .conformance_status,
            "never"
        );
        let evaluations: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM assistant_model_evaluations")
                .fetch_one(&storage.pool)
                .await?;
        assert_eq!(evaluations, 0);
        assert_eq!(
            replacement
                .decrypt(
                    &rotated.id,
                    &rotated.encrypted_api_key,
                    &rotated.api_key_nonce
                )?
                .expose_secret(),
            "fixture-secret"
        );
        Ok(())
    }
}
