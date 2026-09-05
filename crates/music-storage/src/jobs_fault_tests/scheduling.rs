use super::*;

#[derive(Debug)]
struct SteppedProviderHandler {
    release_step: Notify,
}

impl JobHandler for SteppedProviderHandler {
    fn definition(&self) -> JobDefinition {
        definition("test.scheduling.long", JobLane::Provider, false)
    }

    fn execute<'a>(
        &'a self,
        context: &'a music_application::jobs::JobExecutionContext,
        _parameters: Map<String, Value>,
    ) -> JobHandlerFuture<'a> {
        Box::pin(async move {
            for step in 1..=3 {
                context
                    .checkpoint(Map::from_iter([("step".to_owned(), json!(step))]))
                    .await
                    .map_err(JobHandlerError::from_execution)?;
                self.release_step.notified().await;
                context
                    .check_cancelled()
                    .await
                    .map_err(JobHandlerError::from_execution)?;
            }
            Ok(json!({"steps": 3}))
        })
    }
}

#[tokio::test]
async fn provider_waits_for_whole_job_while_local_lane_progresses() -> TestResult {
    let directory = tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("db"))).await?,
    );
    let long = Arc::new(SteppedProviderHandler {
        release_step: Notify::new(),
    });
    let short = Arc::new(ImmediateHandler::new(
        "test.scheduling.short",
        JobLane::Provider,
        false,
    ));
    let local = Arc::new(ImmediateHandler::new(
        "test.scheduling.local",
        JobLane::Local,
        true,
    ));
    storage.create(&new_job("long", long.definition())).await?;
    let coordinator = start_job_coordinator(
        storage.clone(),
        vec![
            long.clone() as Arc<dyn JobHandler>,
            short.clone(),
            local.clone(),
        ],
    )
    .await?;
    wait_for_job(&storage, "long", |job| job.result.is_some()).await?;
    coordinator
        .service
        .enqueue(short.definition().kind, json!({}))
        .await?;
    let local_job = coordinator
        .service
        .enqueue(local.definition().kind, json!({}))
        .await?;
    wait_for_job(&storage, &local_job.id, |job| {
        job.status == JobStatus::Succeeded
    })
    .await?;
    for step in 1..=3 {
        wait_for_job(&storage, "long", |job| {
            job.result.as_ref().and_then(|result| result.get("step")) == Some(&json!(step))
        })
        .await?;
        assert_eq!(
            short.effect_count(),
            0,
            "short job must wait through every long-job checkpoint"
        );
        let timing = crate::read_job_timing_report(&directory.path().join("db"), 10).await?;
        let short_group = timing
            .groups
            .iter()
            .find(|group| group.kind == short.definition().kind)
            .ok_or("short group missing")?;
        assert_eq!(short_group.statuses["queued"], 1);
        assert_eq!(short_group.queue_wait_seconds.measured, 0);
        long.release_step.notify_one();
    }
    wait_for_job(&storage, "long", |job| job.status == JobStatus::Succeeded).await?;
    let jobs = storage.list(&JobListFilter::default()).await?;
    let short_job = jobs
        .iter()
        .find(|job| job.kind == short.definition().kind)
        .ok_or("short job missing")?;
    wait_for_job(&storage, &short_job.id, |job| {
        job.status == JobStatus::Succeeded
    })
    .await?;
    assert_eq!(short.effect_count(), 1);
    assert_eq!(local.effect_count(), 1);
    stop_coordinator(coordinator).await?;
    storage.close().await;
    Ok(())
}
