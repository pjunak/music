use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream};
use music_analysis::{
    AnalysisExecutor, AudioContextAnalyzer, AudioContextDocument, AudioContextError,
    AudioContextPerformance, AudioSignalAnalyzer, AudioSignalError, VoiceAnalysisDocument,
    VoiceAnalysisError, VoiceContextPreparation, VoiceWorker,
};
use music_application::assistant::{
    AUDIO_ANALYSIS_JOB_KIND, AnalysisFailureState, AnalysisFailureWrite, AnalysisState,
    AnalysisWrite, ContextScope, ContextState, ContextWrite, CurrentTrackContext,
    LIBRARY_CONTEXT_JOB_KIND, LOCAL_AUDIO_ANALYZER_ID, LOCAL_CONTEXT_ANALYZER_ID,
    LOCAL_CONTEXT_IMPLEMENTATION_ID, LocalAnalysisRepository, VoiceAnalyzerStatus,
    audio_source_signature, context_source_signature, parse_context_state,
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
            drop(state_by_track);
            drop(failure_by_track);
            drop(states);
            drop(failures);

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
    voice_worker: Option<VoiceWorker>,
}

impl ContextAnalysisJobHandler {
    pub(crate) fn new(
        repository: Arc<dyn LocalAnalysisRepository>,
        root: LibraryRoot,
        executor: AnalysisExecutor,
        analyzer: Arc<dyn AudioContextAnalyzer>,
        voice_analyzer: VoiceAnalyzerStatus,
        voice_worker: Option<VoiceWorker>,
    ) -> Self {
        Self {
            repository,
            root,
            executor,
            analyzer,
            voice_analyzer,
            voice_worker,
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
        let voice = if self.voice_analyzer.is_ready() && self.voice_worker.is_some() {
            VoiceContextPreparation::Deferred
        } else if self.voice_analyzer.status == "unavailable" || self.voice_analyzer.is_ready() {
            VoiceContextPreparation::Unavailable {
                reason: self
                    .voice_analyzer
                    .reason
                    .clone()
                    .unwrap_or_else(|| "runtime_missing".to_owned()),
            }
        } else {
            VoiceContextPreparation::NotConfigured
        };
        let task = self
            .executor
            .execute(move || analyzer.analyze(&path, &cancellation, voice));
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

    async fn analyze_voice_track(
        &self,
        context: &JobExecutionContext,
        path: std::path::PathBuf,
    ) -> Result<Result<VoiceAnalysisDocument, VoiceAnalysisError>, JobHandlerError> {
        let Some(worker) = self.voice_worker.as_ref().cloned() else {
            return Ok(Err(VoiceAnalysisError::WorkerUnavailable));
        };
        let cancellation = Arc::new(AtomicBool::new(false));
        let guard = CancelAnalysisOnDrop(Arc::clone(&cancellation));
        let task = worker.analyze(path, cancellation);
        tokio::pin!(task);
        loop {
            tokio::select! {
                result = &mut task => {
                    drop(guard);
                    return Ok(result);
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
            drop(all_tracks);
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
            let voice_enabled = self.voice_analyzer.is_ready() && self.voice_worker.is_some();
            let mut signal_work = Vec::new();
            let mut audio_completed = 0_usize;
            let mut audio_failed = 0_usize;
            let mut initially_pending_voice = 0_usize;
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
                let parsed = state
                    .filter(|state| state.source_signature == signature)
                    .and_then(parse_context_state);
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
                }
                // A partial row is the durable boundary between the signal and
                // voice passes. Preserve it even for a forced retry so a
                // cancelled voice pass never causes another signal decode.
                let signal_is_current = parsed.as_ref().is_some_and(|parsed| {
                    parsed.completeness == "partial" || !parameters.force || completed_by_job
                });
                if signal_is_current && !current_failure {
                    audio_completed = audio_completed.saturating_add(1);
                    if voice_enabled
                        && parsed.as_ref().and_then(context_voice_stage_status) == Some("pending")
                    {
                        initially_pending_voice = initially_pending_voice.saturating_add(1);
                    }
                    continue;
                }
                if current_failure
                    && failure.is_some_and(|failure| failure.job_id == context.job_id())
                {
                    audio_failed = audio_failed.saturating_add(1);
                    continue;
                }
                signal_work.push((track.clone(), signature));
            }
            drop(state_by_track);
            drop(failure_by_track);
            drop(states);
            drop(failures);

            let starting = context.progress_current().max(checkpointed);
            let signal_work_count = u64::try_from(signal_work.len())
                .map_err(|_| JobHandlerError::new("context analysis is too large"))?;
            let expected_voice_count = if voice_enabled {
                u64::try_from(signal_work.len().saturating_add(initially_pending_voice))
                    .map_err(|_| JobHandlerError::new("context analysis is too large"))?
            } else {
                0
            };
            let initial_total = starting
                .saturating_add(signal_work_count)
                .saturating_add(expected_voice_count);
            let active_workers = self.executor.worker_count().min(signal_work.len());
            let stream_concurrency = active_workers.max(1);
            let initial_audio = ContextPassProgress {
                status: if signal_work.is_empty() {
                    "complete"
                } else {
                    "running"
                },
                completed: audio_completed,
                failed: audio_failed,
                skipped: 0,
                total: tracks.len(),
            };
            let initial_voice = if voice_enabled {
                ContextPassProgress {
                    status: "waiting",
                    completed: 0,
                    failed: 0,
                    skipped: 0,
                    total: tracks.len(),
                }
            } else {
                ContextPassProgress::not_available(tracks.len())
            };
            context
                .checkpoint(context_checkpoint(initial_audio, initial_voice))
                .await
                .map_err(JobHandlerError::from_execution)?;
            context
                .update_progress(
                    JobProgress::new(
                        starting,
                        Some(initial_total),
                        "Analyzing audio context",
                        format!("{} tracks need context analysis", signal_work.len()),
                    )
                    .map_err(|_| JobHandlerError::new("invalid context analysis progress"))?,
                )
                .await
                .map_err(JobHandlerError::from_execution)?;

            let signal_started = Instant::now();
            let mut progress_current = starting;
            let mut performance = ContextPerformanceAggregate::default();
            let mut results = stream::iter(signal_work.into_iter().map(
                |(track, signature)| async move {
                    let analysis = match self.root.resolve_existing(&track.path) {
                        Ok(path) => self.analyze_track(context, path).await,
                        Err(_) => Ok(Err(AudioContextError::MissingFile)),
                    };
                    (track, signature, analysis)
                },
            ))
            .buffer_unordered(stream_concurrency);
            while let Some((track, signature, analysis)) = results.next().await {
                match analysis? {
                    Ok(document) => {
                        performance.observe(&document.performance);
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
                        let stored = self
                            .repository
                            .store_context(
                                LOCAL_CONTEXT_ANALYZER_ID,
                                LOCAL_CONTEXT_IMPLEMENTATION_ID,
                                voice_signature,
                                context.job_id(),
                                &write,
                            )
                            .await
                            .map_err(|_| JobHandlerError::new("context analysis storage failed"))?;
                        if stored {
                            audio_completed = audio_completed.saturating_add(1);
                        }
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
                            audio_failed = audio_failed.saturating_add(1);
                        }
                    }
                }
                progress_current = progress_current.saturating_add(1);
                let audio_progress = ContextPassProgress {
                    status: "running",
                    completed: audio_completed,
                    failed: audio_failed,
                    skipped: 0,
                    total: tracks.len(),
                };
                context
                    .checkpoint(context_checkpoint(audio_progress, initial_voice))
                    .await
                    .map_err(JobHandlerError::from_execution)?;
                context
                    .update_progress(
                        JobProgress::new(
                            progress_current.min(initial_total),
                            Some(initial_total),
                            "Analyzing audio context",
                            format!(
                                "Audio context: {} of {} tracks processed",
                                audio_completed.saturating_add(audio_failed),
                                tracks.len()
                            ),
                        )
                        .map_err(|_| JobHandlerError::new("invalid context analysis progress"))?,
                    )
                    .await
                    .map_err(JobHandlerError::from_execution)?;
            }
            drop(results);
            let signal_wall_seconds = signal_started.elapsed().as_secs_f64();

            let states_after_signal = self
                .repository
                .context_states(LOCAL_CONTEXT_ANALYZER_ID)
                .await
                .map_err(|_| JobHandlerError::new("context analysis storage failed"))?;
            let failures_after_signal = self
                .repository
                .analysis_failures(LOCAL_CONTEXT_ANALYZER_ID)
                .await
                .map_err(|_| JobHandlerError::new("context analysis storage failed"))?;
            let states_after_signal_by_track = context_by_track(&states_after_signal);
            let failures_after_signal_by_track = failures_by_track(&failures_after_signal);
            audio_completed = 0;
            audio_failed = 0;
            let mut voice_work = Vec::new();
            let mut voice_completed = 0_usize;
            let mut voice_failed = 0_usize;
            let mut voice_skipped = 0_usize;
            for track in &tracks {
                let signature = context_source_signature(
                    track,
                    LOCAL_CONTEXT_IMPLEMENTATION_ID,
                    voice_signature,
                )
                .map_err(|_| JobHandlerError::new("track context fingerprint failed"))?;
                let current_state = states_after_signal_by_track
                    .get(&track.id)
                    .copied()
                    .filter(|state| state.source_signature == signature);
                let parsed = current_state.and_then(parse_context_state);
                if let Some(parsed) = parsed {
                    audio_completed = audio_completed.saturating_add(1);
                    if voice_enabled {
                        match context_voice_stage_status(&parsed) {
                            Some("pending") => {
                                if let Some(state) = current_state {
                                    voice_work.push((track.id, track.path.clone(), state.clone()));
                                }
                            }
                            Some("unavailable") => {
                                voice_failed = voice_failed.saturating_add(1);
                            }
                            Some("not_configured") | None => {
                                voice_skipped = voice_skipped.saturating_add(1);
                            }
                            Some(_) => {
                                voice_completed = voice_completed.saturating_add(1);
                            }
                        }
                    }
                } else if failures_after_signal_by_track
                    .get(&track.id)
                    .is_some_and(|failure| failure.source_signature == signature)
                {
                    audio_failed = audio_failed.saturating_add(1);
                    if voice_enabled {
                        voice_skipped = voice_skipped.saturating_add(1);
                    }
                } else if voice_enabled {
                    voice_skipped = voice_skipped.saturating_add(1);
                }
            }
            drop(states_after_signal_by_track);
            drop(failures_after_signal_by_track);
            drop(states_after_signal);
            drop(failures_after_signal);
            let audio_progress = ContextPassProgress {
                status: if audio_failed == 0 {
                    "complete"
                } else {
                    "complete_with_failures"
                },
                completed: audio_completed,
                failed: audio_failed,
                skipped: 0,
                total: tracks.len(),
            };

            let active_voice_workers = usize::from(voice_enabled && !voice_work.is_empty());
            let voice_total = progress_current.saturating_add(
                u64::try_from(voice_work.len())
                    .map_err(|_| JobHandlerError::new("context analysis is too large"))?,
            );
            let mut voice_progress = if voice_enabled {
                ContextPassProgress {
                    status: if voice_work.is_empty() {
                        "complete"
                    } else {
                        "running"
                    },
                    completed: voice_completed,
                    failed: voice_failed,
                    skipped: voice_skipped,
                    total: tracks.len(),
                }
            } else {
                ContextPassProgress::not_available(tracks.len())
            };
            context
                .checkpoint(context_checkpoint(audio_progress, voice_progress))
                .await
                .map_err(JobHandlerError::from_execution)?;
            context
                .update_progress(
                    JobProgress::new(
                        progress_current,
                        Some(voice_total),
                        if voice_enabled {
                            "Detecting voice"
                        } else {
                            "Analyzing audio context"
                        },
                        if voice_enabled {
                            format!(
                                "Voice detection: {} eligible tracks remain",
                                voice_work.len()
                            )
                        } else {
                            "Optional voice detection is not enabled".to_owned()
                        },
                    )
                    .map_err(|_| JobHandlerError::new("invalid context analysis progress"))?,
                )
                .await
                .map_err(JobHandlerError::from_execution)?;

            let voice_started = Instant::now();
            let mut voice_performance = VoicePerformanceAggregate::default();
            for (track_id, track_path, state) in voice_work {
                context
                    .check_cancelled()
                    .await
                    .map_err(JobHandlerError::from_execution)?;
                let attempt_started = Instant::now();
                let analysis = match self.root.resolve_existing(&track_path) {
                    Ok(path) => self.analyze_voice_track(context, path).await?,
                    Err(_) => Err(VoiceAnalysisError::MissingFile),
                };
                let document = match analysis {
                    Ok(document) => document,
                    Err(VoiceAnalysisError::Cancelled) => {
                        context
                            .check_cancelled()
                            .await
                            .map_err(JobHandlerError::from_execution)?;
                        return Err(JobHandlerError::new("voice analysis stopped unexpectedly"));
                    }
                    Err(error) => VoiceAnalysisDocument::unavailable(
                        &error,
                        attempt_started.elapsed().as_secs_f64(),
                    ),
                };
                voice_performance.observe(document.elapsed_seconds);
                let classified =
                    document.summary.get("status").and_then(Value::as_str) == Some("classified");
                let write = context_with_voice(track_id, &state, document)?;
                let stored = self
                    .repository
                    .store_context(
                        LOCAL_CONTEXT_ANALYZER_ID,
                        LOCAL_CONTEXT_IMPLEMENTATION_ID,
                        voice_signature,
                        context.job_id(),
                        &write,
                    )
                    .await
                    .map_err(|_| JobHandlerError::new("context analysis storage failed"))?;
                if stored {
                    if classified {
                        voice_completed = voice_completed.saturating_add(1);
                    } else {
                        voice_failed = voice_failed.saturating_add(1);
                    }
                } else {
                    voice_skipped = voice_skipped.saturating_add(1);
                }
                progress_current = progress_current.saturating_add(1);
                voice_progress = ContextPassProgress {
                    status: "running",
                    completed: voice_completed,
                    failed: voice_failed,
                    skipped: voice_skipped,
                    total: tracks.len(),
                };
                context
                    .checkpoint(context_checkpoint(audio_progress, voice_progress))
                    .await
                    .map_err(JobHandlerError::from_execution)?;
                context
                    .update_progress(
                        JobProgress::new(
                            progress_current.min(voice_total),
                            Some(voice_total),
                            "Detecting voice",
                            format!(
                                "Voice detection: {} of {} eligible tracks processed",
                                voice_completed.saturating_add(voice_failed),
                                tracks.len().saturating_sub(voice_skipped)
                            ),
                        )
                        .map_err(|_| JobHandlerError::new("invalid context analysis progress"))?,
                    )
                    .await
                    .map_err(JobHandlerError::from_execution)?;
            }
            let voice_wall_seconds = voice_started.elapsed().as_secs_f64();
            if voice_enabled {
                voice_progress = ContextPassProgress {
                    status: if voice_failed == 0 && voice_skipped == 0 {
                        "complete"
                    } else {
                        "complete_with_failures"
                    },
                    completed: voice_completed,
                    failed: voice_failed,
                    skipped: voice_skipped,
                    total: tracks.len(),
                };
            }
            context
                .checkpoint(context_checkpoint(audio_progress, voice_progress))
                .await
                .map_err(JobHandlerError::from_execution)?;

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
                active_voice_workers,
                &performance,
                signal_wall_seconds,
                &voice_performance,
                voice_wall_seconds,
                audio_progress,
                voice_progress,
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

#[derive(Debug, Clone, Copy)]
struct ContextPassProgress {
    status: &'static str,
    completed: usize,
    failed: usize,
    skipped: usize,
    total: usize,
}

impl ContextPassProgress {
    const fn not_available(total: usize) -> Self {
        Self {
            status: "not_available",
            completed: 0,
            failed: 0,
            skipped: total,
            total,
        }
    }

    fn as_value(self) -> Value {
        json!({
            "status": self.status,
            "completed_tracks": self.completed,
            "failed_tracks": self.failed,
            "skipped_tracks": self.skipped,
            "total_tracks": self.total,
        })
    }
}

fn context_checkpoint(
    audio_context: ContextPassProgress,
    voice_detection: ContextPassProgress,
) -> Map<String, Value> {
    object(json!({
        "schema_version": "assistant-library-context-job-progress/v1",
        "passes": {
            "audio_context": audio_context.as_value(),
            "voice_detection": voice_detection.as_value(),
        },
    }))
}

fn context_voice_stage_status(context: &CurrentTrackContext) -> Option<&str> {
    context
        .stages
        .get("voice")
        .and_then(Value::as_object)
        .and_then(|stage| stage.get("status"))
        .and_then(Value::as_str)
}

fn context_with_voice(
    track_id: music_domain::TrackId,
    state: &ContextState,
    voice: VoiceAnalysisDocument,
) -> Result<ContextWrite, JobHandlerError> {
    let mut context = parse_context_state(state).ok_or_else(|| {
        JobHandlerError::new("voice analysis has no current audio-context checkpoint")
    })?;
    let classified = voice.summary.get("status").and_then(Value::as_str) == Some("classified");
    context
        .summary
        .insert("voice".to_owned(), Value::Object(voice.summary));
    let reliability = context
        .summary
        .entry("measurement_reliability".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    let reliability = reliability
        .as_object_mut()
        .ok_or_else(|| JobHandlerError::new("stored context reliability is invalid"))?;
    reliability.insert(
        "voice".to_owned(),
        Value::String(if classified { "medium" } else { "unavailable" }.to_owned()),
    );
    context
        .stages
        .insert("voice".to_owned(), Value::Object(voice.stage));
    Ok(ContextWrite {
        track_id,
        source_signature: state.source_signature.clone(),
        completeness: "full".to_owned(),
        confidence: context.confidence,
        summary: context.summary,
        timeline: context.timeline,
        sections: context.sections,
        technical: context.technical,
        stages: context.stages,
    })
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
    voice_worker_count: usize,
    performance: &ContextPerformanceAggregate,
    signal_wall_seconds: f64,
    voice_performance: &VoicePerformanceAggregate,
    voice_wall_seconds: f64,
    audio_progress: ContextPassProgress,
    voice_progress: ContextPassProgress,
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
    let scope = serde_json::to_value(scope)
        .map_err(|_| JobHandlerError::new("context analysis scope could not be encoded"))?;
    Ok(json!({
        "schema_version": "assistant-library-context-job-result/v3",
        "analyzer": LOCAL_CONTEXT_ANALYZER_ID,
        "scope": scope,
        "tracks": tracks.len(),
        "analysis_workers": worker_count,
        "voice_workers": voice_worker_count,
        "passes": {
            "audio_context": audio_progress.as_value(),
            "voice_detection": voice_progress.as_value(),
        },
        "updated": updated,
        "failed": failed,
        "unchanged": tracks.len().saturating_sub(updated).saturating_sub(failed),
        "current_contexts": current_contexts.len(),
        "current_failures": current_failures.len(),
        "failure_samples": failure_samples,
        "performance": performance.as_value(signal_wall_seconds),
        "voice_performance": {
            "schema_version": "library-context-voice-performance/v1",
            "tracks_profiled": voice_performance.tracks_profiled,
            "wall_seconds": round_seconds(voice_wall_seconds),
            "worker_seconds": round_seconds(voice_performance.worker_seconds),
        },
        "wall_seconds": round_seconds(total_wall_seconds),
    }))
}

#[derive(Debug, Default)]
struct ContextPerformanceAggregate {
    tracks_profiled: usize,
    audio_seconds: f64,
    worker_seconds: f64,
    stage_seconds: BTreeMap<String, f64>,
}

impl ContextPerformanceAggregate {
    fn observe(&mut self, sample: &AudioContextPerformance) {
        self.tracks_profiled = self.tracks_profiled.saturating_add(1);
        self.audio_seconds += sample.audio_seconds;
        self.worker_seconds += sample.elapsed_seconds;
        for (stage, seconds) in &sample.stage_seconds {
            if let Some(seconds) = seconds.as_f64() {
                *self.stage_seconds.entry(stage.clone()).or_default() += seconds;
            }
        }
    }

    fn as_value(&self, wall_seconds: f64) -> Value {
        let measured = self.stage_seconds.values().sum::<f64>();
        let dominant = self
            .stage_seconds
            .iter()
            .max_by(|left, right| left.1.total_cmp(right.1))
            .map(|(stage, _)| stage.clone());
        let rounded = self
            .stage_seconds
            .iter()
            .map(|(stage, seconds)| (stage.clone(), round_seconds(*seconds)))
            .collect::<BTreeMap<_, _>>();
        let shares = if measured > 0.0 {
            self.stage_seconds
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
            "tracks_profiled": self.tracks_profiled,
            "wall_seconds": round_seconds(wall_seconds),
            "worker_seconds": round_seconds(self.worker_seconds),
            "audio_seconds": round_seconds(self.audio_seconds),
            "audio_realtime_factor": if wall_seconds > 0.0 {
                Some(round_seconds(self.audio_seconds / wall_seconds))
            } else {
                None
            },
            "dominant_stage": dominant,
            "stage_seconds": rounded,
            "stage_share_percent": shares,
        })
    }
}

#[derive(Debug, Default)]
struct VoicePerformanceAggregate {
    tracks_profiled: usize,
    worker_seconds: f64,
}

impl VoicePerformanceAggregate {
    fn observe(&mut self, elapsed_seconds: f64) {
        self.tracks_profiled = self.tracks_profiled.saturating_add(1);
        self.worker_seconds += elapsed_seconds;
    }
}

fn object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

fn round_seconds(value: f64) -> f64 {
    (value * 1_000.0).round() / 1_000.0
}

#[cfg(test)]
mod tests {
    use music_analysis::{AudioContextPerformance, VoiceAnalysisDocument, VoiceAnalysisError};
    use music_application::assistant::ContextState;
    use music_domain::TrackId;
    use serde_json::{Value, json};

    use super::{
        ContextPassProgress, ContextPerformanceAggregate, VoicePerformanceAggregate,
        context_checkpoint, context_with_voice,
    };

    #[test]
    fn performance_aggregation_preserves_the_result_contract_without_per_track_storage() {
        let mut performance = ContextPerformanceAggregate::default();
        performance.observe(&AudioContextPerformance {
            audio_seconds: 2.0,
            elapsed_seconds: 1.0,
            stage_seconds: serde_json::Map::from_iter([
                ("decode".to_owned(), json!(0.5)),
                ("fft".to_owned(), json!(0.5)),
            ]),
        });
        performance.observe(&AudioContextPerformance {
            audio_seconds: 3.0,
            elapsed_seconds: 2.0,
            stage_seconds: serde_json::Map::from_iter([("decode".to_owned(), json!(0.25))]),
        });
        assert_eq!(
            performance.as_value(4.0),
            json!({
                "schema_version": "library-context-performance/v1",
                "tracks_profiled": 2,
                "wall_seconds": 4.0,
                "worker_seconds": 3.0,
                "audio_seconds": 5.0,
                "audio_realtime_factor": 1.25,
                "dominant_stage": "decode",
                "stage_seconds": {"decode": 0.75, "fft": 0.5},
                "stage_share_percent": {"decode": 60.0, "fft": 40.0},
            })
        );

        let mut voice = VoicePerformanceAggregate::default();
        voice.observe(1.25);
        voice.observe(2.75);
        assert_eq!(voice.tracks_profiled, 2);
        assert_eq!(voice.worker_seconds, 4.0);
    }

    fn partial_context() -> Result<ContextState, Box<dyn std::error::Error>> {
        Ok(ContextState {
            track_id: TrackId::new(7)?,
            source_signature: "current-signature".to_owned(),
            job_id: "signal-job".to_owned(),
            completeness: "partial".to_owned(),
            confidence: "high".to_owned(),
            summary_json: json!({
                "schema_version": "local-context/v2",
                "voice": {"status": "not_classified"},
                "measurement_reliability": {"voice": "pending"},
            })
            .to_string(),
            timeline_json: json!([{"start_s": 0.0}]).to_string(),
            sections_json: json!([{"id": "section-1"}]).to_string(),
            technical_json: json!({"duration_s": 60.0}).to_string(),
            stages_json: json!({"voice": {"status": "pending"}}).to_string(),
            updated_at_unix_seconds: 1,
        })
    }

    #[test]
    fn voice_result_promotes_the_partial_checkpoint_without_rebuilding_signal_context()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = partial_context()?;
        let voice = VoiceAnalysisDocument {
            summary: json!({
                "status": "classified",
                "voice_probability": 0.8,
                "vocal_coverage": 0.75,
                "note": "bounded classifier evidence",
            })
            .as_object()
            .cloned()
            .ok_or("voice summary is not an object")?,
            stage: json!({
                "status": "complete",
                "required": false,
                "analyzer_id": "essentia-musicnn-voice/v1",
            })
            .as_object()
            .cloned()
            .ok_or("voice stage is not an object")?,
            elapsed_seconds: 1.5,
            prediction_windows: 4,
        };
        let write = context_with_voice(state.track_id, &state, voice)?;
        assert_eq!(write.completeness, "full");
        assert_eq!(write.source_signature, "current-signature");
        assert_eq!(write.summary["voice"]["status"], "classified");
        assert_eq!(write.summary["measurement_reliability"]["voice"], "medium");
        assert_eq!(write.stages["voice"]["status"], "complete");
        assert_eq!(
            write.timeline,
            vec![
                json!({"start_s": 0.0})
                    .as_object()
                    .cloned()
                    .ok_or("timeline item is not an object")?
            ]
        );
        assert_eq!(write.technical["duration_s"], 60.0);
        Ok(())
    }

    #[test]
    fn optional_voice_failure_still_promotes_the_remaining_context_to_full()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = partial_context()?;
        let voice = VoiceAnalysisDocument::unavailable(&VoiceAnalysisError::Inference, 0.25);
        let write = context_with_voice(state.track_id, &state, voice)?;
        assert_eq!(write.completeness, "full");
        assert_eq!(write.summary["voice"]["status"], "unavailable");
        assert_eq!(
            write.summary["measurement_reliability"]["voice"],
            "unavailable"
        );
        assert_eq!(write.stages["voice"]["reason"], "inference_failed");
        Ok(())
    }

    #[test]
    fn progress_checkpoint_keeps_both_passes_independently_auditable() {
        let checkpoint = context_checkpoint(
            ContextPassProgress {
                status: "complete",
                completed: 3,
                failed: 1,
                skipped: 0,
                total: 4,
            },
            ContextPassProgress::not_available(4),
        );
        assert_eq!(
            checkpoint["passes"]["audio_context"]["failed_tracks"],
            Value::from(1)
        );
        assert_eq!(
            checkpoint["passes"]["voice_detection"]["status"],
            "not_available"
        );
        assert_eq!(
            checkpoint["passes"]["voice_detection"]["skipped_tracks"],
            Value::from(4)
        );
    }
}
