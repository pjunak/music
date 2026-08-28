use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use music_analysis::{AnalysisExecutor, AudioSignalAnalyzer, AudioSignalError};
use music_application::assistant::{
    AUDIO_ANALYSIS_JOB_KIND, AnalysisFailureState, AnalysisFailureWrite, AnalysisState,
    AnalysisWrite, LOCAL_AUDIO_ANALYZER_ID, LocalAnalysisRepository, audio_source_signature,
};
use music_application::jobs::{
    JobCheckpointPolicy, JobDefinition, JobExecutionContext, JobHandler, JobHandlerError,
    JobHandlerFuture, JobLane, JobProgress,
};
use music_media::LibraryRoot;
use serde::Deserialize;
use serde_json::{Map, Value, json};

const FAILURE_SAMPLE_LIMIT: usize = 20;
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub(crate) struct AudioAnalysisJobHandler {
    repository: Arc<dyn LocalAnalysisRepository>,
    root: LibraryRoot,
    executor: AnalysisExecutor,
    analyzer: Arc<dyn AudioSignalAnalyzer>,
}

impl AudioAnalysisJobHandler {
    pub(crate) fn new(
        repository: Arc<dyn LocalAnalysisRepository>,
        root: LibraryRoot,
        executor: AnalysisExecutor,
        analyzer: Arc<dyn AudioSignalAnalyzer>,
    ) -> Self {
        Self {
            repository,
            root,
            executor,
            analyzer,
        }
    }

    async fn analyze_track(
        &self,
        context: &JobExecutionContext,
        path: std::path::PathBuf,
    ) -> Result<Result<music_analysis::AudioSignalProfile, AudioSignalError>, JobHandlerError> {
        let cancellation = Arc::new(AtomicBool::new(false));
        let guard = CancelAnalysisOnDrop(Arc::clone(&cancellation));
        let analyzer = Arc::clone(&self.analyzer);
        let task = self
            .executor
            .execute(move || analyzer.analyze(&path, &cancellation));
        tokio::pin!(task);
        loop {
            tokio::select! {
                result = &mut task => {
                    drop(guard);
                    return result
                        .map_err(|_| JobHandlerError::new("audio analysis executor failed"));
                }
                () = tokio::time::sleep(CANCELLATION_POLL_INTERVAL) => {
                    if let Err(error) = context.check_cancelled().await {
                        guard.cancel();
                        let _ = task.await;
                        return Err(JobHandlerError::from_execution(error));
                    }
                }
            }
        }
    }
}

impl JobHandler for AudioAnalysisJobHandler {
    fn definition(&self) -> JobDefinition {
        JobDefinition {
            kind: AUDIO_ANALYSIS_JOB_KIND,
            schema_version: 1,
            lane: JobLane::Local,
            restartable: true,
            checkpoint_policy: JobCheckpointPolicy::Replace,
        }
    }

    fn execute<'a>(
        &'a self,
        context: &'a JobExecutionContext,
        parameters: Map<String, Value>,
    ) -> JobHandlerFuture<'a> {
        Box::pin(async move {
            let parameters =
                serde_json::from_value::<AudioAnalysisParameters>(Value::Object(parameters))
                    .map_err(|_| JobHandlerError::new("invalid audio analysis parameters"))?;
            let tracks = self
                .repository
                .all_tracks()
                .await
                .map_err(|_| JobHandlerError::new("audio analysis storage failed"))?;
            let states = self
                .repository
                .analysis_states(LOCAL_AUDIO_ANALYZER_ID)
                .await
                .map_err(|_| JobHandlerError::new("audio analysis storage failed"))?;
            let failures = self
                .repository
                .analysis_failures(LOCAL_AUDIO_ANALYZER_ID)
                .await
                .map_err(|_| JobHandlerError::new("audio analysis storage failed"))?;
            let state_by_track = by_track(&states);
            let failure_by_track = failures_by_track(&failures);
            let mut work = Vec::new();
            let mut checkpointed = 0_u64;
            for track in &tracks {
                let signature = audio_source_signature(track)
                    .map_err(|_| JobHandlerError::new("track audio fingerprint failed"))?;
                let profile = state_by_track.get(&track.id).copied();
                let failure = failure_by_track.get(&track.id).copied();
                let completed_by_job = profile.is_some_and(|state| {
                    state.job_id == context.job_id() && state.source_signature == signature
                }) || failure.is_some_and(|state| {
                    state.job_id == context.job_id() && state.source_signature == signature
                });
                if completed_by_job {
                    checkpointed = checkpointed.saturating_add(1);
                    continue;
                }
                let current_failure =
                    failure.is_some_and(|state| state.source_signature == signature);
                let current_profile =
                    profile.is_some_and(|state| state.source_signature == signature);
                if !parameters.force && current_profile && !current_failure {
                    continue;
                }
                work.push((track, signature));
            }

            let starting = context.progress_current().max(checkpointed);
            let work_count = u64::try_from(work.len())
                .map_err(|_| JobHandlerError::new("audio analysis is too large"))?;
            let total = starting.saturating_add(work_count);
            context
                .update_progress(
                    JobProgress::new(
                        starting,
                        Some(total),
                        "Measuring audio signals",
                        format!("{} tracks need signal analysis", work.len()),
                    )
                    .map_err(|_| JobHandlerError::new("invalid audio analysis progress"))?,
                )
                .await
                .map_err(JobHandlerError::from_execution)?;

            for (index, (track, signature)) in work.iter().enumerate() {
                context
                    .check_cancelled()
                    .await
                    .map_err(JobHandlerError::from_execution)?;
                let analysis = match self.root.resolve_existing(&track.path) {
                    Ok(path) => self.analyze_track(context, path).await,
                    Err(_) => Ok(Err(AudioSignalError::MissingFile)),
                }?;
                match analysis {
                    Ok(profile) => {
                        let write = AnalysisWrite {
                            track_id: track.id,
                            source_signature: signature.clone(),
                            energy: profile.energy,
                            brightness: profile.brightness,
                            tension: profile.tension,
                            moods: Vec::new(),
                            evidence: profile.evidence,
                            metrics: profile.metrics,
                            confidence: profile.confidence,
                        };
                        let _stored = self
                            .repository
                            .store_audio_analysis(LOCAL_AUDIO_ANALYZER_ID, context.job_id(), &write)
                            .await
                            .map_err(|_| JobHandlerError::new("audio analysis storage failed"))?;
                    }
                    Err(AudioSignalError::Cancelled) => {
                        context
                            .check_cancelled()
                            .await
                            .map_err(JobHandlerError::from_execution)?;
                        return Err(JobHandlerError::new("audio analysis stopped unexpectedly"));
                    }
                    Err(error) => {
                        let failure = AnalysisFailureWrite {
                            track_id: track.id,
                            source_signature: signature.clone(),
                            error: format!("AudioSignalError: {error}"),
                        };
                        let _stored = self
                            .repository
                            .store_analysis_failure(
                                LOCAL_AUDIO_ANALYZER_ID,
                                context.job_id(),
                                &failure,
                            )
                            .await
                            .map_err(|_| JobHandlerError::new("audio analysis storage failed"))?;
                    }
                }
                let processed = u64::try_from(index.saturating_add(1))
                    .map_err(|_| JobHandlerError::new("audio analysis is too large"))?;
                let current = starting.saturating_add(processed);
                context
                    .update_progress(
                        JobProgress::new(
                            current,
                            Some(total),
                            "Measuring audio signals",
                            format!("Processed {current} of {total} tracks"),
                        )
                        .map_err(|_| JobHandlerError::new("invalid audio analysis progress"))?,
                    )
                    .await
                    .map_err(JobHandlerError::from_execution)?;
            }

            let final_tracks = self
                .repository
                .all_tracks()
                .await
                .map_err(|_| JobHandlerError::new("audio analysis storage failed"))?;
            let final_states = self
                .repository
                .analysis_states(LOCAL_AUDIO_ANALYZER_ID)
                .await
                .map_err(|_| JobHandlerError::new("audio analysis storage failed"))?;
            let final_failures = self
                .repository
                .analysis_failures(LOCAL_AUDIO_ANALYZER_ID)
                .await
                .map_err(|_| JobHandlerError::new("audio analysis storage failed"))?;
            result(
                &final_tracks,
                &final_states,
                &final_failures,
                context.job_id(),
            )
        })
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AudioAnalysisParameters {
    force: bool,
}

#[derive(Debug)]
struct CancelAnalysisOnDrop(Arc<AtomicBool>);

impl CancelAnalysisOnDrop {
    fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

impl Drop for CancelAnalysisOnDrop {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn by_track(states: &[AnalysisState]) -> BTreeMap<music_domain::TrackId, &AnalysisState> {
    states.iter().map(|state| (state.track_id, state)).collect()
}

fn failures_by_track(
    states: &[AnalysisFailureState],
) -> BTreeMap<music_domain::TrackId, &AnalysisFailureState> {
    states.iter().map(|state| (state.track_id, state)).collect()
}

fn result(
    tracks: &[music_domain::IndexedTrack],
    states: &[AnalysisState],
    failures: &[AnalysisFailureState],
    job_id: &str,
) -> Result<Value, JobHandlerError> {
    let states = by_track(states);
    let failures = failures_by_track(failures);
    let mut current_profiles = Vec::new();
    let mut current_failures = Vec::new();
    let mut paths = BTreeMap::new();
    for track in tracks {
        paths.insert(track.id, track.path.as_str());
        let signature = audio_source_signature(track)
            .map_err(|_| JobHandlerError::new("track audio fingerprint failed"))?;
        if let Some(state) = states.get(&track.id)
            && state.source_signature == signature
        {
            current_profiles.push(*state);
        }
        if let Some(failure) = failures.get(&track.id)
            && failure.source_signature == signature
        {
            current_failures.push(*failure);
        }
    }
    let updated = current_profiles
        .iter()
        .filter(|state| state.job_id == job_id)
        .count();
    let failed = current_failures
        .iter()
        .filter(|state| state.job_id == job_id)
        .count();
    let failure_samples = current_failures
        .iter()
        .filter(|state| state.job_id == job_id)
        .take(FAILURE_SAMPLE_LIMIT)
        .map(|state| {
            json!({
                "track_id": state.track_id.get(),
                "path": paths.get(&state.track_id).copied().unwrap_or(""),
                "error": state.error,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "tracks": tracks.len(),
        "updated": updated,
        "failed": failed,
        "unchanged": tracks.len().saturating_sub(updated).saturating_sub(failed),
        "current_profiles": current_profiles.len(),
        "current_failures": current_failures.len(),
        "failure_samples": failure_samples,
        "analyzer": LOCAL_AUDIO_ANALYZER_ID,
    }))
}
