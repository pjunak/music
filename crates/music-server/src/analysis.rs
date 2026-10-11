use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream};
use music_analysis::{
    AnalysisExecutor, AudioContextAnalyzer, AudioContextDocument, AudioContextError,
    AudioContextPerformance, VoiceAnalysisDocument, VoiceAnalysisError, VoiceContextPreparation,
    VoiceWorker, VoiceWorkerFactory,
};
use music_application::assistant::{
    AnalysisFailureState, AnalysisFailureWrite, AssistantDependencyError, ContextScope,
    ContextState, ContextWrite, CurrentTrackContext, LIBRARY_CONTEXT_JOB_KIND,
    LOCAL_CONTEXT_ANALYZER_ID, LOCAL_CONTEXT_IMPLEMENTATION_ID, LocalAnalysisRepository,
    VoiceAnalyzerStatus, context_source_signature, parse_context_state,
};
use music_application::jobs::{
    JobCheckpointPolicy, JobDefinition, JobExecutionContext, JobHandler, JobHandlerError,
    JobHandlerFuture, JobLane, JobProgress, parse_parameters,
};
use music_media::LibraryRoot;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

const FAILURE_SAMPLE_LIMIT: usize = 20;
const MAX_RETRY_HISTORY: usize = 64;
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(100);

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
    voice_worker_factory: Option<VoiceWorkerFactory>,
}

impl ContextAnalysisJobHandler {
    pub(crate) fn new(
        repository: Arc<dyn LocalAnalysisRepository>,
        root: LibraryRoot,
        executor: AnalysisExecutor,
        analyzer: Arc<dyn AudioContextAnalyzer>,
        voice_analyzer: VoiceAnalyzerStatus,
        voice_worker_factory: Option<VoiceWorkerFactory>,
    ) -> Self {
        Self {
            repository,
            root,
            executor,
            analyzer,
            voice_analyzer,
            voice_worker_factory,
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
        let voice = if self.voice_analyzer.is_ready() && self.voice_worker_factory.is_some() {
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
                        .map_err(|error| JobHandlerError::new(format!("context {error}")));
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
        worker: &VoiceWorker,
        path: std::path::PathBuf,
    ) -> Result<Result<VoiceAnalysisDocument, VoiceAnalysisError>, JobHandlerError> {
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

    async fn retry_job_ids(
        &self,
        context: &JobExecutionContext,
    ) -> Result<BTreeSet<String>, JobHandlerError> {
        let current = context
            .related_job(context.job_id())
            .await
            .map_err(JobHandlerError::from_execution)?
            .ok_or_else(|| JobHandlerError::new("context analysis job history is unavailable"))?;
        let mut completed = BTreeSet::new();
        let mut next = current.retry_of_id.clone();
        while let Some(id) = next {
            // Missing, incompatible, cyclic or excessive history cannot justify reuse.
            // Older results outside this bounded chain must be analyzed again.
            if completed.len() >= MAX_RETRY_HISTORY || id == current.id || completed.contains(&id) {
                break;
            }
            let Some(previous) = context
                .related_job(&id)
                .await
                .map_err(JobHandlerError::from_execution)?
            else {
                break;
            };
            if previous.kind != current.kind
                || previous.schema_version != current.schema_version
                || previous.lane != current.lane
                || previous.restartable != current.restartable
                || previous.checkpoint_policy != current.checkpoint_policy
                || previous.parameters != current.parameters
                || !previous.status.can_retry()
            {
                break;
            }
            next = previous.retry_of_id;
            completed.insert(previous.id);
        }
        Ok(completed)
    }

    async fn start_voice_worker(&self) -> Option<VoiceWorker> {
        let factory = self.voice_worker_factory.as_ref()?.clone();
        self.executor
            .execute(move || factory.start())
            .await
            .ok()
            .and_then(Result::ok)
    }

    async fn stop_voice_worker(&self, worker: VoiceWorker) {
        let _ = self.executor.execute(move || drop(worker)).await;
    }
}

impl JobHandler for ContextAnalysisJobHandler {
    fn definition(&self) -> JobDefinition {
        JobDefinition {
            kind: LIBRARY_CONTEXT_JOB_KIND,
            schema_version: 2,
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
            let parameters: ContextAnalysisParameters =
                parse_parameters(parameters, "invalid context analysis parameters")?;
            let scope = parameters
                .scope
                .to_scope()
                .map_err(|()| JobHandlerError::new("invalid context analysis scope"))?;
            let run = ContextRun {
                handler: self,
                context,
                voice_signature: self.voice_analyzer.source_signature.as_deref(),
                voice_enabled: self.voice_analyzer.is_ready()
                    && self.voice_worker_factory.is_some(),
            };
            let all_tracks = self.repository.all_tracks().await.map_err(storage_failed)?;
            let tracks = scope
                .select(&all_tracks)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            drop(all_tracks);
            let retry_jobs = if parameters.force {
                self.retry_job_ids(context).await?
            } else {
                BTreeSet::new()
            };

            let plan = run
                .plan_signal(&tracks, parameters.force, &retry_jobs)
                .await?;
            let signal = run.signal_pass(tracks.len(), plan).await?;
            let voice_plan = run.plan_voice(&tracks).await?;
            let voice = run
                .voice_pass(tracks.len(), voice_plan, signal.progress_current)
                .await?;

            let final_states = self
                .repository
                .context_states(LOCAL_CONTEXT_ANALYZER_ID)
                .await
                .map_err(storage_failed)?;
            let final_failures = self
                .repository
                .analysis_failures(LOCAL_CONTEXT_ANALYZER_ID)
                .await
                .map_err(storage_failed)?;
            context_result(
                &tracks,
                &final_states,
                &final_failures,
                context.job_id(),
                &parameters.scope,
                &self.voice_analyzer,
                &ContextRunReport {
                    signal,
                    voice,
                    total_wall_seconds: started.elapsed().as_secs_f64(),
                },
            )
        })
    }
}

/// One execution of the library context job: a parallel signal pass over the
/// tracks that need it, then a sequential voice pass over the eligible ones.
/// Every stored track is a durable checkpoint, so recovery resumes instead of
/// decoding audio again.
struct ContextRun<'a> {
    handler: &'a ContextAnalysisJobHandler,
    context: &'a JobExecutionContext,
    voice_signature: Option<&'a str>,
    voice_enabled: bool,
}

/// Tracks whose signal analysis must run, and what earlier attempts settled.
struct SignalPlan {
    work: Vec<(music_domain::TrackId, music_domain::LibraryPath, String)>,
    completed: usize,
    failed: usize,
    pending_voice: usize,
    checkpointed: u64,
}

struct SignalOutcome {
    progress_current: u64,
    performance: ContextPerformanceAggregate,
    wall_seconds: f64,
    workers: usize,
}

/// The audio pass as stored after signal analysis, and the voice work left.
struct VoicePlan {
    audio: ContextPassProgress,
    work: Vec<(
        music_domain::TrackId,
        music_domain::LibraryPath,
        ContextState,
    )>,
    completed: usize,
    failed: usize,
    skipped: usize,
}

struct VoiceOutcome {
    audio: ContextPassProgress,
    progress: ContextPassProgress,
    performance: VoicePerformanceAggregate,
    wall_seconds: f64,
    workers: usize,
}

struct ContextRunReport {
    signal: SignalOutcome,
    voice: VoiceOutcome,
    total_wall_seconds: f64,
}

impl ContextRun<'_> {
    fn job_id(&self) -> &str {
        self.context.job_id()
    }

    fn signature(&self, track: &music_domain::IndexedTrack) -> Result<String, JobHandlerError> {
        context_source_signature(track, LOCAL_CONTEXT_IMPLEMENTATION_ID, self.voice_signature)
            .map_err(|error| {
                JobHandlerError::with_reason("track context fingerprint failed", &error)
            })
    }

    async fn stored_states(
        &self,
    ) -> Result<(Vec<ContextState>, Vec<AnalysisFailureState>), JobHandlerError> {
        let repository = &self.handler.repository;
        let states = repository
            .context_states(LOCAL_CONTEXT_ANALYZER_ID)
            .await
            .map_err(storage_failed)?;
        let failures = repository
            .analysis_failures(LOCAL_CONTEXT_ANALYZER_ID)
            .await
            .map_err(storage_failed)?;
        Ok((states, failures))
    }

    async fn store(&self, write: &ContextWrite) -> Result<bool, JobHandlerError> {
        self.handler
            .repository
            .store_context(
                LOCAL_CONTEXT_ANALYZER_ID,
                LOCAL_CONTEXT_IMPLEMENTATION_ID,
                self.voice_signature,
                self.job_id(),
                write,
            )
            .await
            .map_err(storage_failed)
    }

    async fn checkpoint(
        &self,
        audio: ContextPassProgress,
        voice: ContextPassProgress,
    ) -> Result<(), JobHandlerError> {
        self.context
            .checkpoint(context_checkpoint(audio, voice))
            .await
            .map_err(JobHandlerError::from_execution)
    }

    async fn report(
        &self,
        current: u64,
        total: u64,
        phase: &str,
        message: String,
    ) -> Result<(), JobHandlerError> {
        let progress = JobProgress::new(current, Some(total), phase, message).map_err(|error| {
            JobHandlerError::caused_by("invalid context analysis progress", &error)
        })?;
        self.context
            .update_progress(progress)
            .await
            .map_err(JobHandlerError::from_execution)
    }

    /// Decide which tracks need signal analysis. A forced run repeats current
    /// results, except those this job or its retried predecessors produced.
    async fn plan_signal(
        &self,
        tracks: &[music_domain::IndexedTrack],
        force: bool,
        retry_jobs: &BTreeSet<String>,
    ) -> Result<SignalPlan, JobHandlerError> {
        let (states, failures) = self.stored_states().await?;
        let state_by_track = context_by_track(&states);
        let failure_by_track = failures_by_track(&failures);
        let job_id = self.job_id();
        let mut plan = SignalPlan {
            work: Vec::new(),
            completed: 0,
            failed: 0,
            pending_voice: 0,
            checkpointed: 0,
        };
        for track in tracks {
            let signature = self.signature(track)?;
            let state = state_by_track.get(&track.id).copied();
            let failure = failure_by_track.get(&track.id).copied();
            let parsed = state
                .filter(|state| state.source_signature == signature)
                .and_then(parse_context_state);
            let current_failure =
                failure.is_some_and(|failure| failure.source_signature == signature);
            let completed_by_job = state.is_some_and(|state| {
                state.job_id == job_id
                    && state.source_signature == signature
                    && parse_context_state(state).is_some()
            }) || failure.is_some_and(|failure| {
                failure.job_id == job_id && failure.source_signature == signature
            });
            if completed_by_job {
                plan.checkpointed = plan.checkpointed.saturating_add(1);
            }
            let completed_by_retry = state.is_some_and(|state| retry_jobs.contains(&state.job_id));
            // A partial row is the durable boundary between the signal and
            // voice passes. Preserve it even for a forced retry so a
            // cancelled voice pass never causes another signal decode.
            let signal_is_current = parsed.as_ref().is_some_and(|parsed| {
                parsed.completeness == "partial" || !force || completed_by_job || completed_by_retry
            });
            if signal_is_current && !current_failure {
                plan.completed = plan.completed.saturating_add(1);
                if self.voice_enabled
                    && state
                        .zip(parsed.as_ref())
                        .is_some_and(|(state, parsed)| voice_needs_attempt(parsed, state, job_id))
                {
                    plan.pending_voice = plan.pending_voice.saturating_add(1);
                }
                continue;
            }
            if current_failure && failure.is_some_and(|failure| failure.job_id == job_id) {
                plan.failed = plan.failed.saturating_add(1);
                continue;
            }
            plan.work.push((track.id, track.path.clone(), signature));
        }
        Ok(plan)
    }

    /// Analyze the planned tracks on the bounded CPU executor, storing each
    /// result or failure as soon as it arrives.
    async fn signal_pass(
        &self,
        track_count: usize,
        plan: SignalPlan,
    ) -> Result<SignalOutcome, JobHandlerError> {
        let SignalPlan {
            work,
            mut completed,
            mut failed,
            pending_voice,
            checkpointed,
        } = plan;
        let starting = self.context.progress_current().max(checkpointed);
        let work_count = u64::try_from(work.len())
            .map_err(|error| JobHandlerError::caused_by("context analysis is too large", &error))?;
        let expected_voice = if self.voice_enabled {
            u64::try_from(work.len().saturating_add(pending_voice)).map_err(|error| {
                JobHandlerError::caused_by("context analysis is too large", &error)
            })?
        } else {
            0
        };
        let total = starting
            .saturating_add(work_count)
            .saturating_add(expected_voice);
        let workers = self.handler.executor.worker_count().min(work.len());
        let initial_voice = if self.voice_enabled {
            ContextPassProgress::new(PassStatus::Waiting, 0, 0, 0, track_count)
        } else {
            ContextPassProgress::not_available(track_count)
        };
        let status = if work.is_empty() {
            PassStatus::Complete
        } else {
            PassStatus::Running
        };
        self.checkpoint(
            ContextPassProgress::new(status, completed, failed, 0, track_count),
            initial_voice,
        )
        .await?;
        self.report(
            starting,
            total,
            "Analyzing audio context",
            format!("{} tracks need context analysis", work.len()),
        )
        .await?;

        let started = Instant::now();
        let mut progress_current = starting;
        let mut performance = ContextPerformanceAggregate::default();
        let handler = self.handler;
        let context = self.context;
        let mut results = stream::iter(work.into_iter().map(
            |(track_id, track_path, signature)| async move {
                let analysis = match handler.root.resolve_existing(&track_path) {
                    Ok(path) => handler.analyze_track(context, path).await,
                    Err(_) => Ok(Err(AudioContextError::MissingFile)),
                };
                (track_id, signature, analysis)
            },
        ))
        .buffer_unordered(workers.max(1));
        while let Some((track_id, signature, analysis)) = results.next().await {
            match analysis? {
                Ok(document) => {
                    performance.observe(&document.performance);
                    let write = ContextWrite {
                        track_id,
                        source_signature: signature,
                        completeness: document.completeness.to_owned(),
                        summary: document.summary,
                        timeline: document.timeline,
                        sections: document.sections,
                        technical: document.technical,
                        stages: document.stages,
                    };
                    if self.store(&write).await? {
                        completed = completed.saturating_add(1);
                    }
                }
                Err(AudioContextError::Cancelled) => {
                    self.context
                        .check_cancelled()
                        .await
                        .map_err(JobHandlerError::from_execution)?;
                    return Err(JobHandlerError::new(
                        "context analysis stopped unexpectedly",
                    ));
                }
                Err(error) => {
                    let failure = AnalysisFailureWrite {
                        track_id,
                        source_signature: signature,
                        error: format!("AudioContextError: {error}"),
                    };
                    let stored = self
                        .handler
                        .repository
                        .store_context_failure(
                            LOCAL_CONTEXT_ANALYZER_ID,
                            LOCAL_CONTEXT_IMPLEMENTATION_ID,
                            self.voice_signature,
                            self.job_id(),
                            &failure,
                        )
                        .await
                        .map_err(storage_failed)?;
                    if stored {
                        failed = failed.saturating_add(1);
                    }
                }
            }
            progress_current = progress_current.saturating_add(1);
            self.checkpoint(
                ContextPassProgress::new(PassStatus::Running, completed, failed, 0, track_count),
                initial_voice,
            )
            .await?;
            self.report(
                progress_current.min(total),
                total,
                "Analyzing audio context",
                format!(
                    "Audio context: {} of {} tracks processed",
                    completed.saturating_add(failed),
                    track_count
                ),
            )
            .await?;
        }
        Ok(SignalOutcome {
            progress_current,
            performance,
            wall_seconds: started.elapsed().as_secs_f64(),
            workers,
        })
    }

    /// Recount both passes from storage after the signal pass and collect the
    /// tracks whose voice stage still needs an attempt.
    async fn plan_voice(
        &self,
        tracks: &[music_domain::IndexedTrack],
    ) -> Result<VoicePlan, JobHandlerError> {
        let (states, failures) = self.stored_states().await?;
        let state_by_track = context_by_track(&states);
        let failure_by_track = failures_by_track(&failures);
        let mut audio_completed = 0_usize;
        let mut audio_failed = 0_usize;
        let mut plan = VoicePlan {
            audio: ContextPassProgress::not_available(tracks.len()),
            work: Vec::new(),
            completed: 0,
            failed: 0,
            skipped: 0,
        };
        for track in tracks {
            let signature = self.signature(track)?;
            let current_state = state_by_track
                .get(&track.id)
                .copied()
                .filter(|state| state.source_signature == signature);
            let parsed = current_state.and_then(parse_context_state);
            if let (Some(state), Some(parsed)) = (current_state, parsed) {
                audio_completed = audio_completed.saturating_add(1);
                if self.voice_enabled {
                    if voice_needs_attempt(&parsed, state, self.job_id()) {
                        plan.work
                            .push((track.id, track.path.clone(), state.clone()));
                        continue;
                    }
                    match context_voice_stage_status(&parsed) {
                        Some("unavailable") => plan.failed = plan.failed.saturating_add(1),
                        Some("not_configured") | None => {
                            plan.skipped = plan.skipped.saturating_add(1);
                        }
                        Some(_) => plan.completed = plan.completed.saturating_add(1),
                    }
                }
            } else if failure_by_track
                .get(&track.id)
                .is_some_and(|failure| failure.source_signature == signature)
            {
                audio_failed = audio_failed.saturating_add(1);
                if self.voice_enabled {
                    plan.skipped = plan.skipped.saturating_add(1);
                }
            } else if self.voice_enabled {
                plan.skipped = plan.skipped.saturating_add(1);
            }
        }
        let status = if audio_failed == 0 {
            PassStatus::Complete
        } else {
            PassStatus::CompleteWithFailures
        };
        plan.audio =
            ContextPassProgress::new(status, audio_completed, audio_failed, 0, tracks.len());
        Ok(plan)
    }

    /// Run voice inference one track at a time on the single model worker.
    async fn voice_pass(
        &self,
        track_count: usize,
        plan: VoicePlan,
        mut progress_current: u64,
    ) -> Result<VoiceOutcome, JobHandlerError> {
        let VoicePlan {
            audio,
            work,
            mut completed,
            mut failed,
            mut skipped,
        } = plan;
        let worker = if self.voice_enabled && !work.is_empty() {
            self.handler.start_voice_worker().await
        } else {
            None
        };
        let workers = usize::from(worker.is_some());
        let total =
            progress_current.saturating_add(u64::try_from(work.len()).map_err(|error| {
                JobHandlerError::caused_by("context analysis is too large", &error)
            })?);
        let mut progress = if self.voice_enabled {
            let status = if work.is_empty() {
                PassStatus::Complete
            } else {
                PassStatus::Running
            };
            ContextPassProgress::new(status, completed, failed, skipped, track_count)
        } else {
            ContextPassProgress::not_available(track_count)
        };
        self.checkpoint(audio, progress).await?;
        let (phase, message) = if self.voice_enabled {
            (
                "Detecting voice",
                format!("Voice detection: {} eligible tracks remain", work.len()),
            )
        } else {
            (
                "Analyzing audio context",
                "Optional voice detection is not enabled".to_owned(),
            )
        };
        self.report(progress_current, total, phase, message).await?;

        let started = Instant::now();
        let mut performance = VoicePerformanceAggregate::default();
        for (track_id, track_path, state) in work {
            self.context
                .check_cancelled()
                .await
                .map_err(JobHandlerError::from_execution)?;
            let attempt_started = Instant::now();
            let analysis = match self.handler.root.resolve_existing(&track_path) {
                Ok(path) => match worker.as_ref() {
                    Some(worker) => {
                        self.handler
                            .analyze_voice_track(self.context, worker, path)
                            .await?
                    }
                    None => Err(VoiceAnalysisError::WorkerUnavailable),
                },
                Err(_) => Err(VoiceAnalysisError::MissingFile),
            };
            let document = match analysis {
                Ok(document) => document,
                Err(VoiceAnalysisError::Cancelled) => {
                    self.context
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
            performance.observe(document.elapsed_seconds);
            let classified =
                document.summary.get("status").and_then(Value::as_str) == Some("classified");
            if self
                .store(&context_with_voice(track_id, &state, document)?)
                .await?
            {
                if classified {
                    completed = completed.saturating_add(1);
                } else {
                    failed = failed.saturating_add(1);
                }
            } else {
                skipped = skipped.saturating_add(1);
            }
            progress_current = progress_current.saturating_add(1);
            progress = ContextPassProgress::new(
                PassStatus::Running,
                completed,
                failed,
                skipped,
                track_count,
            );
            self.checkpoint(audio, progress).await?;
            self.report(
                progress_current.min(total),
                total,
                "Detecting voice",
                format!(
                    "Voice detection: {} of {} eligible tracks processed",
                    completed.saturating_add(failed),
                    track_count.saturating_sub(skipped)
                ),
            )
            .await?;
        }
        if let Some(worker) = worker {
            self.handler.stop_voice_worker(worker).await;
        }
        let wall_seconds = started.elapsed().as_secs_f64();
        if self.voice_enabled {
            let status = if failed == 0 && skipped == 0 {
                PassStatus::Complete
            } else {
                PassStatus::CompleteWithFailures
            };
            progress = ContextPassProgress::new(status, completed, failed, skipped, track_count);
        }
        self.checkpoint(audio, progress).await?;
        Ok(VoiceOutcome {
            audio,
            progress,
            performance,
            wall_seconds,
            workers,
        })
    }
}

fn storage_failed(error: AssistantDependencyError) -> JobHandlerError {
    JobHandlerError::caused_by("context analysis storage failed", error.as_ref())
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

fn failures_by_track(
    states: &[AnalysisFailureState],
) -> BTreeMap<music_domain::TrackId, &AnalysisFailureState> {
    states.iter().map(|state| (state.track_id, state)).collect()
}

fn context_by_track(states: &[ContextState]) -> BTreeMap<music_domain::TrackId, &ContextState> {
    states.iter().map(|state| (state.track_id, state)).collect()
}

/// The state of one pass, as checkpointed and reported to the UI.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum PassStatus {
    Waiting,
    Running,
    Complete,
    CompleteWithFailures,
    NotAvailable,
}

impl PassStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Running => "running",
            Self::Complete => "complete",
            Self::CompleteWithFailures => "complete_with_failures",
            Self::NotAvailable => "not_available",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ContextPassProgress {
    status: PassStatus,
    completed: usize,
    failed: usize,
    skipped: usize,
    total: usize,
}

impl ContextPassProgress {
    const fn new(
        status: PassStatus,
        completed: usize,
        failed: usize,
        skipped: usize,
        total: usize,
    ) -> Self {
        Self {
            status,
            completed,
            failed,
            skipped,
            total,
        }
    }

    const fn not_available(total: usize) -> Self {
        Self::new(PassStatus::NotAvailable, 0, 0, total, total)
    }

    fn as_value(self) -> Value {
        json!({
            "status": self.status.as_str(),
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

fn voice_needs_attempt(context: &CurrentTrackContext, state: &ContextState, job_id: &str) -> bool {
    // Failures are complete factual checkpoints, not successful voice results.
    // A new requested job can retry them; recovery of the same job cannot loop.
    match context_voice_stage_status(context) {
        Some("pending") => true,
        Some("unavailable") => state.job_id != job_id,
        _ => false,
    }
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
        summary: context.summary,
        timeline: context.timeline,
        sections: context.sections,
        technical: context.technical,
        stages: context.stages,
    })
}

fn context_result(
    tracks: &[music_domain::IndexedTrack],
    states: &[ContextState],
    failures: &[AnalysisFailureState],
    job_id: &str,
    scope: &ContextScopeParameters,
    voice_analyzer: &VoiceAnalyzerStatus,
    report: &ContextRunReport,
) -> Result<Value, JobHandlerError> {
    let ContextRunReport {
        signal,
        voice,
        total_wall_seconds,
    } = report;
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
                .map_err(|error| {
                    JobHandlerError::with_reason("track context fingerprint failed", &error)
                })?;
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
    let scope = serde_json::to_value(scope).map_err(|error| {
        JobHandlerError::caused_by("context analysis scope could not be encoded", &error)
    })?;
    Ok(json!({
        "schema_version": "assistant-library-context-job-result/v3",
        "analyzer": LOCAL_CONTEXT_ANALYZER_ID,
        "scope": scope,
        "tracks": tracks.len(),
        "analysis_workers": signal.workers,
        "voice_workers": voice.workers,
        "passes": {
            "audio_context": voice.audio.as_value(),
            "voice_detection": voice.progress.as_value(),
        },
        "updated": updated,
        "failed": failed,
        "unchanged": tracks.len().saturating_sub(updated).saturating_sub(failed),
        "current_contexts": current_contexts.len(),
        "current_failures": current_failures.len(),
        "failure_samples": failure_samples,
        "performance": signal.performance.as_value(signal.wall_seconds),
        "voice_performance": {
            "schema_version": "library-context-voice-performance/v1",
            "tracks_profiled": voice.performance.tracks_profiled,
            "wall_seconds": round_seconds(voice.wall_seconds),
            "worker_seconds": round_seconds(voice.performance.worker_seconds),
        },
        "wall_seconds": round_seconds(*total_wall_seconds),
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
#[path = "analysis_recovery_tests.rs"]
mod recovery_tests;

#[cfg(test)]
mod tests {
    use music_analysis::{AudioContextPerformance, VoiceAnalysisDocument, VoiceAnalysisError};
    use music_application::assistant::ContextState;
    use music_domain::TrackId;
    use serde_json::{Value, json};

    use super::{
        ContextPassProgress, ContextPerformanceAggregate, PassStatus, VoicePerformanceAggregate,
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
            summary_json: json!({
                "schema_version": "local-context/v3",
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
                "voice_score": 0.8,
                "vocal_coverage": 0.75,
                "note": "bounded classifier evidence",
            })
            .as_object()
            .cloned()
            .ok_or("voice summary is not an object")?,
            stage: json!({
                "status": "complete",
                "required": false,
                "analyzer_id": "essentia-musicnn-voice/v2",
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
            ContextPassProgress::new(PassStatus::Complete, 3, 1, 0, 4),
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
