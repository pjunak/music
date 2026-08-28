use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream};
use music_analysis::{
    AnalysisExecutor, AudioContextAnalyzer, AudioContextDocument, AudioContextError,
    AudioContextPerformance, AudioSignalAnalyzer, AudioSignalError,
};
use music_application::assistant::{
    AUDIO_ANALYSIS_JOB_KIND, AnalysisFailureState, AnalysisFailureWrite, AnalysisState,
    AnalysisWrite, ContextScope, ContextState, ContextWrite, LIBRARY_CONTEXT_JOB_KIND,
    LOCAL_AUDIO_ANALYZER_ID, LOCAL_CONTEXT_ANALYZER_ID, LOCAL_CONTEXT_IMPLEMENTATION_ID,
    LocalAnalysisRepository, VoiceAnalyzerStatus, audio_source_signature, context_source_signature,
    parse_context_state,
};
use music_application::jobs::{
    JobCheckpointPolicy, JobDefinition, JobExecutionContext, JobHandler, JobHandlerError,
    JobHandlerFuture, JobLane, JobProgress,
};
use music_media::LibraryRoot;
use serde::{Deserialize, Serialize};
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

#[derive(Debug)]
pub(crate) struct ContextAnalysisJobHandler {
    repository: Arc<dyn LocalAnalysisRepository>,
    root: LibraryRoot,
    executor: AnalysisExecutor,
    analyzer: Arc<dyn AudioContextAnalyzer>,
    voice_analyzer: VoiceAnalyzerStatus,
}

impl ContextAnalysisJobHandler {
    pub(crate) fn new(
        repository: Arc<dyn LocalAnalysisRepository>,
        root: LibraryRoot,
        executor: AnalysisExecutor,
        analyzer: Arc<dyn AudioContextAnalyzer>,
        voice_analyzer: VoiceAnalyzerStatus,
    ) -> Self {
        Self {
            repository,
            root,
            executor,
            analyzer,
            voice_analyzer,
        }
    }

    async fn analyze_track(
        &self,
        context: &JobExecutionContext,
        path: std::path::PathBuf,
    ) -> Result<Result<AudioContextDocument, AudioContextError>, JobHandlerError> {
        let cancellation = Arc::new(AtomicBool::new(false));
        let guard = CancelAnalysisOnDrop(Arc::clone(&cancellation));
        let analyzer = Arc::clone(&self.analyzer);
        let defer_voice = self.voice_analyzer.is_ready();
        let task = self
            .executor
            .execute(move || analyzer.analyze(&path, &cancellation, defer_voice));
        tokio::pin!(task);
        loop {
            tokio::select! {
                result = &mut task => {
                    drop(guard);
                    return result
                        .map_err(|_| JobHandlerError::new("context analysis executor failed"));
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

impl JobHandler for ContextAnalysisJobHandler {
    fn definition(&self) -> JobDefinition {
        JobDefinition {
            kind: LIBRARY_CONTEXT_JOB_KIND,
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
            let started = Instant::now();
            let parameters =
                serde_json::from_value::<ContextAnalysisParameters>(Value::Object(parameters))
                    .map_err(|_| JobHandlerError::new("invalid context analysis parameters"))?;
            let scope = parameters
                .scope
                .to_scope()
                .map_err(|_| JobHandlerError::new("invalid context analysis scope"))?;
            let all_tracks = self
                .repository
                .all_tracks()
                .await
                .map_err(|_| JobHandlerError::new("context analysis storage failed"))?;
            let tracks = scope
                .select(&all_tracks)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            let states = self
                .repository
                .context_states(LOCAL_CONTEXT_ANALYZER_ID)
                .await
                .map_err(|_| JobHandlerError::new("context analysis storage failed"))?;
            let failures = self
                .repository
                .analysis_failures(LOCAL_CONTEXT_ANALYZER_ID)
                .await
                .map_err(|_| JobHandlerError::new("context analysis storage failed"))?;
            let state_by_track = context_by_track(&states);
            let failure_by_track = failures_by_track(&failures);
            let voice_signature = self.voice_analyzer.source_signature.as_deref();
            let mut work = Vec::new();
            let mut checkpointed = 0_u64;
            for track in &tracks {
                let signature = context_source_signature(
                    track,
                    LOCAL_CONTEXT_IMPLEMENTATION_ID,
                    voice_signature,
                )
                .map_err(|_| JobHandlerError::new("track context fingerprint failed"))?;
                let state = state_by_track.get(&track.id).copied();
                let failure = failure_by_track.get(&track.id).copied();
                let current_context = state.is_some_and(|state| {
                    state.source_signature == signature && parse_context_state(state).is_some()
                });
                let current_failure =
                    failure.is_some_and(|failure| failure.source_signature == signature);
                let completed_by_job = state.is_some_and(|state| {
                    state.job_id == context.job_id()
                        && state.source_signature == signature
                        && parse_context_state(state).is_some()
                }) || failure.is_some_and(|failure| {
                    failure.job_id == context.job_id() && failure.source_signature == signature
                });
                if completed_by_job {
                    checkpointed = checkpointed.saturating_add(1);
                    continue;
                }
                if !parameters.force && current_context && !current_failure {
                    continue;
                }
                work.push((track.clone(), signature));
            }

            let starting = context.progress_current().max(checkpointed);
            let work_count = u64::try_from(work.len())
                .map_err(|_| JobHandlerError::new("context analysis is too large"))?;
            let total = starting.saturating_add(work_count);
            let active_workers = self.executor.worker_count().max(1).min(work.len());
            let stream_concurrency = active_workers.max(1);
            context
                .checkpoint(context_checkpoint(
                    "running",
                    starting as usize,
                    0,
                    tracks.len(),
                    &self.voice_analyzer,
                ))
                .await
                .map_err(JobHandlerError::from_execution)?;
            context
                .update_progress(
                    JobProgress::new(
                        starting,
                        Some(total),
                        "Analyzing audio context",
                        format!("{} tracks need context analysis", work.len()),
                    )
                    .map_err(|_| JobHandlerError::new("invalid context analysis progress"))?,
                )
                .await
                .map_err(JobHandlerError::from_execution)?;

            let signal_started = Instant::now();
            let mut completed = usize::try_from(starting).unwrap_or(usize::MAX);
            let mut failed = 0_usize;
            let mut performance = Vec::new();
            let mut results = stream::iter(work.into_iter().map(|(track, signature)| async move {
                let analysis = match self.root.resolve_existing(&track.path) {
                    Ok(path) => self.analyze_track(context, path).await,
                    Err(_) => Ok(Err(AudioContextError::MissingFile)),
                };
                (track, signature, analysis)
            }))
            .buffer_unordered(stream_concurrency);
            while let Some((track, signature, analysis)) = results.next().await {
                match analysis? {
                    Ok(document) => {
                        performance.push(document.performance.clone());
                        let write = ContextWrite {
                            track_id: track.id,
                            source_signature: signature.clone(),
                            completeness: document.completeness.to_owned(),
                            confidence: document.confidence.to_owned(),
                            summary: document.summary,
                            timeline: document.timeline,
                            sections: document.sections,
                            technical: document.technical,
                            stages: document.stages,
                        };
                        self.repository
                            .store_context(
                                LOCAL_CONTEXT_ANALYZER_ID,
                                LOCAL_CONTEXT_IMPLEMENTATION_ID,
                                voice_signature,
                                context.job_id(),
                                &write,
                            )
                            .await
                            .map_err(|_| JobHandlerError::new("context analysis storage failed"))?;
                    }
                    Err(AudioContextError::Cancelled) => {
                        context
                            .check_cancelled()
                            .await
                            .map_err(JobHandlerError::from_execution)?;
                        return Err(JobHandlerError::new(
                            "context analysis stopped unexpectedly",
                        ));
                    }
                    Err(error) => {
                        let failure = AnalysisFailureWrite {
                            track_id: track.id,
                            source_signature: signature,
                            error: format!("AudioContextError: {error}"),
                        };
                        let stored = self
                            .repository
                            .store_context_failure(
                                LOCAL_CONTEXT_ANALYZER_ID,
                                LOCAL_CONTEXT_IMPLEMENTATION_ID,
                                voice_signature,
                                context.job_id(),
                                &failure,
                            )
                            .await
                            .map_err(|_| JobHandlerError::new("context analysis storage failed"))?;
                        if stored {
                            failed = failed.saturating_add(1);
                        }
                    }
                }
                completed = completed.saturating_add(1);
                let current = u64::try_from(completed)
                    .map_err(|_| JobHandlerError::new("context analysis is too large"))?;
                context
                    .checkpoint(context_checkpoint(
                        "running",
                        completed.saturating_sub(failed),
                        failed,
                        tracks.len(),
                        &self.voice_analyzer,
                    ))
                    .await
                    .map_err(JobHandlerError::from_execution)?;
                context
                    .update_progress(
                        JobProgress::new(
                            current.min(total),
                            Some(total),
                            "Analyzing audio context",
                            format!("Processed {} of {} tracks", current.min(total), total),
                        )
                        .map_err(|_| JobHandlerError::new("invalid context analysis progress"))?,
                    )
                    .await
                    .map_err(JobHandlerError::from_execution)?;
            }
            drop(results);
            let wall_seconds = signal_started.elapsed().as_secs_f64();

            let final_states = self
                .repository
                .context_states(LOCAL_CONTEXT_ANALYZER_ID)
                .await
                .map_err(|_| JobHandlerError::new("context analysis storage failed"))?;
            let final_failures = self
                .repository
                .analysis_failures(LOCAL_CONTEXT_ANALYZER_ID)
                .await
                .map_err(|_| JobHandlerError::new("context analysis storage failed"))?;
            context_result(
                &tracks,
                &final_states,
                &final_failures,
                context.job_id(),
                &parameters.scope,
                &self.voice_analyzer,
                active_workers,
                &performance,
                wall_seconds,
                started.elapsed().as_secs_f64(),
            )
        })
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ContextAnalysisParameters {
    pub(crate) force: bool,
    pub(crate) scope: ContextScopeParameters,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ContextScopeParameters {
    #[serde(rename = "type")]
    pub(crate) kind: ContextScopeKind,
    pub(crate) path: String,
    pub(crate) recursive: bool,
    pub(crate) track_ids: Vec<i64>,
}

impl Default for ContextScopeParameters {
    fn default() -> Self {
        Self {
            kind: ContextScopeKind::All,
            path: String::new(),
            recursive: true,
            track_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ContextScopeKind {
    #[default]
    All,
    Folder,
    Tracks,
}

impl ContextScopeParameters {
    pub(crate) fn to_scope(&self) -> Result<ContextScope, ()> {
        if self.path.len() > 1_024 || self.track_ids.len() > 5_000 {
            return Err(());
        }
        match self.kind {
            ContextScopeKind::All if self.path.is_empty() && self.track_ids.is_empty() => {
                Ok(ContextScope::All)
            }
            ContextScopeKind::Folder if self.track_ids.is_empty() => {
                let path = if self.path.is_empty() {
                    None
                } else {
                    Some(music_domain::LibraryPath::parse(self.path.clone()).map_err(|_| ())?)
                };
                Ok(ContextScope::Folder {
                    path,
                    recursive: self.recursive,
                })
            }
            ContextScopeKind::Tracks if self.path.is_empty() && !self.track_ids.is_empty() => {
                let mut seen = BTreeSet::new();
                let mut ids = Vec::new();
                for value in &self.track_ids {
                    let id = music_domain::TrackId::new(*value).map_err(|_| ())?;
                    if seen.insert(id) {
                        ids.push(id);
                    }
                }
                Ok(ContextScope::Tracks(ids))
            }
            _ => Err(()),
        }
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

fn context_by_track(states: &[ContextState]) -> BTreeMap<music_domain::TrackId, &ContextState> {
    states.iter().map(|state| (state.track_id, state)).collect()
}

fn context_checkpoint(
    status: &str,
    completed: usize,
    failed: usize,
    total: usize,
    voice_analyzer: &VoiceAnalyzerStatus,
) -> Map<String, Value> {
    let voice_enabled = voice_analyzer.is_ready();
    object(json!({
        "schema_version": "assistant-library-context-job-progress/v1",
        "passes": {
            "audio_context": {
                "status": status,
                "completed_tracks": completed,
                "failed_tracks": failed,
                "skipped_tracks": 0,
                "total_tracks": total,
            },
            "voice_detection": {
                "status": if voice_enabled { "waiting" } else { "not_available" },
                "completed_tracks": 0,
                "failed_tracks": 0,
                "skipped_tracks": if voice_enabled { 0 } else { total },
                "total_tracks": total,
            },
        },
    }))
}

#[allow(clippy::too_many_arguments)]
fn context_result(
    tracks: &[music_domain::IndexedTrack],
    states: &[ContextState],
    failures: &[AnalysisFailureState],
    job_id: &str,
    scope: &ContextScopeParameters,
    voice_analyzer: &VoiceAnalyzerStatus,
    worker_count: usize,
    performance: &[AudioContextPerformance],
    signal_wall_seconds: f64,
    total_wall_seconds: f64,
) -> Result<Value, JobHandlerError> {
    let state_by_track = context_by_track(states);
    let failure_by_track = failures_by_track(failures);
    let voice_signature = voice_analyzer.source_signature.as_deref();
    let mut current_contexts = Vec::new();
    let mut current_failures = Vec::new();
    let mut failures_from_job = Vec::new();
    let mut updated = 0_usize;
    for track in tracks {
        let signature =
            context_source_signature(track, LOCAL_CONTEXT_IMPLEMENTATION_ID, voice_signature)
                .map_err(|_| JobHandlerError::new("track context fingerprint failed"))?;
        let current = state_by_track.get(&track.id).copied().filter(|state| {
            state.source_signature == signature && parse_context_state(state).is_some()
        });
        if let Some(state) = current {
            if state.job_id == job_id {
                updated = updated.saturating_add(1);
            }
            current_contexts.push(state);
        }
        if let Some(failure) = failure_by_track
            .get(&track.id)
            .copied()
            .filter(|failure| failure.source_signature == signature)
        {
            if current.is_none() {
                current_failures.push(failure);
            }
            if failure.job_id == job_id {
                failures_from_job.push((track, failure));
            }
        }
    }
    let failed = failures_from_job.len();
    let failure_samples = failures_from_job
        .iter()
        .take(FAILURE_SAMPLE_LIMIT)
        .map(|(track, failure)| {
            json!({
                "track_id": track.id.get(),
                "path": track.path.as_str(),
                "error": failure.error,
            })
        })
        .collect::<Vec<_>>();
    let voice_enabled = voice_analyzer.is_ready();
    let scope = serde_json::to_value(scope)
        .map_err(|_| JobHandlerError::new("context analysis scope could not be encoded"))?;
    Ok(json!({
        "schema_version": "assistant-library-context-job-result/v3",
        "analyzer": LOCAL_CONTEXT_ANALYZER_ID,
        "scope": scope,
        "tracks": tracks.len(),
        "analysis_workers": worker_count,
        "voice_workers": 0,
        "passes": {
            "audio_context": {
                "status": if failed == 0 { "complete" } else { "complete_with_failures" },
                "completed_tracks": current_contexts.len(),
                "failed_tracks": current_failures.len(),
                "skipped_tracks": 0,
                "total_tracks": tracks.len(),
            },
            "voice_detection": {
                "status": if voice_enabled { "waiting" } else { "not_available" },
                "completed_tracks": 0,
                "failed_tracks": 0,
                "skipped_tracks": if voice_enabled { 0 } else { tracks.len() },
                "total_tracks": tracks.len(),
            },
        },
        "updated": updated,
        "failed": failed,
        "unchanged": tracks.len().saturating_sub(updated).saturating_sub(failed),
        "current_contexts": current_contexts.len(),
        "current_failures": current_failures.len(),
        "failure_samples": failure_samples,
        "performance": context_performance(performance, signal_wall_seconds),
        "voice_performance": {
            "schema_version": "library-context-voice-performance/v1",
            "tracks_profiled": 0,
            "wall_seconds": 0.0,
            "worker_seconds": 0.0,
        },
        "wall_seconds": round_seconds(total_wall_seconds),
    }))
}

fn context_performance(samples: &[AudioContextPerformance], wall_seconds: f64) -> Value {
    let mut stage_seconds = BTreeMap::<String, f64>::new();
    let mut audio_seconds = 0.0;
    let mut worker_seconds = 0.0;
    for sample in samples {
        audio_seconds += sample.audio_seconds;
        worker_seconds += sample.elapsed_seconds;
        for (stage, seconds) in &sample.stage_seconds {
            if let Some(seconds) = seconds.as_f64() {
                *stage_seconds.entry(stage.clone()).or_default() += seconds;
            }
        }
    }
    let measured = stage_seconds.values().sum::<f64>();
    let dominant = stage_seconds
        .iter()
        .max_by(|left, right| left.1.total_cmp(right.1))
        .map(|(stage, _)| stage.clone());
    let rounded = stage_seconds
        .iter()
        .map(|(stage, seconds)| (stage.clone(), round_seconds(*seconds)))
        .collect::<BTreeMap<_, _>>();
    let shares = if measured > 0.0 {
        stage_seconds
            .iter()
            .map(|(stage, seconds)| {
                (
                    stage.clone(),
                    ((*seconds * 1_000.0 / measured).round() / 10.0),
                )
            })
            .collect::<BTreeMap<_, _>>()
    } else {
        BTreeMap::new()
    };
    json!({
        "schema_version": "library-context-performance/v1",
        "tracks_profiled": samples.len(),
        "wall_seconds": round_seconds(wall_seconds),
        "worker_seconds": round_seconds(worker_seconds),
        "audio_seconds": round_seconds(audio_seconds),
        "audio_realtime_factor": if wall_seconds > 0.0 {
            Some(round_seconds(audio_seconds / wall_seconds))
        } else {
            None
        },
        "dominant_stage": dominant,
        "stage_seconds": rounded,
        "stage_share_percent": shares,
    })
}

fn object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

fn round_seconds(value: f64) -> f64 {
    (value * 1_000.0).round() / 1_000.0
}
