use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use music_analysis::{
    AnalysisExecutor, AudioContextAnalyzer, AudioContextDocument, AudioContextError,
    AudioContextPerformance, VoiceAnalysisDocument, VoiceAnalysisError, VoiceBackend,
    VoiceContextPreparation,
};
use music_application::assistant::{
    AssistantRepository, ContextState, ContextWrite, LIBRARY_CONTEXT_JOB_KIND,
    LOCAL_CONTEXT_ANALYZER_ID, LOCAL_CONTEXT_IMPLEMENTATION_ID, LocalAnalysisRepository,
    context_source_signature,
};
use music_application::jobs::{
    JobRecord, JobService, JobStatus, SpawnedJobCoordinator, start_job_coordinator,
};
use music_application::library::{
    DiscoveredTrack, LibraryMutationRepository, LibraryRepository, ReconciliationCommit,
};
use music_domain::{IndexedTrack, LibraryPath, TrackMetadata};
use music_media::LibraryRoot;
use music_storage::{SqliteStorage, SqliteStorageOptions};
use serde_json::{Map, Value, json};
use tempfile::{TempDir, tempdir};
use tokio::sync::Notify;
use tokio::time::{sleep, timeout};

use super::{ContextAnalysisJobHandler, context_with_voice};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
const DEADLINE: Duration = Duration::from_secs(10);

#[derive(Debug, Default)]
struct ControlledAnalyzer {
    calls: Mutex<BTreeMap<String, usize>>,
    blocked: Mutex<Option<String>>,
    entered: Notify,
}

impl ControlledAnalyzer {
    fn hold(&self, name: Option<&str>) -> TestResult {
        *self.blocked.lock().map_err(|_| "analysis gate poisoned")? = name.map(str::to_owned);
        Ok(())
    }

    fn calls(&self, name: &str) -> TestResult<usize> {
        Ok(self
            .calls
            .lock()
            .map_err(|_| "analysis count poisoned")?
            .get(name)
            .copied()
            .unwrap_or(0))
    }

    async fn wait_until_held(&self) -> TestResult {
        timeout(DEADLINE, self.entered.notified()).await?;
        Ok(())
    }
}

fn document(marker: &str) -> AudioContextDocument {
    AudioContextDocument {
        completeness: "full",
        summary: Map::from_iter([
            (
                "schema_version".to_owned(),
                json!(LOCAL_CONTEXT_ANALYZER_ID),
            ),
            ("fixture_marker".to_owned(), json!(marker)),
            ("voice".to_owned(), json!({"status":"not_classified"})),
        ]),
        timeline: vec![Map::from_iter([("start_s".to_owned(), json!(0.0))])],
        sections: Vec::new(),
        technical: Map::new(),
        stages: Map::from_iter([("voice".to_owned(), json!({"status":"not_configured"}))]),
        performance: AudioContextPerformance {
            audio_seconds: 1.0,
            elapsed_seconds: 0.0,
            stage_seconds: Map::new(),
        },
    }
}

impl AudioContextAnalyzer for ControlledAnalyzer {
    fn analyzer_id(&self) -> &'static str {
        LOCAL_CONTEXT_ANALYZER_ID
    }
    fn implementation_id(&self) -> &'static str {
        LOCAL_CONTEXT_IMPLEMENTATION_ID
    }

    fn analyze(
        &self,
        path: &Path,
        cancelled: &AtomicBool,
        voice: VoiceContextPreparation,
    ) -> Result<AudioContextDocument, AudioContextError> {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(AudioContextError::Decode)?;
        *self
            .calls
            .lock()
            .map_err(|_| AudioContextError::Decode)?
            .entry(name.to_owned())
            .or_default() += 1;
        let deadline = Instant::now() + DEADLINE;
        if self
            .blocked
            .lock()
            .map_err(|_| AudioContextError::Decode)?
            .as_deref()
            == Some(name)
        {
            self.entered.notify_one();
            while self
                .blocked
                .lock()
                .map_err(|_| AudioContextError::Decode)?
                .as_deref()
                == Some(name)
            {
                if cancelled.load(Ordering::Relaxed) {
                    return Err(AudioContextError::Cancelled);
                }
                if Instant::now() >= deadline {
                    return Err(AudioContextError::Decode);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err(AudioContextError::Cancelled);
        }
        let mut result = document("fresh");
        if voice == VoiceContextPreparation::Deferred {
            result.completeness = "partial";
            result
                .stages
                .insert("voice".to_owned(), json!({"status":"pending"}));
        }
        Ok(result)
    }
}

struct Fixture {
    directory: TempDir,
    storage: Arc<SqliteStorage>,
    analyzer: Arc<ControlledAnalyzer>,
    tracks: Vec<IndexedTrack>,
}

impl Fixture {
    async fn new() -> TestResult<Self> {
        let directory = tempdir()?;
        fs::create_dir(directory.path().join("music"))?;
        let storage = Arc::new(
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
        );
        let mut discovered = Vec::new();
        for name in ["a.wav", "b.wav", "c.wav"] {
            fs::write(
                directory.path().join("music").join(name),
                b"synthetic analyzer input",
            )?;
            discovered.push(DiscoveredTrack {
                path: LibraryPath::parse(name)?,
                metadata: TrackMetadata {
                    title: name.to_owned(),
                    artist: String::new(),
                    album_artist: String::new(),
                    album: String::new(),
                    release_date: String::new(),
                    original_release_date: String::new(),
                    composer: String::new(),
                    track_no: None,
                    disc_no: None,
                    year: None,
                    genre: String::new(),
                    bpm: None,
                },
                duration: Duration::from_secs(1),
                size_bytes: 24,
                mtime_unix_seconds: 1,
            });
        }
        let generation = storage.begin_reconciliation().await?.generation;
        assert!(matches!(
            storage
                .commit_reconciliation(generation, discovered)
                .await?,
            ReconciliationCommit::Applied { .. }
        ));
        let mut tracks = storage.all_tracks().await?;
        tracks.sort_by(|left, right| left.path.cmp(&right.path));
        storage
            .patch_tags(
                &tracks.iter().map(|track| track.id).collect::<Vec<_>>(),
                &["keep-authored".to_owned()],
                &[],
            )
            .await?;
        Ok(Self {
            directory,
            storage,
            analyzer: Arc::new(ControlledAnalyzer::default()),
            tracks,
        })
    }

    async fn start(&self) -> TestResult<SpawnedJobCoordinator> {
        self.start_with_voice(VoiceBackend::initialize(None, "ffmpeg"))
            .await
    }

    async fn start_with_voice(&self, voice: VoiceBackend) -> TestResult<SpawnedJobCoordinator> {
        let handler = ContextAnalysisJobHandler::new(
            self.storage.clone(),
            LibraryRoot::open(self.directory.path().join("music"))?,
            AnalysisExecutor::new(1)?,
            self.analyzer.clone(),
            voice.status,
            voice.worker_factory,
        );
        Ok(start_job_coordinator(self.storage.clone(), vec![Arc::new(handler)]).await?)
    }

    fn parameters(&self, force: bool) -> Value {
        json!({"force":force,"scope":{"type":"tracks","track_ids":self.tracks.iter().map(|track| track.id.get()).collect::<Vec<_>>()}})
    }

    async fn state(&self, index: usize) -> TestResult<ContextState> {
        self.storage
            .context_states(LOCAL_CONTEXT_ANALYZER_ID)
            .await?
            .into_iter()
            .find(|state| state.track_id == self.tracks[index].id)
            .ok_or_else(|| "context missing".into())
    }

    async fn seed_prior_results(&self) -> TestResult {
        for track in &self.tracks {
            let existing = document("before-forced-rebuild");
            assert!(
                self.storage
                    .store_context(
                        LOCAL_CONTEXT_ANALYZER_ID,
                        LOCAL_CONTEXT_IMPLEMENTATION_ID,
                        None,
                        "previous-success",
                        &ContextWrite {
                            track_id: track.id,
                            source_signature: context_source_signature(
                                track,
                                LOCAL_CONTEXT_IMPLEMENTATION_ID,
                                None
                            )?,
                            completeness: existing.completeness.to_owned(),
                            summary: existing.summary,
                            timeline: existing.timeline,
                            sections: existing.sections,
                            technical: existing.technical,
                            stages: existing.stages,
                        }
                    )
                    .await?
            );
        }
        Ok(())
    }

    async fn check_authored_state(&self) -> TestResult {
        let tracks = AssistantRepository::tracks(self.storage.as_ref()).await?;
        assert!(
            tracks
                .iter()
                .all(|track| track.manual_tags == ["keep-authored"])
        );
        for track in &self.tracks {
            assert_eq!(
                fs::read(
                    self.directory
                        .path()
                        .join("music")
                        .join(track.path.as_str())
                )?,
                b"synthetic analyzer input"
            );
        }
        Ok(())
    }
}

async fn wait_for(service: &JobService, id: &str, status: JobStatus) -> TestResult<JobRecord> {
    timeout(DEADLINE, async {
        loop {
            let job = service.get(id).await?.ok_or("job missing")?;
            if job.status == status {
                return Ok(job);
            }
            if !job.status.is_active() {
                return Err(
                    format!("unexpected job outcome: {:?}: {:?}", job.status, job.error).into(),
                );
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await?
}

async fn stop(coordinator: SpawnedJobCoordinator) -> TestResult {
    coordinator.service.shutdown();
    timeout(DEADLINE, coordinator.local_task).await???;
    timeout(DEADLINE, coordinator.provider_task).await???;
    Ok(())
}

#[tokio::test]
async fn forced_rebuild_retries_reuse_only_completed_results_from_the_retry_chain() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.seed_prior_results().await?;
    fixture.analyzer.hold(Some("b.wav"))?;
    let coordinator = fixture.start().await?;
    let original = coordinator
        .service
        .enqueue(LIBRARY_CONTEXT_JOB_KIND, fixture.parameters(true))
        .await?;
    fixture.analyzer.wait_until_held().await?;
    let first_saved = fixture.state(0).await?;
    coordinator.service.cancel(&original.id).await?;
    wait_for(&coordinator.service, &original.id, JobStatus::Cancelled).await?;

    fixture.analyzer.hold(Some("c.wav"))?;
    let retry = coordinator.service.retry(&original.id).await?;
    fixture.analyzer.wait_until_held().await?;
    let first_after_retry = fixture.state(0).await?;
    let second_saved = fixture.state(1).await?;
    coordinator.service.cancel(&retry.id).await?;
    wait_for(&coordinator.service, &retry.id, JobStatus::Cancelled).await?;

    fixture.analyzer.hold(None)?;
    let last = coordinator.service.retry(&retry.id).await?;
    wait_for(&coordinator.service, &last.id, JobStatus::Succeeded).await?;
    stop(coordinator).await?;
    assert_eq!(
        first_after_retry, first_saved,
        "retry must not rewrite an already completed new-pipeline result"
    );
    assert_eq!(fixture.state(0).await?, first_saved);
    assert_eq!(fixture.state(1).await?, second_saved);
    assert_eq!(fixture.state(2).await?.job_id, last.id);
    assert_eq!(
        [
            fixture.analyzer.calls("a.wav")?,
            fixture.analyzer.calls("b.wav")?,
            fixture.analyzer.calls("c.wav")?
        ],
        [1, 2, 2]
    );

    // A deliberately new forced job still recomputes every recording.
    let coordinator = fixture.start().await?;
    let fresh = coordinator
        .service
        .enqueue(LIBRARY_CONTEXT_JOB_KIND, fixture.parameters(true))
        .await?;
    wait_for(&coordinator.service, &fresh.id, JobStatus::Succeeded).await?;
    stop(coordinator).await?;
    assert_eq!(
        [
            fixture.analyzer.calls("a.wav")?,
            fixture.analyzer.calls("b.wav")?,
            fixture.analyzer.calls("c.wav")?
        ],
        [2, 3, 3]
    );
    fixture.check_authored_state().await
}

#[tokio::test]
async fn forced_rebuild_restart_reuses_committed_work_after_database_reopen() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.analyzer.hold(Some("b.wav"))?;
    let coordinator = fixture.start().await?;
    let original = coordinator
        .service
        .enqueue(LIBRARY_CONTEXT_JOB_KIND, fixture.parameters(true))
        .await?;
    fixture.analyzer.wait_until_held().await?;
    let saved = fixture.state(0).await?;
    stop(coordinator).await?;
    let Fixture {
        directory,
        storage,
        analyzer,
        tracks,
    } = fixture;
    storage.close().await;
    drop(storage);
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let fixture = Fixture {
        directory,
        storage,
        analyzer,
        tracks,
    };

    fixture.analyzer.hold(None)?;
    let resumed = fixture.start().await?;
    let finished = wait_for(&resumed.service, &original.id, JobStatus::Succeeded).await?;
    stop(resumed).await?;
    assert_eq!(finished.attempts, 2);
    assert_eq!(fixture.state(0).await?, saved);
    assert_eq!(
        [
            fixture.analyzer.calls("a.wav")?,
            fixture.analyzer.calls("b.wav")?,
            fixture.analyzer.calls("c.wav")?
        ],
        [1, 2, 1]
    );
    assert_eq!(
        finished.result.as_ref().ok_or("missing result")?["current_contexts"],
        3
    );
    fixture.check_authored_state().await
}

#[tokio::test]
async fn rebuild_keeps_missing_files_failed_then_recovers_without_repeating_successes() -> TestResult
{
    let fixture = Fixture::new().await?;
    let missing = fixture.directory.path().join("music/b.wav");
    fs::remove_file(&missing)?;
    let coordinator = fixture.start().await?;
    let original = coordinator
        .service
        .enqueue(LIBRARY_CONTEXT_JOB_KIND, fixture.parameters(false))
        .await?;
    let failed_track = wait_for(&coordinator.service, &original.id, JobStatus::Succeeded).await?;
    let report = failed_track.result.as_ref().ok_or("missing result")?;
    assert_eq!(report["failed"], 1);
    assert_eq!(
        report["passes"]["audio_context"]["status"],
        "complete_with_failures"
    );
    assert_eq!(report["current_contexts"], 2);
    assert_eq!(
        fixture
            .storage
            .analysis_failures(LOCAL_CONTEXT_ANALYZER_ID)
            .await?
            .len(),
        1
    );
    let first_saved = fixture.state(0).await?;
    let third_saved = fixture.state(2).await?;

    fs::write(missing, b"synthetic analyzer input")?;
    let retry = coordinator
        .service
        .enqueue(LIBRARY_CONTEXT_JOB_KIND, fixture.parameters(false))
        .await?;
    let finished = wait_for(&coordinator.service, &retry.id, JobStatus::Succeeded).await?;
    stop(coordinator).await?;
    assert_eq!(
        finished.result.as_ref().ok_or("missing result")?["failed"],
        0
    );
    assert!(
        fixture
            .storage
            .analysis_failures(LOCAL_CONTEXT_ANALYZER_ID)
            .await?
            .is_empty()
    );
    assert_eq!(fixture.state(0).await?, first_saved);
    assert_eq!(fixture.state(2).await?, third_saved);
    assert_eq!(
        [
            fixture.analyzer.calls("a.wav")?,
            fixture.analyzer.calls("b.wav")?,
            fixture.analyzer.calls("c.wav")?
        ],
        [1, 1, 1]
    );
    fixture.check_authored_state().await
}

#[tokio::test]
async fn forced_retry_reanalyzes_a_completed_recording_when_its_source_changed() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.analyzer.hold(Some("b.wav"))?;
    let coordinator = fixture.start().await?;
    let original = coordinator
        .service
        .enqueue(LIBRARY_CONTEXT_JOB_KIND, fixture.parameters(true))
        .await?;
    fixture.analyzer.wait_until_held().await?;
    let saved = fixture.state(0).await?;
    coordinator.service.cancel(&original.id).await?;
    wait_for(&coordinator.service, &original.id, JobStatus::Cancelled).await?;

    let discovered = fixture
        .tracks
        .iter()
        .enumerate()
        .map(|(index, track)| DiscoveredTrack {
            path: track.path.clone(),
            metadata: track.metadata.clone(),
            duration: track.duration,
            size_bytes: track.size_bytes,
            mtime_unix_seconds: track.mtime_unix_seconds + i64::from(index == 0),
        })
        .collect();
    let generation = fixture.storage.begin_reconciliation().await?.generation;
    assert!(matches!(
        fixture
            .storage
            .commit_reconciliation(generation, discovered)
            .await?,
        ReconciliationCommit::Applied { .. }
    ));
    fixture.analyzer.hold(None)?;
    let retry = coordinator.service.retry(&original.id).await?;
    wait_for(&coordinator.service, &retry.id, JobStatus::Succeeded).await?;
    stop(coordinator).await?;
    let refreshed = fixture.state(0).await?;
    assert_ne!(refreshed.source_signature, saved.source_signature);
    assert_eq!(refreshed.job_id, retry.id);
    assert_eq!(
        [
            fixture.analyzer.calls("a.wav")?,
            fixture.analyzer.calls("b.wav")?,
            fixture.analyzer.calls("c.wav")?
        ],
        [2, 2, 1]
    );
    fixture.check_authored_state().await
}

#[tokio::test]
async fn forced_retry_retries_failed_voice_without_rebuilding_factual_context() -> TestResult {
    let Some(voice) = configured_voice()? else {
        return Ok(());
    };
    let signature = voice
        .status
        .source_signature
        .clone()
        .ok_or("voice signature missing")?;
    let fixture = Fixture::new().await?;
    fixture.analyzer.hold(Some("b.wav"))?;
    let coordinator = fixture.start_with_voice(voice).await?;
    let original = coordinator
        .service
        .enqueue(LIBRARY_CONTEXT_JOB_KIND, fixture.parameters(true))
        .await?;
    fixture.analyzer.wait_until_held().await?;
    coordinator.service.cancel(&original.id).await?;
    wait_for(&coordinator.service, &original.id, JobStatus::Cancelled).await?;

    // Reproduce a prior optional-voice failure saved before a later cancellation.
    // Optional failure deliberately leaves the factual context marked full.
    let failed = context_with_voice(
        fixture.tracks[0].id,
        &fixture.state(0).await?,
        VoiceAnalysisDocument::unavailable(&VoiceAnalysisError::Decode, 0.0),
    )?;
    assert!(
        fixture
            .storage
            .store_context(
                LOCAL_CONTEXT_ANALYZER_ID,
                LOCAL_CONTEXT_IMPLEMENTATION_ID,
                Some(&signature),
                &original.id,
                &failed
            )
            .await?
    );
    assert_eq!(fixture.state(0).await?.completeness, "full");

    fixture.analyzer.hold(None)?;
    let retry = coordinator.service.retry(&original.id).await?;
    let finished = wait_for(&coordinator.service, &retry.id, JobStatus::Succeeded).await?;
    stop(coordinator).await?;
    assert_eq!(
        fixture.analyzer.calls("a.wav")?,
        1,
        "failed voice must be retried without repeating its completed factual pass"
    );
    let retried = fixture.state(0).await?;
    assert_eq!(retried.job_id, retry.id);
    let stages: Value = serde_json::from_str(&retried.stages_json)?;
    assert_eq!(stages["voice"]["reason"], failed.stages["voice"]["reason"]);
    let result = finished.result.as_ref().ok_or("missing result")?;
    assert_eq!(result["voice_performance"]["tracks_profiled"], 3);
    // The deliberately invalid synthetic audio fails real decoding. It must stay a
    // visible voice failure, never become a completed classification on retry.
    assert_eq!(result["passes"]["voice_detection"]["failed_tracks"], 3);
    assert_eq!(result["passes"]["voice_detection"]["completed_tracks"], 0);
    fixture.check_authored_state().await
}

#[tokio::test]
async fn new_analysis_retries_only_failed_voice_without_repeating_current_facts() -> TestResult {
    let Some(voice) = configured_voice()? else {
        return Ok(());
    };
    let signature = voice
        .status
        .source_signature
        .clone()
        .ok_or("voice signature missing")?;
    let fixture = Fixture::new().await?;
    let coordinator = fixture.start_with_voice(voice).await?;
    let original = coordinator
        .service
        .enqueue(LIBRARY_CONTEXT_JOB_KIND, fixture.parameters(false))
        .await?;
    let first = wait_for(&coordinator.service, &original.id, JobStatus::Succeeded).await?;
    assert_eq!(
        first.result.as_ref().ok_or("missing result")?["voice_performance"]["tracks_profiled"],
        3
    );
    let saved = fixture.state(0).await?;

    // Keep a previously classified row beside the two actual decoder failures.
    // This is controlled recovery state, not a listening judgment.
    let classified = VoiceAnalysisDocument {
        summary: Map::from_iter([
            ("status".to_owned(), json!("classified")),
            ("voice_score".to_owned(), json!(0.2)),
        ]),
        stage: Map::from_iter([("status".to_owned(), json!("complete"))]),
        elapsed_seconds: 0.0,
        prediction_windows: 1,
    };
    let write = context_with_voice(fixture.tracks[2].id, &fixture.state(2).await?, classified)?;
    assert!(
        fixture
            .storage
            .store_context(
                LOCAL_CONTEXT_ANALYZER_ID,
                LOCAL_CONTEXT_IMPLEMENTATION_ID,
                Some(&signature),
                &original.id,
                &write
            )
            .await?
    );
    let complete = fixture.state(2).await?;

    let retry = coordinator
        .service
        .enqueue(LIBRARY_CONTEXT_JOB_KIND, fixture.parameters(false))
        .await?;
    let retried = wait_for(&coordinator.service, &retry.id, JobStatus::Succeeded).await?;
    stop(coordinator).await?;
    let result = retried.result.as_ref().ok_or("missing result")?;
    assert_eq!(
        result["voice_performance"]["tracks_profiled"], 2,
        "new analysis must retry unavailable voice, without redoing successful voice"
    );
    assert_eq!(result["performance"]["tracks_profiled"], 0);
    assert_eq!(result["passes"]["voice_detection"]["failed_tracks"], 2);
    assert_eq!(result["passes"]["voice_detection"]["completed_tracks"], 1);
    assert_eq!(fixture.state(2).await?, complete);
    let after = fixture.state(0).await?;
    assert_eq!(after.job_id, retry.id);
    assert_eq!(after.source_signature, saved.source_signature);
    assert_eq!(after.summary_json, saved.summary_json);
    assert_eq!(after.timeline_json, saved.timeline_json);
    assert_eq!(after.sections_json, saved.sections_json);
    assert_eq!(after.technical_json, saved.technical_json);
    assert_eq!(after.stages_json, saved.stages_json);
    for name in ["a.wav", "b.wav", "c.wav"] {
        assert_eq!(fixture.analyzer.calls(name)?, 1);
    }

    // Force still requests a new factual pass, even for previous voice failures.
    let coordinator = fixture
        .start_with_voice(configured_voice()?.ok_or("voice fixture missing")?)
        .await?;
    let forced = coordinator
        .service
        .enqueue(LIBRARY_CONTEXT_JOB_KIND, fixture.parameters(true))
        .await?;
    let rebuilt = wait_for(&coordinator.service, &forced.id, JobStatus::Succeeded).await?;
    stop(coordinator).await?;
    assert_eq!(
        rebuilt.result.as_ref().ok_or("missing result")?["performance"]["tracks_profiled"],
        3
    );
    assert_eq!(
        rebuilt.result.as_ref().ok_or("missing result")?["voice_performance"]["tracks_profiled"],
        3
    );
    for name in ["a.wav", "b.wav", "c.wav"] {
        assert_eq!(fixture.analyzer.calls(name)?, 2);
    }
    fixture.check_authored_state().await
}

fn configured_voice() -> TestResult<Option<VoiceBackend>> {
    let Some(model) = std::env::var_os("MUSIC_TEST_VOICE_MODEL") else {
        return Ok(None);
    };
    let ffmpeg = std::env::var_os("MUSIC_TEST_FFMPEG").unwrap_or_else(|| "ffmpeg".into());
    let voice = VoiceBackend::initialize(Some(Path::new(&model)), ffmpeg);
    assert!(
        voice.status.is_ready(),
        "explicit voice fixture must be ready"
    );
    Ok(Some(voice))
}

#[tokio::test]
async fn same_job_restart_keeps_voice_failures_without_repeating_the_attempt() -> TestResult {
    let Some(voice) = configured_voice()? else {
        return Ok(());
    };
    let signature = voice
        .status
        .source_signature
        .clone()
        .ok_or("voice signature missing")?;
    let fixture = Fixture::new().await?;
    fixture.analyzer.hold(Some("b.wav"))?;
    let coordinator = fixture.start_with_voice(voice).await?;
    let original = coordinator
        .service
        .enqueue(LIBRARY_CONTEXT_JOB_KIND, fixture.parameters(true))
        .await?;
    fixture.analyzer.wait_until_held().await?;
    stop(coordinator).await?;

    // Recreate a committed voice failure before a restart, while other recordings
    // still need work. Same-job recovery must retain this completed attempt.
    let write = context_with_voice(
        fixture.tracks[0].id,
        &fixture.state(0).await?,
        VoiceAnalysisDocument::unavailable(&VoiceAnalysisError::Decode, 0.0),
    )?;
    assert!(
        fixture
            .storage
            .store_context(
                LOCAL_CONTEXT_ANALYZER_ID,
                LOCAL_CONTEXT_IMPLEMENTATION_ID,
                Some(&signature),
                &original.id,
                &write
            )
            .await?
    );
    let failed = fixture.state(0).await?;
    fixture.analyzer.hold(None)?;
    let resumed = fixture
        .start_with_voice(configured_voice()?.ok_or("voice fixture missing")?)
        .await?;
    let finished = wait_for(&resumed.service, &original.id, JobStatus::Succeeded).await?;
    stop(resumed).await?;
    assert_eq!(finished.attempts, 2);
    assert_eq!(fixture.state(0).await?, failed);
    assert_eq!(fixture.analyzer.calls("a.wav")?, 1);
    let result = finished.result.as_ref().ok_or("missing result")?;
    assert_eq!(result["voice_performance"]["tracks_profiled"], 2);
    assert_eq!(result["passes"]["voice_detection"]["failed_tracks"], 3);
    assert_eq!(result["passes"]["voice_detection"]["completed_tracks"], 0);
    fixture.check_authored_state().await
}
