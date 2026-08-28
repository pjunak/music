use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use music_domain::{IndexedTrack, TrackId};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::planner::metadata_profile;
use super::tags::{
    AssistantFuture, Confidence, LOCAL_METADATA_ANALYZER_ID, metadata_source_signature,
};
use crate::jobs::{
    JobCheckpointPolicy, JobDefinition, JobExecutionContext, JobHandler, JobHandlerError,
    JobHandlerFuture, JobLane, JobProgress,
};
use crate::library::LibraryRepository;

pub const METADATA_ANALYSIS_JOB_KIND: &str = "assistant.library-analysis";
const METADATA_ANALYSIS_BATCH_SIZE: usize = 50;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AnalysisState {
    pub track_id: TrackId,
    pub source_signature: String,
    pub job_id: String,
    pub confidence: String,
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
    pub confidence: Confidence,
}

pub trait LocalAnalysisRepository: LibraryRepository {
    fn analysis_states<'a>(
        &'a self,
        analyzer_id: &'a str,
    ) -> AssistantFuture<'a, Vec<AnalysisState>>;

    /// Store a batch only while each indexed track still has the signature the
    /// analyzer consumed. This prevents a concurrent reconciliation from
    /// publishing stale analysis as current.
    fn store_metadata_analysis<'a>(
        &'a self,
        analyzer_id: &'a str,
        job_id: &'a str,
        profiles: &'a [AnalysisWrite],
    ) -> AssistantFuture<'a, usize>;
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
}

impl LocalAnalysisService {
    #[must_use]
    pub fn new(repository: Arc<dyn LocalAnalysisRepository>) -> Self {
        Self { repository }
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
                .map_err(|error| JobHandlerError::new(error.to_string()))?;

            let mut processed = 0_u64;
            for chunk in work.chunks(METADATA_ANALYSIS_BATCH_SIZE) {
                context
                    .check_cancelled()
                    .await
                    .map_err(|error| JobHandlerError::new(error.to_string()))?;
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
                    .map_err(|error| JobHandlerError::new(error.to_string()))?;
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
                    .map_err(|error| JobHandlerError::new(error.to_string()))?;
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use music_domain::{IndexedTrack, LibraryPath, TrackId, TrackMetadata};

    use super::analyze_metadata;
    use crate::assistant::{Confidence, metadata_source_signature};

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
}
