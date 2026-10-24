//! Background conversion of library tracks into lighter streaming copies. The
//! job encodes beside each track and hands the result to the library
//! coordinator, which keeps the replaced file as the track's original.

use std::sync::Arc;

use music_application::jobs::{
    JobCheckpointPolicy, JobDefinition, JobExecutionContext, JobHandler, JobHandlerError,
    JobHandlerFuture, JobLane, JobProgress, parse_parameters,
};
use music_application::library::{
    LibraryCoordinatorError, LibraryCoordinatorHandle, LibraryService,
};
use music_domain::{IndexedTrack, LibraryPath, TrackId};
use music_media::{
    LibraryRoot, MetadataError, RootedPathError, StreamingAssessment, StreamingEncoder,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::blocking::BlockingMediaExecutor;

pub(crate) const LIBRARY_CONVERSION_JOB_KIND: &str = "library_conversion";
const FAILURE_SAMPLE_LIMIT: usize = 20;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct ConversionParameters {
    /// Tracks to convert; `None` converts every eligible track in the library.
    pub(crate) track_ids: Option<Vec<i64>>,
}

#[derive(Debug)]
pub(crate) struct LibraryConversionJobHandler {
    library: Arc<LibraryService>,
    coordinator: LibraryCoordinatorHandle,
    root: LibraryRoot,
    encoder: StreamingEncoder,
    /// The local job lane converts one track at a time, so one slot suffices.
    executor: BlockingMediaExecutor,
}

impl LibraryConversionJobHandler {
    pub(crate) fn new(
        library: Arc<LibraryService>,
        coordinator: LibraryCoordinatorHandle,
        root: LibraryRoot,
        encoder: StreamingEncoder,
    ) -> Self {
        Self {
            library,
            coordinator,
            root,
            encoder,
            executor: BlockingMediaExecutor::single(),
        }
    }

    async fn convert(&self, track: &IndexedTrack) -> Result<TrackConversion, String> {
        let staged = conversion_stage_path(&track.path, track.id)?;
        let root = self.root.clone();
        let encoder = self.encoder.clone();
        let source = track.path.clone();
        let stage = staged.clone();
        let encoded = self
            .executor
            .execute(move || encode_beside(&root, &encoder, &source, &stage))
            .await
            .map_err(|error| error.to_string())?;
        match encoded {
            Ok(StreamingAssessment::Worthwhile) => {}
            Ok(StreamingAssessment::AlreadyLight) => return Ok(TrackConversion::AlreadyLight),
            Ok(StreamingAssessment::Unsupported) => return Ok(TrackConversion::Unsupported),
            Err(reason) => return Err(reason),
        }
        match self.coordinator.convert_track(track.id, staged).await {
            Ok(converted) => Ok(TrackConversion::Converted {
                saved_bytes: i64::try_from(track.size_bytes)
                    .unwrap_or(i64::MAX)
                    .saturating_sub(i64::try_from(converted.size_bytes).unwrap_or(i64::MAX)),
            }),
            Err(LibraryCoordinatorError::TrackAlreadyConverted { .. }) => {
                Ok(TrackConversion::AlreadyConverted)
            }
            Err(error) => {
                tracing::warn!(
                    track_id = track.id.get(),
                    error = %music_application::ErrorChain(&error),
                    "track conversion was not published"
                );
                Err(error.to_string())
            }
        }
    }
}

enum TrackConversion {
    Converted { saved_bytes: i64 },
    AlreadyConverted,
    AlreadyLight,
    Unsupported,
}

/// Encodes a streaming copy next to the track when that makes it lighter.
fn encode_beside(
    root: &LibraryRoot,
    encoder: &StreamingEncoder,
    source: &LibraryPath,
    staged: &LibraryPath,
) -> Result<StreamingAssessment, String> {
    let source = root
        .resolve_existing_file_for_mutation(source)
        .map_err(|error| rooted_reason(&error))?;
    let assessment = encoder
        .assess(&source)
        .map_err(|error| metadata_reason(&error))?;
    if assessment != StreamingAssessment::Worthwhile {
        return Ok(assessment);
    }
    let staged = root
        .resolve_for_creation(staged)
        .map_err(|error| rooted_reason(&error))?;
    // Only this job writes stage files, so a leftover one is from an interrupted run.
    match std::fs::remove_file(&staged) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("a previous encoding could not be removed".to_owned()),
    }
    encoder
        .encode(&source, &staged)
        .map_err(|error| metadata_reason(&error))?;
    // A copy that is not lighter than its source is never published.
    let size = |path: &std::path::Path| std::fs::metadata(path).map(|facts| facts.len()).ok();
    if size(&staged) >= size(&source) {
        let _ = std::fs::remove_file(&staged);
        return Ok(StreamingAssessment::AlreadyLight);
    }
    Ok(assessment)
}

fn conversion_stage_path(path: &LibraryPath, track_id: TrackId) -> Result<LibraryPath, String> {
    let name = format!(".convert-{}.partial", track_id.get());
    path.parent()
        .map_or_else(|| LibraryPath::parse(&name), |parent| parent.join(&name))
        .map_err(|_| "the track path is too long to stage a conversion".to_owned())
}

fn rooted_reason(error: &RootedPathError) -> String {
    tracing::warn!(error = %music_application::ErrorChain(error), "conversion source is unavailable");
    "the track file is unavailable".to_owned()
}

fn metadata_reason(error: &MetadataError) -> String {
    tracing::warn!(error = %music_application::ErrorChain(error), "streaming copy failed");
    error.to_string()
}

impl JobHandler for LibraryConversionJobHandler {
    fn definition(&self) -> JobDefinition {
        JobDefinition {
            kind: LIBRARY_CONVERSION_JOB_KIND,
            schema_version: 1,
            lane: JobLane::Local,
            // Converted tracks are skipped, so a restart resumes where it stopped.
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
            let parameters: ConversionParameters =
                parse_parameters(parameters, "invalid library conversion parameters")?;
            let tracks = match parameters.track_ids {
                Some(raw_ids) => {
                    let ids = raw_ids
                        .into_iter()
                        .filter_map(|id| TrackId::new(id).ok())
                        .collect::<Vec<_>>();
                    self.library.tracks_by_ids(&ids).await
                }
                None => self.library.all_tracks().await,
            }
            .map_err(|error| {
                JobHandlerError::caused_by("library tracks are unavailable", error.as_ref())
            })?
            .into_iter()
            .filter(|track| track.original.is_none())
            .collect::<Vec<_>>();

            let total = u64::try_from(tracks.len()).unwrap_or(u64::MAX);
            let mut report = ConversionReport::default();
            for (index, track) in tracks.iter().enumerate() {
                context
                    .check_cancelled()
                    .await
                    .map_err(JobHandlerError::from_execution)?;
                let progress = JobProgress::new(
                    u64::try_from(index).unwrap_or(u64::MAX),
                    Some(total),
                    "converting",
                    track.path.as_str(),
                )
                .map_err(|_| JobHandlerError::new("conversion progress is invalid"))?;
                context
                    .update_progress(progress)
                    .await
                    .map_err(JobHandlerError::from_execution)?;
                report.record(track.id, self.convert(track).await);
                context
                    .checkpoint(report.to_result())
                    .await
                    .map_err(JobHandlerError::from_execution)?;
            }
            let done = JobProgress::new(total, Some(total), "complete", "")
                .map_err(|_| JobHandlerError::new("conversion progress is invalid"))?;
            context
                .update_progress(done)
                .await
                .map_err(JobHandlerError::from_execution)?;
            Ok(Value::Object(report.to_result()))
        })
    }
}

#[derive(Debug, Default)]
struct ConversionReport {
    converted: u64,
    already_converted: u64,
    already_light: u64,
    unsupported: u64,
    failed: u64,
    saved_bytes: i64,
    failures: Vec<Value>,
}

impl ConversionReport {
    fn record(&mut self, track_id: TrackId, outcome: Result<TrackConversion, String>) {
        match outcome {
            Ok(TrackConversion::Converted { saved_bytes }) => {
                self.converted += 1;
                self.saved_bytes = self.saved_bytes.saturating_add(saved_bytes);
            }
            Ok(TrackConversion::AlreadyConverted) => self.already_converted += 1,
            Ok(TrackConversion::AlreadyLight) => self.already_light += 1,
            Ok(TrackConversion::Unsupported) => self.unsupported += 1,
            Err(reason) => {
                self.failed += 1;
                if self.failures.len() < FAILURE_SAMPLE_LIMIT {
                    self.failures
                        .push(json!({"track_id": track_id.get(), "reason": reason}));
                }
            }
        }
    }

    fn to_result(&self) -> Map<String, Value> {
        Map::from_iter([
            ("converted".to_owned(), json!(self.converted)),
            (
                "already_converted".to_owned(),
                json!(self.already_converted),
            ),
            ("already_light".to_owned(), json!(self.already_light)),
            ("unsupported".to_owned(), json!(self.unsupported)),
            ("failed".to_owned(), json!(self.failed)),
            ("saved_bytes".to_owned(), json!(self.saved_bytes)),
            ("failures".to_owned(), Value::Array(self.failures.clone())),
        ])
    }
}
