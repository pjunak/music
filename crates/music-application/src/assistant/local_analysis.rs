use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use music_domain::{IndexedTrack, LibraryPath, TrackId};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::planner::metadata_profile;
use super::tags::{
    AssistantFuture, Confidence, LOCAL_AUDIO_ANALYZER_ID, LOCAL_METADATA_ANALYZER_ID,
    audio_source_signature, metadata_source_signature,
};
use crate::jobs::{
    JobCheckpointPolicy, JobDefinition, JobExecutionContext, JobHandler, JobHandlerError,
    JobHandlerFuture, JobLane, JobProgress,
};
use crate::library::LibraryRepository;

pub const METADATA_ANALYSIS_JOB_KIND: &str = "assistant.library-analysis";
pub const AUDIO_ANALYSIS_JOB_KIND: &str = "assistant.library-audio-analysis";
pub const LIBRARY_CONTEXT_JOB_KIND: &str = "assistant.library-context-analysis";
pub const LOCAL_CONTEXT_ANALYZER_ID: &str = "local-context/v2";
pub const LOCAL_CONTEXT_IMPLEMENTATION_ID: &str = "local-context/v2+rustfft/v1";
const METADATA_ANALYSIS_BATCH_SIZE: usize = 50;
pub const VOICE_ANALYZER_ID: &str = "essentia-musicnn-voice/v1";
pub const VOICE_MODEL_FILENAME: &str = "voice_instrumental-musicnn-msd-2.pb";
pub const VOICE_MODEL_SHA256: &str =
    "b734bca3fc99257cf0088211b44bd36e8a26fbb1f9ce67e1e97d39f188094b0a";

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AnalysisState {
    pub track_id: TrackId,
    pub source_signature: String,
    pub job_id: String,
    pub confidence: String,
    pub updated_at_unix_seconds: i64,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AnalysisFailureState {
    pub track_id: TrackId,
    pub source_signature: String,
    pub job_id: String,
    pub error: String,
    pub updated_at_unix_seconds: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisWrite {
    pub track_id: TrackId,
    pub source_signature: String,
    pub energy: f64,
    pub brightness: f64,
    pub tension: f64,
    pub moods: Vec<String>,
    pub evidence: Vec<String>,
    pub metrics: Map<String, Value>,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelAnalysisWrite {
    pub profile: AnalysisWrite,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AnalysisFailureWrite {
    pub track_id: TrackId,
    pub source_signature: String,
    pub error: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ContextState {
    pub track_id: TrackId,
    pub source_signature: String,
    pub job_id: String,
    pub completeness: String,
    pub confidence: String,
    pub summary_json: String,
    pub timeline_json: String,
    pub sections_json: String,
    pub technical_json: String,
    pub stages_json: String,
    pub updated_at_unix_seconds: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextWrite {
    pub track_id: TrackId,
    pub source_signature: String,
    pub completeness: String,
    pub confidence: String,
    pub summary: Map<String, Value>,
    pub timeline: Vec<Map<String, Value>>,
    pub sections: Vec<Map<String, Value>>,
    pub technical: Map<String, Value>,
    pub stages: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CurrentTrackContext {
    pub analyzer_id: String,
    pub source_signature: String,
    pub completeness: String,
    pub confidence: String,
    pub summary: Map<String, Value>,
    pub timeline: Vec<Map<String, Value>>,
    pub sections: Vec<Map<String, Value>>,
    pub technical: Map<String, Value>,
    pub stages: Map<String, Value>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ContextScope {
    All,
    Folder {
        path: Option<LibraryPath>,
        recursive: bool,
    },
    Tracks(Vec<TrackId>),
}

impl ContextScope {
    #[must_use]
    pub fn contains(&self, track: &IndexedTrack) -> bool {
        match self {
            Self::All => true,
            Self::Tracks(track_ids) => track_ids.contains(&track.id),
            Self::Folder { path, recursive } => match path {
                None if *recursive => true,
                None => track.path.parent().is_none(),
                Some(path) if *recursive => {
                    let prefix = format!("{}/", path.as_str());
                    track.path.as_str().starts_with(&prefix)
                }
                Some(path) => track.path.parent().as_ref() == Some(path),
            },
        }
    }

    #[must_use]
    pub fn select<'a>(&self, tracks: &'a [IndexedTrack]) -> Vec<&'a IndexedTrack> {
        if let Self::Tracks(track_ids) = self {
            let by_id = tracks
                .iter()
                .map(|track| (track.id, track))
                .collect::<BTreeMap<_, _>>();
            return track_ids
                .iter()
                .filter_map(|track_id| by_id.get(track_id).copied())
                .collect();
        }
        tracks.iter().filter(|track| self.contains(track)).collect()
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct VoiceAnalyzerStatus {
    pub analyzer_id: String,
    pub status: String,
    pub reason: Option<String>,
    pub model_filename: String,
    pub model_sha256: String,
    pub source_signature: Option<String>,
}

impl VoiceAnalyzerStatus {
    #[must_use]
    pub fn not_configured() -> Self {
        Self {
            analyzer_id: VOICE_ANALYZER_ID.to_owned(),
            status: "not_configured".to_owned(),
            reason: Some("model_missing".to_owned()),
            model_filename: VOICE_MODEL_FILENAME.to_owned(),
            model_sha256: VOICE_MODEL_SHA256.to_owned(),
            source_signature: None,
        }
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.status == "ready" && self.source_signature.is_some()
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LibraryContextPassSummary {
    pub completed_tracks: usize,
    pub failed_tracks: usize,
    pub skipped_tracks: usize,
    pub total_tracks: usize,
    pub enabled: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LibraryContextSummary {
    pub analyzer: String,
    pub voice_analyzer: VoiceAnalyzerStatus,
    pub audio_context: LibraryContextPassSummary,
    pub voice_detection: LibraryContextPassSummary,
    pub library_tracks: usize,
    pub analyzed_tracks: usize,
    pub full_tracks: usize,
    pub partial_tracks: usize,
    pub missing_tracks: usize,
    pub failed_tracks: usize,
    pub stale_tracks: usize,
    pub high_confidence: usize,
    pub medium_confidence: usize,
    pub low_confidence: usize,
    pub last_updated_at_unix_seconds: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackContextDetail {
    pub track_id: TrackId,
    pub title: String,
    pub artist: String,
    pub status: String,
    pub analyzer_id: String,
    pub confidence: Option<String>,
    pub updated_at_unix_seconds: Option<i64>,
    pub summary: Option<Map<String, Value>>,
    pub timeline: Vec<Map<String, Value>>,
    pub sections: Vec<Map<String, Value>>,
    pub technical: Option<Map<String, Value>>,
    pub stages: Option<Map<String, Value>>,
    pub error: Option<String>,
}

pub trait LocalAnalysisRepository: LibraryRepository {
    fn analysis_states<'a>(
        &'a self,
        analyzer_id: &'a str,
    ) -> AssistantFuture<'a, Vec<AnalysisState>>;
    fn analysis_failures<'a>(
        &'a self,
        analyzer_id: &'a str,
    ) -> AssistantFuture<'a, Vec<AnalysisFailureState>>;

    /// Store a batch only while each indexed track still has the signature the
    /// analyzer consumed. This prevents a concurrent reconciliation from
    /// publishing stale analysis as current.
    fn store_metadata_analysis<'a>(
        &'a self,
        analyzer_id: &'a str,
        job_id: &'a str,
        profiles: &'a [AnalysisWrite],
    ) -> AssistantFuture<'a, usize>;
    fn store_audio_analysis<'a>(
        &'a self,
        analyzer_id: &'a str,
        job_id: &'a str,
        profile: &'a AnalysisWrite,
    ) -> AssistantFuture<'a, bool>;
    /// Store generated model profiles only while their complete source
    /// signatures still match the current metadata, local context, provider
    /// role, and vocabulary. Implementations must perform the comparison and
    /// write in the same transaction.
    fn store_model_analysis<'a>(
        &'a self,
        analyzer_id: &'a str,
        job_id: &'a str,
        role_fingerprint: &'a str,
        vocabulary_fingerprint: &'a str,
        voice_signature: Option<&'a str>,
        profiles: &'a [ModelAnalysisWrite],
    ) -> AssistantFuture<'a, usize>;
    fn store_analysis_failure<'a>(
        &'a self,
        analyzer_id: &'a str,
        job_id: &'a str,
        failure: &'a AnalysisFailureWrite,
    ) -> AssistantFuture<'a, bool>;
    fn context_states<'a>(&'a self, analyzer_id: &'a str)
    -> AssistantFuture<'a, Vec<ContextState>>;
    fn store_context<'a>(
        &'a self,
        analyzer_id: &'a str,
        implementation_id: &'a str,
        voice_signature: Option<&'a str>,
        job_id: &'a str,
        document: &'a ContextWrite,
    ) -> AssistantFuture<'a, bool>;
    fn store_context_failure<'a>(
        &'a self,
        analyzer_id: &'a str,
        implementation_id: &'a str,
        voice_signature: Option<&'a str>,
        job_id: &'a str,
        failure: &'a AnalysisFailureWrite,
    ) -> AssistantFuture<'a, bool>;
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LibraryAnalysisSummary {
    pub analyzer: String,
    pub library_tracks: usize,
    pub analyzed_tracks: usize,
    pub failed_tracks: usize,
    pub stale_tracks: usize,
    pub high_confidence: usize,
    pub medium_confidence: usize,
    pub low_confidence: usize,
    pub last_updated_at_unix_seconds: Option<i64>,
}

#[derive(Debug)]
pub enum LocalAnalysisError {
    InvalidSourceSignature,
    InvalidStoredState,
    Dependency(Box<dyn Error + Send + Sync>),
}

impl Display for LocalAnalysisError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSourceSignature => "track metadata could not be fingerprinted",
            Self::InvalidStoredState => "stored local analysis state is invalid",
            Self::Dependency(_) => "local analysis storage is unavailable",
        })
    }
}

impl Error for LocalAnalysisError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Dependency(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LocalAnalysisService {
    repository: Arc<dyn LocalAnalysisRepository>,
    voice_analyzer: VoiceAnalyzerStatus,
}

impl LocalAnalysisService {
    #[must_use]
    pub fn new(repository: Arc<dyn LocalAnalysisRepository>) -> Self {
        Self {
            repository,
            voice_analyzer: VoiceAnalyzerStatus::not_configured(),
        }
    }

    #[must_use]
    pub fn with_voice_analyzer(
        repository: Arc<dyn LocalAnalysisRepository>,
        voice_analyzer: VoiceAnalyzerStatus,
    ) -> Self {
        Self {
            repository,
            voice_analyzer,
        }
    }

    pub async fn metadata_summary(&self) -> Result<LibraryAnalysisSummary, LocalAnalysisError> {
        let tracks = self
            .repository
            .all_tracks()
            .await
            .map_err(LocalAnalysisError::Dependency)?;
        let states = self
            .repository
            .analysis_states(LOCAL_METADATA_ANALYZER_ID)
            .await
            .map_err(LocalAnalysisError::Dependency)?;
        summarize_analysis(&tracks, &states, LOCAL_METADATA_ANALYZER_ID)
    }

    pub async fn audio_summary(&self) -> Result<LibraryAnalysisSummary, LocalAnalysisError> {
        let tracks = self
            .repository
            .all_tracks()
            .await
            .map_err(LocalAnalysisError::Dependency)?;
        let states = self
            .repository
            .analysis_states(LOCAL_AUDIO_ANALYZER_ID)
            .await
            .map_err(LocalAnalysisError::Dependency)?;
        let failures = self
            .repository
            .analysis_failures(LOCAL_AUDIO_ANALYZER_ID)
            .await
            .map_err(LocalAnalysisError::Dependency)?;
        summarize_audio_analysis(&tracks, &states, &failures)
    }

    pub async fn context_summary(&self) -> Result<LibraryContextSummary, LocalAnalysisError> {
        let (tracks, states, failures) = tokio::try_join!(
            self.repository.all_tracks(),
            self.repository.context_states(LOCAL_CONTEXT_ANALYZER_ID),
            self.repository.analysis_failures(LOCAL_CONTEXT_ANALYZER_ID),
        )
        .map_err(LocalAnalysisError::Dependency)?;
        summarize_context(&tracks, &states, &failures, &self.voice_analyzer)
    }

    pub async fn context_detail(
        &self,
        track_id: TrackId,
    ) -> Result<Option<TrackContextDetail>, LocalAnalysisError> {
        let (tracks, states, failures) = tokio::try_join!(
            self.repository.all_tracks(),
            self.repository.context_states(LOCAL_CONTEXT_ANALYZER_ID),
            self.repository.analysis_failures(LOCAL_CONTEXT_ANALYZER_ID),
        )
        .map_err(LocalAnalysisError::Dependency)?;
        let Some(track) = tracks.iter().find(|track| track.id == track_id) else {
            return Ok(None);
        };
        let row = states.iter().find(|state| state.track_id == track_id);
        let failure = failures.iter().find(|state| state.track_id == track_id);
        Ok(Some(context_detail(
            track,
            row,
            failure,
            self.voice_analyzer.source_signature.as_deref(),
        )?))
    }

    pub async fn current_contexts(
        &self,
        tracks: &[IndexedTrack],
    ) -> Result<BTreeMap<TrackId, CurrentTrackContext>, LocalAnalysisError> {
        let states = self
            .repository
            .context_states(LOCAL_CONTEXT_ANALYZER_ID)
            .await
            .map_err(LocalAnalysisError::Dependency)?
            .into_iter()
            .map(|state| (state.track_id, state))
            .collect::<BTreeMap<_, _>>();
        let mut current = BTreeMap::new();
        for track in tracks {
            let expected = context_source_signature(
                track,
                LOCAL_CONTEXT_IMPLEMENTATION_ID,
                self.voice_analyzer.source_signature.as_deref(),
            )
            .map_err(|_| LocalAnalysisError::InvalidSourceSignature)?;
            let Some(state) = states
                .get(&track.id)
                .filter(|state| state.source_signature == expected)
            else {
                continue;
            };
            if let Some(context) = parse_context_state(state) {
                current.insert(track.id, context);
            }
        }
        Ok(current)
    }

    #[must_use]
    pub fn voice_analyzer(&self) -> &VoiceAnalyzerStatus {
        &self.voice_analyzer
    }
}

#[derive(Debug)]
pub struct MetadataAnalysisJobHandler {
    repository: Arc<dyn LocalAnalysisRepository>,
}

impl MetadataAnalysisJobHandler {
    #[must_use]
    pub fn new(repository: Arc<dyn LocalAnalysisRepository>) -> Self {
        Self { repository }
    }
}

impl JobHandler for MetadataAnalysisJobHandler {
    fn definition(&self) -> JobDefinition {
        JobDefinition {
            kind: METADATA_ANALYSIS_JOB_KIND,
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
                serde_json::from_value::<MetadataAnalysisParameters>(Value::Object(parameters))
                    .map_err(|_| JobHandlerError::new("invalid metadata analysis parameters"))?;
            let tracks = self
                .repository
                .all_tracks()
                .await
                .map_err(|_| JobHandlerError::new("metadata analysis storage failed"))?;
            let states = self
                .repository
                .analysis_states(LOCAL_METADATA_ANALYZER_ID)
                .await
                .map_err(|_| JobHandlerError::new("metadata analysis storage failed"))?;
            let existing = states
                .iter()
                .map(|state| (state.track_id, state))
                .collect::<BTreeMap<_, _>>();
            let mut work = Vec::new();
            for track in &tracks {
                let signature = metadata_source_signature(track)
                    .map_err(|_| JobHandlerError::new("track metadata fingerprint failed"))?;
                let current = existing.get(&track.id).is_some_and(|state| {
                    if parameters.force {
                        state.job_id == context.job_id()
                    } else {
                        state.source_signature == signature
                    }
                });
                if !current {
                    work.push((track, signature));
                }
            }

            let starting = context.progress_current();
            let work_count = u64::try_from(work.len())
                .map_err(|_| JobHandlerError::new("metadata analysis is too large"))?;
            let total = context
                .progress_total()
                .unwrap_or(0)
                .max(starting.saturating_add(work_count));
            context
                .update_progress(
                    JobProgress::new(
                        starting,
                        Some(total),
                        "Profiling library",
                        format!("{} track profiles need updating", work.len()),
                    )
                    .map_err(|_| JobHandlerError::new("invalid metadata analysis progress"))?,
                )
                .await
                .map_err(JobHandlerError::from_execution)?;

            let mut processed = 0_u64;
            for chunk in work.chunks(METADATA_ANALYSIS_BATCH_SIZE) {
                context
                    .check_cancelled()
                    .await
                    .map_err(JobHandlerError::from_execution)?;
                let profiles = chunk
                    .iter()
                    .map(|(track, signature)| analyze_metadata(track, signature.clone()))
                    .collect::<Vec<_>>();
                let _stored = self
                    .repository
                    .store_metadata_analysis(
                        LOCAL_METADATA_ANALYZER_ID,
                        context.job_id(),
                        &profiles,
                    )
                    .await
                    .map_err(|_| JobHandlerError::new("metadata analysis storage failed"))?;
                processed = processed.saturating_add(
                    u64::try_from(chunk.len())
                        .map_err(|_| JobHandlerError::new("metadata analysis is too large"))?,
                );
                let current = total.min(starting.saturating_add(processed));
                context
                    .update_progress(
                        JobProgress::new(
                            current,
                            Some(total),
                            "Profiling library",
                            format!("Processed {current} of {total} tracks"),
                        )
                        .map_err(|_| JobHandlerError::new("invalid metadata analysis progress"))?,
                    )
                    .await
                    .map_err(JobHandlerError::from_execution)?;
            }
            if starting.saturating_add(processed) < total {
                context
                    .update_progress(
                        JobProgress::new(
                            total,
                            Some(total),
                            "Profiling library",
                            format!("Processed {total} of {total} tracks"),
                        )
                        .map_err(|_| JobHandlerError::new("invalid metadata analysis progress"))?,
                    )
                    .await
                    .map_err(JobHandlerError::from_execution)?;
            }

            let final_states = self
                .repository
                .analysis_states(LOCAL_METADATA_ANALYZER_ID)
                .await
                .map_err(|_| JobHandlerError::new("metadata analysis storage failed"))?;
            let updated = final_states
                .iter()
                .filter(|state| state.job_id == context.job_id())
                .count();
            Ok(json!({
                "tracks": tracks.len(),
                "updated": updated,
                "unchanged": tracks.len().saturating_sub(updated),
                "current_profiles": final_states.len(),
                "analyzer": LOCAL_METADATA_ANALYZER_ID,
            }))
        })
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct MetadataAnalysisParameters {
    force: bool,
}

fn analyze_metadata(track: &IndexedTrack, source_signature: String) -> AnalysisWrite {
    let profile = metadata_profile(track);
    let mut evidence = Vec::new();
    if !profile.moods.is_empty() {
        evidence.push(format!(
            "Mood metadata: {}",
            profile
                .moods
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(bpm) = track.metadata.bpm {
        evidence.push(format!("Tempo metadata: {bpm} BPM"));
    }
    if !track.metadata.genre.is_empty() {
        evidence.push(format!("Genre metadata: {}", track.metadata.genre));
    }
    if evidence.is_empty() {
        evidence.push("No explicit mood, genre, or tempo metadata".to_owned());
    }
    AnalysisWrite {
        track_id: track.id,
        source_signature,
        energy: profile.energy,
        brightness: profile.brightness,
        tension: profile.tension,
        moods: profile.moods,
        evidence,
        metrics: Map::new(),
        confidence: profile.confidence,
    }
}

fn summarize_analysis(
    tracks: &[IndexedTrack],
    states: &[AnalysisState],
    analyzer: &str,
) -> Result<LibraryAnalysisSummary, LocalAnalysisError> {
    let by_track = states
        .iter()
        .map(|state| (state.track_id, state))
        .collect::<BTreeMap<_, _>>();
    let mut current = Vec::new();
    let mut stale_tracks = 0_usize;
    for track in tracks {
        let Some(state) = by_track.get(&track.id) else {
            continue;
        };
        let signature = metadata_source_signature(track)
            .map_err(|_| LocalAnalysisError::InvalidSourceSignature)?;
        if state.source_signature == signature {
            current.push(*state);
        } else {
            stale_tracks = stale_tracks.saturating_add(1);
        }
    }
    let valid_confidences = current
        .iter()
        .filter_map(|state| Confidence::parse(&state.confidence).map(|value| (state, value)))
        .collect::<Vec<_>>();
    if valid_confidences.len() != current.len() {
        return Err(LocalAnalysisError::InvalidStoredState);
    }
    Ok(LibraryAnalysisSummary {
        analyzer: analyzer.to_owned(),
        library_tracks: tracks.len(),
        analyzed_tracks: current.len(),
        failed_tracks: 0,
        stale_tracks,
        high_confidence: valid_confidences
            .iter()
            .filter(|(_, value)| *value == Confidence::High)
            .count(),
        medium_confidence: valid_confidences
            .iter()
            .filter(|(_, value)| *value == Confidence::Medium)
            .count(),
        low_confidence: valid_confidences
            .iter()
            .filter(|(_, value)| *value == Confidence::Low)
            .count(),
        last_updated_at_unix_seconds: current
            .iter()
            .map(|state| state.updated_at_unix_seconds)
            .max(),
    })
}

fn summarize_audio_analysis(
    tracks: &[IndexedTrack],
    states: &[AnalysisState],
    failures: &[AnalysisFailureState],
) -> Result<LibraryAnalysisSummary, LocalAnalysisError> {
    let by_track = states
        .iter()
        .map(|state| (state.track_id, state))
        .collect::<BTreeMap<_, _>>();
    let failures_by_track = failures
        .iter()
        .map(|state| (state.track_id, state))
        .collect::<BTreeMap<_, _>>();
    let mut current = Vec::new();
    let mut current_failures = Vec::new();
    let mut stale_tracks = 0_usize;
    for track in tracks {
        let signature = audio_source_signature(track)
            .map_err(|_| LocalAnalysisError::InvalidSourceSignature)?;
        if let Some(state) = by_track.get(&track.id) {
            if state.source_signature == signature {
                current.push(*state);
            } else {
                stale_tracks = stale_tracks.saturating_add(1);
            }
        }
        if let Some(failure) = failures_by_track.get(&track.id)
            && failure.source_signature == signature
        {
            current_failures.push(*failure);
        }
    }
    let valid_confidences = current
        .iter()
        .filter_map(|state| Confidence::parse(&state.confidence).map(|value| (state, value)))
        .collect::<Vec<_>>();
    if valid_confidences.len() != current.len() {
        return Err(LocalAnalysisError::InvalidStoredState);
    }
    Ok(LibraryAnalysisSummary {
        analyzer: LOCAL_AUDIO_ANALYZER_ID.to_owned(),
        library_tracks: tracks.len(),
        analyzed_tracks: current.len(),
        failed_tracks: current_failures.len(),
        stale_tracks,
        high_confidence: valid_confidences
            .iter()
            .filter(|(_, value)| *value == Confidence::High)
            .count(),
        medium_confidence: valid_confidences
            .iter()
            .filter(|(_, value)| *value == Confidence::Medium)
            .count(),
        low_confidence: valid_confidences
            .iter()
            .filter(|(_, value)| *value == Confidence::Low)
            .count(),
        last_updated_at_unix_seconds: current
            .iter()
            .map(|state| state.updated_at_unix_seconds)
            .chain(
                current_failures
                    .iter()
                    .map(|state| state.updated_at_unix_seconds),
            )
            .max(),
    })
}

pub fn context_source_signature(
    track: &IndexedTrack,
    implementation_id: &str,
    voice_signature: Option<&str>,
) -> Result<String, String> {
    let mut source_facts = vec![
        Value::String(implementation_id.to_owned()),
        Value::String(track.path.as_str().to_owned()),
        json!(track.size_bytes),
        json!(track.mtime_unix_seconds),
    ];
    if let Some(voice_signature) = voice_signature {
        source_facts.push(Value::String(voice_signature.to_owned()));
    }
    serde_json::to_vec(&source_facts)
        .map(|encoded| format!("{:x}", Sha256::digest(encoded)))
        .map_err(|_| "track context source signature could not be encoded".to_owned())
}

pub fn parse_context_state(state: &ContextState) -> Option<CurrentTrackContext> {
    if state.completeness != "full" && state.completeness != "partial" {
        return None;
    }
    Confidence::parse(&state.confidence)?;
    let summary = serde_json::from_str::<Value>(&state.summary_json)
        .ok()?
        .as_object()?
        .clone();
    if summary.get("schema_version").and_then(Value::as_str) != Some(LOCAL_CONTEXT_ANALYZER_ID) {
        return None;
    }
    let timeline = parse_object_array(&state.timeline_json)?;
    if timeline
        .iter()
        .flat_map(Map::values)
        .any(|value| !value.is_number())
    {
        return None;
    }
    let sections = parse_object_array(&state.sections_json)?;
    let technical = serde_json::from_str::<Value>(&state.technical_json)
        .ok()?
        .as_object()?
        .clone();
    let stages = serde_json::from_str::<Value>(&state.stages_json)
        .ok()?
        .as_object()?
        .clone();
    Some(CurrentTrackContext {
        analyzer_id: LOCAL_CONTEXT_ANALYZER_ID.to_owned(),
        source_signature: state.source_signature.clone(),
        completeness: state.completeness.clone(),
        confidence: state.confidence.clone(),
        summary,
        timeline,
        sections,
        technical,
        stages,
    })
}

fn parse_object_array(value: &str) -> Option<Vec<Map<String, Value>>> {
    serde_json::from_str::<Value>(value)
        .ok()?
        .as_array()?
        .iter()
        .map(|value| value.as_object().cloned())
        .collect()
}

fn summarize_context(
    tracks: &[IndexedTrack],
    states: &[ContextState],
    failures: &[AnalysisFailureState],
    voice_analyzer: &VoiceAnalyzerStatus,
) -> Result<LibraryContextSummary, LocalAnalysisError> {
    let states_by_track = states
        .iter()
        .map(|state| (state.track_id, state))
        .collect::<BTreeMap<_, _>>();
    let failures_by_track = failures
        .iter()
        .map(|failure| (failure.track_id, failure))
        .collect::<BTreeMap<_, _>>();
    let voice_signature = voice_analyzer.source_signature.as_deref();
    let mut current = Vec::new();
    let mut current_failures = Vec::new();
    let mut stale_tracks = 0_usize;
    for track in tracks {
        let signature =
            context_source_signature(track, LOCAL_CONTEXT_IMPLEMENTATION_ID, voice_signature)
                .map_err(|_| LocalAnalysisError::InvalidSourceSignature)?;
        let state = states_by_track.get(&track.id).copied();
        let parsed = state
            .filter(|state| state.source_signature == signature)
            .and_then(parse_context_state);
        let has_current = parsed.is_some();
        if let (Some(state), Some(parsed)) = (state, parsed) {
            current.push((state, parsed));
        } else if state.is_some() {
            stale_tracks = stale_tracks.saturating_add(1);
        }
        if !has_current
            && let Some(failure) = failures_by_track.get(&track.id)
            && failure.source_signature == signature
        {
            current_failures.push(*failure);
        }
    }
    let full_tracks = current
        .iter()
        .filter(|(_, context)| context.completeness == "full")
        .count();
    let partial_tracks = current
        .iter()
        .filter(|(_, context)| context.completeness == "partial")
        .count();
    let high_confidence = current
        .iter()
        .filter(|(_, context)| context.confidence == "high")
        .count();
    let medium_confidence = current
        .iter()
        .filter(|(_, context)| context.confidence == "medium")
        .count();
    let low_confidence = current
        .iter()
        .filter(|(_, context)| context.confidence == "low")
        .count();
    let voice_complete = current
        .iter()
        .filter(|(_, context)| {
            voice_stage_status(context).is_some_and(|status| {
                !matches!(status, "pending" | "unavailable" | "not_configured")
            })
        })
        .count();
    let voice_failed = current
        .iter()
        .filter(|(_, context)| voice_stage_status(context) == Some("unavailable"))
        .count();
    let voice_enabled = voice_analyzer.is_ready();
    Ok(LibraryContextSummary {
        analyzer: LOCAL_CONTEXT_ANALYZER_ID.to_owned(),
        voice_analyzer: voice_analyzer.clone(),
        audio_context: LibraryContextPassSummary {
            completed_tracks: current.len(),
            failed_tracks: current_failures.len(),
            skipped_tracks: 0,
            total_tracks: tracks.len(),
            enabled: true,
        },
        voice_detection: LibraryContextPassSummary {
            completed_tracks: if voice_enabled { voice_complete } else { 0 },
            failed_tracks: if voice_enabled { voice_failed } else { 0 },
            skipped_tracks: if voice_enabled { 0 } else { tracks.len() },
            total_tracks: tracks.len(),
            enabled: voice_enabled,
        },
        library_tracks: tracks.len(),
        analyzed_tracks: current.len(),
        full_tracks,
        partial_tracks,
        missing_tracks: tracks
            .len()
            .saturating_sub(current.len())
            .saturating_sub(current_failures.len()),
        failed_tracks: current_failures.len(),
        stale_tracks,
        high_confidence,
        medium_confidence,
        low_confidence,
        last_updated_at_unix_seconds: current
            .iter()
            .map(|(state, _)| state.updated_at_unix_seconds)
            .chain(
                current_failures
                    .iter()
                    .map(|failure| failure.updated_at_unix_seconds),
            )
            .max(),
    })
}

fn context_detail(
    track: &IndexedTrack,
    state: Option<&ContextState>,
    failure: Option<&AnalysisFailureState>,
    voice_signature: Option<&str>,
) -> Result<TrackContextDetail, LocalAnalysisError> {
    let signature =
        context_source_signature(track, LOCAL_CONTEXT_IMPLEMENTATION_ID, voice_signature)
            .map_err(|_| LocalAnalysisError::InvalidSourceSignature)?;
    let parsed = state
        .filter(|state| state.source_signature == signature)
        .and_then(parse_context_state);
    let title = if track.display_title.is_empty() {
        track.metadata.title.clone()
    } else {
        track.display_title.clone()
    };
    if let Some(parsed) = parsed {
        return Ok(TrackContextDetail {
            track_id: track.id,
            title,
            artist: track.metadata.artist.clone(),
            status: parsed.completeness.clone(),
            analyzer_id: LOCAL_CONTEXT_ANALYZER_ID.to_owned(),
            confidence: Some(parsed.confidence),
            updated_at_unix_seconds: state.map(|state| state.updated_at_unix_seconds),
            summary: Some(parsed.summary),
            timeline: parsed.timeline,
            sections: parsed.sections,
            technical: Some(parsed.technical),
            stages: Some(parsed.stages),
            error: None,
        });
    }
    let (status, error, updated_at) =
        if let Some(failure) = failure.filter(|failure| failure.source_signature == signature) {
            (
                "failed",
                Some(failure.error.clone()),
                Some(failure.updated_at_unix_seconds),
            )
        } else if let Some(state) = state {
            ("stale", None, Some(state.updated_at_unix_seconds))
        } else {
            ("missing", None, None)
        };
    Ok(TrackContextDetail {
        track_id: track.id,
        title,
        artist: track.metadata.artist.clone(),
        status: status.to_owned(),
        analyzer_id: LOCAL_CONTEXT_ANALYZER_ID.to_owned(),
        confidence: None,
        updated_at_unix_seconds: updated_at,
        summary: None,
        timeline: Vec::new(),
        sections: Vec::new(),
        technical: None,
        stages: None,
        error,
    })
}

fn voice_stage_status(context: &CurrentTrackContext) -> Option<&str> {
    context
        .stages
        .get("voice")
        .and_then(Value::as_object)
        .and_then(|stage| stage.get("status"))
        .and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use music_domain::{IndexedTrack, LibraryPath, TrackId, TrackMetadata};

    use super::{ContextScope, ContextState, analyze_metadata, parse_context_state};
    use crate::assistant::{
        Confidence, LOCAL_CONTEXT_ANALYZER_ID, LOCAL_CONTEXT_IMPLEMENTATION_ID,
        context_source_signature, metadata_source_signature,
    };

    fn track() -> Result<IndexedTrack, Box<dyn std::error::Error>> {
        Ok(IndexedTrack {
            id: TrackId::new(7)?,
            path: LibraryPath::parse("Battle/Final Boss.flac")?,
            metadata: TrackMetadata {
                title: "Final Battle".to_owned(),
                artist: "Composer".to_owned(),
                album_artist: String::new(),
                album: "Dark Adventure".to_owned(),
                track_no: None,
                disc_no: None,
                year: None,
                genre: "Cinematic".to_owned(),
                bpm: Some(180),
            },
            duration: Duration::from_secs(120),
            display_title: String::new(),
            origin: String::new(),
            size_bytes: 42,
            mtime_unix_seconds: 100,
            added_at_unix_seconds: 100,
        })
    }

    #[test]
    fn metadata_profiles_are_deterministic_and_review_only()
    -> Result<(), Box<dyn std::error::Error>> {
        let track = track()?;
        let signature = metadata_source_signature(&track)?;
        let profile = analyze_metadata(&track, signature.clone());
        assert_eq!(profile.source_signature, signature);
        assert_eq!(profile.confidence, Confidence::High);
        assert!(profile.moods.iter().any(|mood| mood == "combat"));
        assert!(
            profile
                .evidence
                .iter()
                .any(|item| item == "Tempo metadata: 180 BPM")
        );
        Ok(())
    }

    #[test]
    fn context_identity_includes_the_rust_implementation_and_optional_voice_backend()
    -> Result<(), Box<dyn std::error::Error>> {
        let track = track()?;
        let without_voice =
            context_source_signature(&track, LOCAL_CONTEXT_IMPLEMENTATION_ID, None)?;
        let with_voice = context_source_signature(
            &track,
            LOCAL_CONTEXT_IMPLEMENTATION_ID,
            Some("essentia-musicnn-voice/v1:model:runtime"),
        )?;
        assert_eq!(without_voice.len(), 64);
        assert_ne!(with_voice, without_voice);
        assert_ne!(
            context_source_signature(&track, "local-context/v2+other/v1", None)?,
            without_voice
        );
        Ok(())
    }

    #[test]
    fn context_parser_rejects_semantically_invalid_rows() -> Result<(), Box<dyn std::error::Error>>
    {
        let valid = ContextState {
            track_id: TrackId::new(7)?,
            source_signature: "a".repeat(64),
            job_id: "job".to_owned(),
            completeness: "full".to_owned(),
            confidence: "medium".to_owned(),
            summary_json: serde_json::json!({
                "schema_version": LOCAL_CONTEXT_ANALYZER_ID,
            })
            .to_string(),
            timeline_json: "[{\"start_s\":0.0}]".to_owned(),
            sections_json: "[{\"id\":\"s1\"}]".to_owned(),
            technical_json: "{}".to_owned(),
            stages_json: "{}".to_owned(),
            updated_at_unix_seconds: 100,
        };
        assert!(parse_context_state(&valid).is_some());
        let mut invalid = valid;
        invalid.timeline_json = "[{\"start_s\":\"zero\"}]".to_owned();
        assert!(parse_context_state(&invalid).is_none());
        Ok(())
    }

    #[test]
    fn context_folder_scope_respects_direct_and_recursive_boundaries()
    -> Result<(), Box<dyn std::error::Error>> {
        let first = track()?;
        let mut second = first.clone();
        second.id = TrackId::new(8)?;
        second.path = LibraryPath::parse("Battle/Act Two/Finale.flac")?;
        let tracks = vec![first, second];
        let direct = ContextScope::Folder {
            path: Some(LibraryPath::parse("Battle")?),
            recursive: false,
        };
        let recursive = ContextScope::Folder {
            path: Some(LibraryPath::parse("Battle")?),
            recursive: true,
        };
        assert_eq!(direct.select(&tracks).len(), 1);
        assert_eq!(recursive.select(&tracks).len(), 2);
        Ok(())
    }
}
