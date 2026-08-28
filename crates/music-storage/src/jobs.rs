use std::collections::BTreeMap;

use music_application::jobs::{
    JobCheckpointPolicy, JobClaim, JobDefinition, JobFinish, JobFuture, JobLane, JobLeaseState,
    JobListFilter, JobProgress, JobRecord, JobRepository, JobStatus, NewJob,
};
use serde_json::{Map, Value};
use sqlx::sqlite::SqliteRow;
use sqlx::{AssertSqlSafe, Row, Sqlite, Transaction};
use uuid::Uuid;

use crate::{SqliteStorage, StorageError};

const JOB_SELECT: &str = "SELECT id, kind, status, parameters_json, result_json, error, \
    progress_current, progress_total, progress_phase, progress_message, attempts, retry_of_id, \
    CAST(strftime('%s', created_at) AS INTEGER) AS created_at_unix_seconds, \
    CAST(strftime('%s', updated_at) AS INTEGER) AS updated_at_unix_seconds, \
    CAST(strftime('%s', started_at) AS INTEGER) AS started_at_unix_seconds, \
    CAST(strftime('%s', finished_at) AS INTEGER) AS finished_at_unix_seconds, \
    lane, schema_version, restartable, checkpoint_policy, execution_id \
    FROM background_jobs";

impl JobRepository for SqliteStorage {
    fn create<'a>(&'a self, job: &'a NewJob) -> JobFuture<'a, JobRecord> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            insert_job(&mut transaction, job)
                .await
                .map_err(box_storage)?;
            let record = read_job(&mut transaction, &job.id)
                .await
                .map_err(box_storage)?
                .ok_or_else(|| box_storage(StorageError::InvalidJobRecord("insert disappeared")))?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(record)
        })
    }

    fn create_unique_active<'a>(&'a self, job: &'a NewJob) -> JobFuture<'a, (JobRecord, bool)> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let query = format!(
                "{JOB_SELECT} WHERE kind = ? AND status IN ('queued', 'running', 'cancel_requested') \
                 ORDER BY created_at DESC, id DESC LIMIT 1"
            );
            if let Some(row) = sqlx::query(AssertSqlSafe(query))
                .bind(job.definition.kind)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(box_storage)?
            {
                let record = row_to_job(&row).map_err(box_storage)?;
                transaction.commit().await.map_err(box_storage)?;
                return Ok((record, false));
            }
            insert_job(&mut transaction, job)
                .await
                .map_err(box_storage)?;
            let record = read_job(&mut transaction, &job.id)
                .await
                .map_err(box_storage)?
                .ok_or_else(|| box_storage(StorageError::InvalidJobRecord("insert disappeared")))?;
            transaction.commit().await.map_err(box_storage)?;
            Ok((record, true))
        })
    }

    fn get<'a>(&'a self, id: &'a str) -> JobFuture<'a, Option<JobRecord>> {
        Box::pin(async move {
            let query = format!("{JOB_SELECT} WHERE id = ?");
            let row = sqlx::query(AssertSqlSafe(query))
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(box_storage)?;
            row.as_ref()
                .map(row_to_job)
                .transpose()
                .map_err(box_storage)
        })
    }

    fn list<'a>(&'a self, filter: &'a JobListFilter) -> JobFuture<'a, Vec<JobRecord>> {
        Box::pin(async move {
            let kind = filter.kind.as_deref();
            let status = filter.status.map(JobStatus::as_str);
            let query = format!(
                "{JOB_SELECT} WHERE (? IS NULL OR kind = ?) AND (? IS NULL OR status = ?) \
                 ORDER BY created_at DESC, id DESC LIMIT ?"
            );
            let rows = sqlx::query(AssertSqlSafe(query))
                .bind(kind)
                .bind(kind)
                .bind(status)
                .bind(status)
                .bind(i64::from(filter.limit))
                .fetch_all(&self.pool)
                .await
                .map_err(box_storage)?;
            rows.iter()
                .map(row_to_job)
                .collect::<Result<Vec<_>, _>>()
                .map_err(box_storage)
        })
    }

    fn request_cancellation<'a>(&'a self, id: &'a str) -> JobFuture<'a, Option<(JobRecord, bool)>> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let Some(before) = read_job(&mut transaction, id).await.map_err(box_storage)? else {
                transaction.commit().await.map_err(box_storage)?;
                return Ok(None);
            };
            let changed = match before.status {
                JobStatus::Queued => {
                    sqlx::query(
                        "UPDATE background_jobs SET status = 'cancelled', \
                         progress_phase = 'Cancelled', updated_at = CURRENT_TIMESTAMP, \
                         finished_at = CURRENT_TIMESTAMP, execution_id = NULL \
                         WHERE id = ? AND status = 'queued'",
                    )
                    .bind(id)
                    .execute(&mut *transaction)
                    .await
                    .map_err(box_storage)?
                    .rows_affected()
                        == 1
                }
                JobStatus::Running => {
                    sqlx::query(
                        "UPDATE background_jobs SET status = 'cancel_requested', \
                         progress_phase = 'Cancelling', updated_at = CURRENT_TIMESTAMP \
                         WHERE id = ? AND status = 'running'",
                    )
                    .bind(id)
                    .execute(&mut *transaction)
                    .await
                    .map_err(box_storage)?
                    .rows_affected()
                        == 1
                }
                JobStatus::CancelRequested
                | JobStatus::Succeeded
                | JobStatus::Failed
                | JobStatus::Cancelled => false,
            };
            let current = read_job(&mut transaction, id)
                .await
                .map_err(box_storage)?
                .ok_or_else(|| box_storage(StorageError::InvalidJobRecord("job disappeared")))?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(Some((current, changed)))
        })
    }

    fn claim_next<'a>(&'a self, lane: JobLane) -> JobFuture<'a, Option<JobClaim>> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let id = sqlx::query_scalar::<_, String>(
                "SELECT id FROM background_jobs WHERE status = 'queued' AND lane = ? \
                 ORDER BY created_at, id LIMIT 1",
            )
            .bind(lane.as_str())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(box_storage)?;
            let Some(id) = id else {
                transaction.commit().await.map_err(box_storage)?;
                return Ok(None);
            };
            let execution_id = Uuid::new_v4().simple().to_string();
            let changed = sqlx::query(
                "UPDATE background_jobs SET status = 'running', execution_id = ?, \
                 started_at = CURRENT_TIMESTAMP, finished_at = NULL, error = NULL, \
                 progress_phase = 'Starting', updated_at = CURRENT_TIMESTAMP, attempts = attempts + 1 \
                 WHERE id = ? AND status = 'queued'",
            )
            .bind(&execution_id)
            .bind(&id)
            .execute(&mut *transaction)
            .await
            .map_err(box_storage)?
            .rows_affected();
            if changed != 1 {
                transaction.commit().await.map_err(box_storage)?;
                return Ok(None);
            }
            let job = read_job(&mut transaction, &id)
                .await
                .map_err(box_storage)?
                .ok_or_else(|| box_storage(StorageError::InvalidJobRecord("claim disappeared")))?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(Some(JobClaim { job, execution_id }))
        })
    }

    fn lease_state<'a>(
        &'a self,
        id: &'a str,
        execution_id: &'a str,
    ) -> JobFuture<'a, JobLeaseState> {
        Box::pin(async move {
            lease_state(&self.pool, id, execution_id)
                .await
                .map_err(box_storage)
        })
    }

    fn update_progress<'a>(
        &'a self,
        claim: &'a JobClaim,
        progress: &'a JobProgress,
    ) -> JobFuture<'a, JobLeaseState> {
        Box::pin(async move {
            let current = i64::try_from(progress.current).map_err(|_| {
                box_storage(StorageError::InvalidJobRecord("progress current overflow"))
            })?;
            let total = progress.total.map(i64::try_from).transpose().map_err(|_| {
                box_storage(StorageError::InvalidJobRecord("progress total overflow"))
            })?;
            sqlx::query(
                "UPDATE background_jobs SET progress_current = ?, progress_total = ?, \
                 progress_phase = ?, progress_message = ?, updated_at = CURRENT_TIMESTAMP \
                 WHERE id = ? AND execution_id = ? \
                   AND status IN ('running', 'cancel_requested')",
            )
            .bind(current)
            .bind(total)
            .bind(&progress.phase)
            .bind(&progress.message)
            .bind(&claim.job.id)
            .bind(&claim.execution_id)
            .execute(&self.pool)
            .await
            .map_err(box_storage)?;
            lease_state(&self.pool, &claim.job.id, &claim.execution_id)
                .await
                .map_err(box_storage)
        })
    }

    fn checkpoint<'a>(
        &'a self,
        claim: &'a JobClaim,
        result: &'a Map<String, Value>,
    ) -> JobFuture<'a, JobLeaseState> {
        Box::pin(async move {
            let encoded = serde_json::to_string(result)
                .map_err(StorageError::JobSerialization)
                .map_err(box_storage)?;
            sqlx::query(
                "UPDATE background_jobs SET result_json = ?, updated_at = CURRENT_TIMESTAMP \
                 WHERE id = ? AND execution_id = ? \
                   AND status IN ('running', 'cancel_requested')",
            )
            .bind(encoded)
            .bind(&claim.job.id)
            .bind(&claim.execution_id)
            .execute(&self.pool)
            .await
            .map_err(box_storage)?;
            lease_state(&self.pool, &claim.job.id, &claim.execution_id)
                .await
                .map_err(box_storage)
        })
    }

    fn finish<'a>(
        &'a self,
        claim: &'a JobClaim,
        finish: &'a JobFinish,
    ) -> JobFuture<'a, JobLeaseState> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let state = lease_state(&mut *transaction, &claim.job.id, &claim.execution_id)
                .await
                .map_err(box_storage)?;
            if state == JobLeaseState::Lost {
                transaction.commit().await.map_err(box_storage)?;
                return Ok(state);
            }
            apply_finish(&mut transaction, claim, finish, state)
                .await
                .map_err(box_storage)?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(JobLeaseState::Lost)
        })
    }

    fn recover_interrupted<'a>(
        &'a self,
        definitions: &'a BTreeMap<String, JobDefinition>,
    ) -> JobFuture<'a, usize> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            let rows = sqlx::query(
                "SELECT id, kind, status, lane, schema_version, restartable \
                 FROM background_jobs WHERE status IN ('running', 'cancel_requested')",
            )
            .fetch_all(&mut *transaction)
            .await
            .map_err(box_storage)?;
            for row in &rows {
                let id: String = row
                    .try_get("id")
                    .map_err(StorageError::from)
                    .map_err(box_storage)?;
                let status: String = row
                    .try_get("status")
                    .map_err(StorageError::from)
                    .map_err(box_storage)?;
                if status == "cancel_requested" {
                    sqlx::query(
                        "UPDATE background_jobs SET status = 'cancelled', \
                         progress_phase = 'Cancelled', finished_at = CURRENT_TIMESTAMP, \
                         updated_at = CURRENT_TIMESTAMP, execution_id = NULL WHERE id = ?",
                    )
                    .bind(&id)
                    .execute(&mut *transaction)
                    .await
                    .map_err(box_storage)?;
                    continue;
                }
                let kind: String = row
                    .try_get("kind")
                    .map_err(StorageError::from)
                    .map_err(box_storage)?;
                let lane: String = row
                    .try_get("lane")
                    .map_err(StorageError::from)
                    .map_err(box_storage)?;
                let schema_version: i64 = row
                    .try_get("schema_version")
                    .map_err(StorageError::from)
                    .map_err(box_storage)?;
                let restartable: i64 = row
                    .try_get("restartable")
                    .map_err(StorageError::from)
                    .map_err(box_storage)?;
                let can_restart = definitions.get(&kind).is_some_and(|definition| {
                    definition.restartable
                        && restartable != 0
                        && i64::from(definition.schema_version) == schema_version
                        && definition.lane.as_str() == lane
                });
                if can_restart {
                    sqlx::query(
                        "UPDATE background_jobs SET status = 'queued', \
                         progress_phase = 'Queued after server restart', started_at = NULL, \
                         updated_at = CURRENT_TIMESTAMP, execution_id = NULL WHERE id = ?",
                    )
                    .bind(&id)
                    .execute(&mut *transaction)
                    .await
                    .map_err(box_storage)?;
                } else {
                    sqlx::query(
                        "UPDATE background_jobs SET status = 'failed', \
                         error = 'Job was interrupted by a server restart.', \
                         progress_phase = 'Interrupted', finished_at = CURRENT_TIMESTAMP, \
                         updated_at = CURRENT_TIMESTAMP, execution_id = NULL WHERE id = ?",
                    )
                    .bind(&id)
                    .execute(&mut *transaction)
                    .await
                    .map_err(box_storage)?;
                }
            }
            transaction.commit().await.map_err(box_storage)?;
            Ok(rows.len())
        })
    }
}

async fn insert_job(
    transaction: &mut Transaction<'_, Sqlite>,
    job: &NewJob,
) -> Result<(), StorageError> {
    let parameters =
        serde_json::to_string(&job.parameters).map_err(StorageError::JobSerialization)?;
    sqlx::query(
        "INSERT INTO background_jobs (id, kind, status, parameters_json, result_json, error, \
         progress_current, progress_total, progress_phase, progress_message, attempts, retry_of_id, \
         created_at, updated_at, started_at, finished_at, lane, schema_version, restartable, \
         checkpoint_policy, execution_id) \
         VALUES (?, ?, 'queued', ?, NULL, NULL, 0, NULL, 'Queued', '', 0, ?, \
         CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, NULL, NULL, ?, ?, ?, ?, NULL)",
    )
    .bind(&job.id)
    .bind(job.definition.kind)
    .bind(parameters)
    .bind(&job.retry_of_id)
    .bind(job.definition.lane.as_str())
    .bind(i64::from(job.definition.schema_version))
    .bind(job.definition.restartable)
    .bind(job.definition.checkpoint_policy.as_str())
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn read_job(
    transaction: &mut Transaction<'_, Sqlite>,
    id: &str,
) -> Result<Option<JobRecord>, StorageError> {
    let query = format!("{JOB_SELECT} WHERE id = ?");
    let row = sqlx::query(AssertSqlSafe(query))
        .bind(id)
        .fetch_optional(&mut **transaction)
        .await?;
    row.as_ref().map(row_to_job).transpose()
}

fn row_to_job(row: &SqliteRow) -> Result<JobRecord, StorageError> {
    let parameters_json: String = row.try_get("parameters_json")?;
    let parameters = json_object(&parameters_json, "parameters are not an object")?;
    let result_json: Option<String> = row.try_get("result_json")?;
    let result = result_json
        .as_deref()
        .map(|value| json_object(value, "result is not an object"))
        .transpose()?;
    let progress_current: i64 = row.try_get("progress_current")?;
    let progress_total: Option<i64> = row.try_get("progress_total")?;
    let attempts: i64 = row.try_get("attempts")?;
    let schema_version: i64 = row.try_get("schema_version")?;
    Ok(JobRecord {
        id: row.try_get("id")?,
        kind: row.try_get("kind")?,
        status: JobStatus::parse(row.try_get::<String, _>("status")?.as_str())
            .map_err(|_| StorageError::InvalidJobRecord("status"))?,
        parameters,
        result,
        error: row.try_get("error")?,
        progress_current: u64::try_from(progress_current)
            .map_err(|_| StorageError::InvalidJobRecord("negative progress"))?,
        progress_total: progress_total
            .map(u64::try_from)
            .transpose()
            .map_err(|_| StorageError::InvalidJobRecord("negative progress total"))?,
        progress_phase: row.try_get("progress_phase")?,
        progress_message: row.try_get("progress_message")?,
        attempts: u32::try_from(attempts)
            .map_err(|_| StorageError::InvalidJobRecord("attempt count"))?,
        retry_of_id: row.try_get("retry_of_id")?,
        created_at_unix_seconds: required_timestamp(row, "created_at_unix_seconds")?,
        updated_at_unix_seconds: required_timestamp(row, "updated_at_unix_seconds")?,
        started_at_unix_seconds: row.try_get("started_at_unix_seconds")?,
        finished_at_unix_seconds: row.try_get("finished_at_unix_seconds")?,
        lane: JobLane::parse(row.try_get::<String, _>("lane")?.as_str())
            .map_err(|_| StorageError::InvalidJobRecord("lane"))?,
        schema_version: u32::try_from(schema_version)
            .ok()
            .filter(|version| *version > 0)
            .ok_or(StorageError::InvalidJobRecord("schema version"))?,
        restartable: row.try_get::<i64, _>("restartable")? != 0,
        checkpoint_policy: JobCheckpointPolicy::parse(
            row.try_get::<String, _>("checkpoint_policy")?.as_str(),
        )
        .map_err(|_| StorageError::InvalidJobRecord("checkpoint policy"))?,
        execution_id: row.try_get("execution_id")?,
    })
}

fn json_object(value: &str, detail: &'static str) -> Result<Map<String, Value>, StorageError> {
    let parsed = serde_json::from_str::<Value>(value).map_err(StorageError::JobSerialization)?;
    let Value::Object(object) = parsed else {
        return Err(StorageError::InvalidJobRecord(detail));
    };
    Ok(object)
}

fn required_timestamp(row: &SqliteRow, column: &'static str) -> Result<i64, StorageError> {
    row.try_get::<Option<i64>, _>(column)?
        .ok_or(StorageError::InvalidJobRecord("timestamp"))
}

async fn lease_state<'e, E>(
    executor: E,
    id: &str,
    execution_id: &str,
) -> Result<JobLeaseState, StorageError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query("SELECT status, execution_id FROM background_jobs WHERE id = ?")
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else {
        return Ok(JobLeaseState::Lost);
    };
    if row.try_get::<Option<String>, _>("execution_id")?.as_deref() != Some(execution_id) {
        return Ok(JobLeaseState::Lost);
    }
    match row.try_get::<String, _>("status")?.as_str() {
        "running" => Ok(JobLeaseState::Active),
        "cancel_requested" => Ok(JobLeaseState::CancellationRequested),
        _ => Ok(JobLeaseState::Lost),
    }
}

async fn apply_finish(
    transaction: &mut Transaction<'_, Sqlite>,
    claim: &JobClaim,
    finish: &JobFinish,
    state: JobLeaseState,
) -> Result<(), StorageError> {
    let (status, phase, result, error, clear_started, finished) =
        if state == JobLeaseState::CancellationRequested {
            ("cancelled", "Cancelled", None, None, false, true)
        } else {
            match finish {
                JobFinish::Succeeded(result) => (
                    "succeeded",
                    "Complete",
                    Some(serde_json::to_string(result).map_err(StorageError::JobSerialization)?),
                    None,
                    false,
                    true,
                ),
                JobFinish::Failed(error) => {
                    ("failed", "Failed", None, Some(error.as_str()), false, true)
                }
                JobFinish::Cancelled => ("cancelled", "Cancelled", None, None, false, true),
                JobFinish::Interrupted { restartable: true } => {
                    ("queued", "Queued for restart", None, None, true, false)
                }
                JobFinish::Interrupted { restartable: false } => (
                    "failed",
                    "Interrupted",
                    None,
                    Some("Job was interrupted during server shutdown."),
                    false,
                    true,
                ),
            }
        };
    sqlx::query(
        "UPDATE background_jobs SET status = ?, progress_phase = ?, \
         result_json = COALESCE(?, result_json), error = ?, \
         started_at = CASE WHEN ? THEN NULL ELSE started_at END, \
         finished_at = CASE WHEN ? THEN CURRENT_TIMESTAMP ELSE NULL END, \
         updated_at = CURRENT_TIMESTAMP, execution_id = NULL \
         WHERE id = ? AND execution_id = ? \
           AND status IN ('running', 'cancel_requested')",
    )
    .bind(status)
    .bind(phase)
    .bind(result)
    .bind(error)
    .bind(clear_started)
    .bind(finished)
    .bind(&claim.job.id)
    .bind(&claim.execution_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn box_storage<E>(error: E) -> Box<dyn std::error::Error + Send + Sync>
where
    StorageError: From<E>,
{
    Box::new(StorageError::from(error))
}

#[cfg(test)]
#[path = "jobs_fault_tests.rs"]
mod fault_tests;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::error::Error;

    use music_application::jobs::{
        JobCheckpointPolicy, JobDefinition, JobFinish, JobLane, JobListFilter, JobProgress,
        JobRepository, JobStatus, NewJob,
    };
    use serde_json::{Map, Value, json};
    use tempfile::tempdir;

    use crate::{SqliteStorage, SqliteStorageOptions};

    fn definition(kind: &'static str, lane: JobLane, restartable: bool) -> JobDefinition {
        JobDefinition {
            kind,
            schema_version: 1,
            lane,
            restartable,
            checkpoint_policy: JobCheckpointPolicy::Replace,
        }
    }

    fn new_job(id: &str, definition: JobDefinition) -> NewJob {
        NewJob {
            id: id.to_owned(),
            definition,
            parameters: Map::from_iter([("steps".to_owned(), json!(3))]),
            retry_of_id: None,
        }
    }

    #[tokio::test]
    async fn claims_are_lane_isolated_and_lease_guarded() -> Result<(), Box<dyn Error + Send + Sync>>
    {
        let directory = tempdir()?;
        let storage =
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("db"))).await?;
        let local = new_job("local", definition("test.local", JobLane::Local, true));
        let provider = new_job(
            "provider",
            definition("assistant.model.test", JobLane::Provider, false),
        );
        storage.create(&provider).await?;
        storage.create(&local).await?;

        let claim = storage
            .claim_next(JobLane::Local)
            .await?
            .ok_or("missing claim")?;
        assert_eq!(claim.job.id, "local");
        assert_eq!(storage.claim_next(JobLane::Local).await?, None);
        storage
            .update_progress(&claim, &JobProgress::new(1, Some(3), "Testing", "one")?)
            .await?;
        let mut stale = claim.clone();
        stale.execution_id = "stale".to_owned();
        assert_eq!(
            storage
                .checkpoint(
                    &stale,
                    &Map::from_iter([("ignored".to_owned(), Value::Bool(true))])
                )
                .await?,
            music_application::jobs::JobLeaseState::Lost
        );
        storage
            .finish(
                &claim,
                &JobFinish::Succeeded(Map::from_iter([("processed".to_owned(), json!(1))])),
            )
            .await?;
        let finished = storage.get("local").await?.ok_or("missing job")?;
        assert_eq!(finished.status, JobStatus::Succeeded);
        assert_eq!(finished.progress_current, 1);
        assert_eq!(
            finished
                .result
                .and_then(|value| value.get("processed").cloned()),
            Some(json!(1))
        );
        Ok(())
    }

    #[tokio::test]
    async fn recovery_requeues_only_matching_restartable_contracts()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let storage =
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("db"))).await?;
        let restartable = definition("test.restartable", JobLane::Local, true);
        let provider = definition("assistant.model.test", JobLane::Provider, false);
        storage
            .create(&new_job("restartable", restartable.clone()))
            .await?;
        storage
            .create(&new_job("provider", provider.clone()))
            .await?;
        let local_claim = storage.claim_next(JobLane::Local).await?.ok_or("local")?;
        let provider_claim = storage
            .claim_next(JobLane::Provider)
            .await?
            .ok_or("provider")?;
        assert_eq!(local_claim.job.status, JobStatus::Running);
        assert_eq!(provider_claim.job.status, JobStatus::Running);

        let definitions = BTreeMap::from([
            (restartable.kind.to_owned(), restartable),
            (provider.kind.to_owned(), provider),
        ]);
        assert_eq!(storage.recover_interrupted(&definitions).await?, 2);
        assert_eq!(
            storage.get("restartable").await?.ok_or("local")?.status,
            JobStatus::Queued
        );
        assert_eq!(
            storage.get("provider").await?.ok_or("provider")?.status,
            JobStatus::Failed
        );
        assert_eq!(storage.list(&JobListFilter::default()).await?.len(), 2);
        Ok(())
    }
}
