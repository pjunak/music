use std::path::Path;
use std::time::Duration;

use music_application::jobs::timing::{
    JobTimingReport, JobTimingSample, MAX_JOB_TIMING_SAMPLE, summarize_job_timing,
};
use music_application::jobs::{JobLane, JobStatus};
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow};

use crate::StorageError;

/// Inspect a live database or an offline copy without migrations or writer ownership.
/// A single SELECT gives the sample and truncation flag from the same snapshot.
pub async fn read_job_timing_report(
    path: &Path,
    limit: u16,
) -> Result<JobTimingReport, StorageError> {
    if !(1..=MAX_JOB_TIMING_SAMPLE).contains(&limit) {
        return Err(StorageError::InvalidOption(
            "job timing limit must be between 1 and 10000",
        ));
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .read_only(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    // Do not load job parameters, result JSON, errors, progress prose, or IDs.
    let result = sqlx::query(
        "SELECT kind, lane, status, attempts, \
         CAST(strftime('%s', created_at) AS INTEGER) AS created_at, \
         CAST(strftime('%s', started_at) AS INTEGER) AS started_at, \
         CAST(strftime('%s', finished_at) AS INTEGER) AS finished_at \
         FROM background_jobs ORDER BY background_jobs.created_at DESC, rowid DESC LIMIT ?",
    )
    .bind(i64::from(limit) + 1)
    .fetch_all(&pool)
    .await;
    pool.close().await;
    let rows = result?;
    let has_more = rows.len() > usize::from(limit);
    let samples = rows
        .iter()
        .take(usize::from(limit))
        .map(row_to_sample)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(summarize_job_timing(&samples, limit, has_more))
}

fn row_to_sample(row: &SqliteRow) -> Result<JobTimingSample, StorageError> {
    Ok(JobTimingSample {
        kind: row.try_get("kind")?,
        lane: JobLane::parse(row.try_get("lane")?)
            .map_err(|_| StorageError::InvalidJobRecord("unknown lane"))?,
        status: JobStatus::parse(row.try_get("status")?)
            .map_err(|_| StorageError::InvalidJobRecord("unknown status"))?,
        attempts: u32::try_from(row.try_get::<i64, _>("attempts")?)
            .map_err(|_| StorageError::InvalidJobRecord("invalid attempts"))?,
        created_at: row.try_get("created_at")?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
    })
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use music_application::jobs::{JobCheckpointPolicy, JobDefinition, JobRepository, NewJob};
    use serde_json::Map;
    use tempfile::tempdir;

    use super::*;
    use crate::{SqliteStorage, SqliteStorageOptions};

    #[tokio::test]
    async fn timing_reads_bounded_snapshot_while_writer_is_open_without_payloads_or_migrations()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let path = directory.path().join("db");
        let storage = SqliteStorage::open(SqliteStorageOptions::new(&path)).await?;
        for id in ["z-private", "m-private", "a-private"] {
            storage
                .create(&NewJob {
                    id: id.to_owned(),
                    definition: JobDefinition {
                        kind: "test.timing",
                        lane: JobLane::Provider,
                        schema_version: 1,
                        restartable: false,
                        checkpoint_policy: JobCheckpointPolicy::Replace,
                    },
                    parameters: Map::new(),
                    retry_of_id: None,
                })
                .await?;
        }
        // Unparseable private payloads must not prevent a timing-only inspection.
        sqlx::query(
            "UPDATE background_jobs SET parameters_json = 'private payload', \
            result_json = 'private result', error = 'private error', attempts = 1, \
            status = 'succeeded', created_at = '2026-09-05 12:00:00', \
            started_at = '2026-09-05 12:00:05', finished_at = '2026-09-05 12:00:15'",
        )
        .execute(&storage.pool)
        .await?;
        sqlx::query("UPDATE background_jobs SET started_at = 'malformed' WHERE id = 'a-private'")
            .execute(&storage.pool)
            .await?;
        let report = read_job_timing_report(&path, 2).await?;
        assert_eq!(report.sampled_jobs, 2);
        assert!(report.has_more);
        assert_eq!(report.groups[0].queue_wait_seconds.measured, 1);
        assert_eq!(report.groups[0].queue_wait_seconds.unavailable, 1);
        assert_eq!(report.groups[0].queue_wait_seconds.p95, Some(5));
        assert_eq!(report.groups[0].execution_seconds.p95, Some(10));
        assert!(!serde_json::to_string(&report)?.contains("private"));
        assert!(!read_job_timing_report(&path, 3).await?.has_more);
        let payloads: Vec<String> =
            sqlx::query_scalar("SELECT parameters_json FROM background_jobs")
                .fetch_all(&storage.pool)
                .await?;
        assert_eq!(payloads, vec!["private payload"; 3]);
        storage.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn timing_rejects_invalid_limits_and_never_initializes_missing_or_old_databases()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let path = directory.path().join("db");
        for limit in [0, MAX_JOB_TIMING_SAMPLE + 1] {
            assert!(matches!(
                read_job_timing_report(&path, limit).await,
                Err(StorageError::InvalidOption(_))
            ));
        }
        assert!(read_job_timing_report(&path, 1).await.is_err());
        assert!(!path.exists());
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true),
            )
            .await?;
        sqlx::query("CREATE TABLE legacy_marker (value TEXT)")
            .execute(&pool)
            .await?;
        assert!(read_job_timing_report(&path, 1).await.is_err());
        let tables: Vec<String> =
            sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table'")
                .fetch_all(&pool)
                .await?;
        assert_eq!(tables, ["legacy_marker"]);
        pool.close().await;
        Ok(())
    }
}
