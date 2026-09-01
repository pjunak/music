use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::TryStreamExt;
use music_application::assistant::{
    AnalysisWrite, AssistantService, CATALOG_TAG_ANALYZER_ID, Confidence, LocalAnalysisRepository,
    TagVocabularySnapshot, metadata_source_signature, normalize_manual_tag,
};
use music_application::cleanup::{CleanupScope, CleanupService};
use music_application::cleanup_enrichment::{
    CLEANUP_ENRICHMENT_JOB_KIND, CLEANUP_ENRICHMENT_SCHEMA, CleanupEnrichmentRecord,
    CleanupEnrichmentRepository, MAX_CLEANUP_ENRICHMENT_TRACKS,
    cleanup_enrichment_source_signature,
};
use music_application::cleanup_sources::{
    ACOUSTID_SOURCE_ID, CleanupSourceService, LASTFM_SOURCE_ID, MUSICBRAINZ_SOURCE_ID,
};
use music_application::jobs::{
    JobCheckpointPolicy, JobDefinition, JobExecutionContext, JobHandler, JobHandlerError,
    JobHandlerFuture, JobLane, JobProgress,
};
use music_domain::{IndexedTrack, TrackId};
use music_media::LibraryRoot;
use music_storage::SecretString;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::process::Command;

use crate::cleanup::MusicBrainzNameLookup;

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const FINGERPRINT_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_CATALOG_TEXT_BYTES: usize = 512;
const MAX_RELEASES: usize = 100;
const MAX_MEDIA: usize = 100;
const MAX_TRACKS_PER_MEDIUM: usize = 1_000;
const ACOUSTID_ENDPOINT: &str = "https://api.acoustid.org/v2/lookup";
const LASTFM_ENDPOINT: &str = "https://ws.audioscrobbler.com/2.0/";
const ACOUSTID_MIN_SCORE: f64 = 0.85;
const ACOUSTID_MIN_MARGIN: f64 = 0.10;
const METADATA_MIN_SCORE: f64 = 0.86;
const METADATA_MIN_MARGIN: f64 = 0.05;
const LASTFM_MIN_TAG_COUNT: u64 = 10;
const MAX_LASTFM_TAGS: usize = 50;
const MAX_CATALOG_TAGS: usize = 8;

#[derive(Debug)]
pub(crate) struct CleanupConnectorConfig {
    acoustid_api_key: Option<SecretString>,
    lastfm_api_key: Option<SecretString>,
    fpcalc_path: PathBuf,
}

impl CleanupConnectorConfig {
    pub(crate) fn new(
        acoustid_api_key: Option<SecretString>,
        lastfm_api_key: Option<SecretString>,
        fpcalc_path: PathBuf,
    ) -> Self {
        Self {
            acoustid_api_key,
            lastfm_api_key,
            fpcalc_path,
        }
    }

    pub(crate) const fn acoustid_configured(&self) -> bool {
        self.acoustid_api_key.is_some()
    }

    pub(crate) const fn lastfm_configured(&self) -> bool {
        self.lastfm_api_key.is_some()
    }

    pub(crate) async fn fpcalc_available(&self) -> bool {
        let command = Command::new(&self.fpcalc_path)
            .arg("-version")
            .kill_on_drop(true)
            .output();
        tokio::time::timeout(Duration::from_secs(5), command)
            .await
            .ok()
            .and_then(Result::ok)
            .is_some_and(|output| output.status.success())
    }
}

#[derive(Debug)]
pub(crate) struct CleanupEnrichmentJobHandler {
    cleanup: Arc<CleanupService>,
    cache: Arc<dyn CleanupEnrichmentRepository>,
    analyses: Arc<dyn LocalAnalysisRepository>,
    assistant: Arc<AssistantService>,
    sources: Arc<CleanupSourceService>,
    musicbrainz: Arc<MusicBrainzNameLookup>,
    library_root: LibraryRoot,
    config: CleanupConnectorConfig,
    http: Client,
}

pub(crate) struct CleanupEnrichmentServices {
    pub(crate) cleanup: Arc<CleanupService>,
    pub(crate) cache: Arc<dyn CleanupEnrichmentRepository>,
    pub(crate) analyses: Arc<dyn LocalAnalysisRepository>,
    pub(crate) assistant: Arc<AssistantService>,
    pub(crate) sources: Arc<CleanupSourceService>,
    pub(crate) musicbrainz: Arc<MusicBrainzNameLookup>,
}

impl CleanupEnrichmentJobHandler {
    pub(crate) fn new(
        services: CleanupEnrichmentServices,
        library_root: LibraryRoot,
        config: CleanupConnectorConfig,
    ) -> Result<Self, reqwest::Error> {
        let http = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("music-dnd-orchestrator/0.1 (https://github.com/pjunak/music)")
            .build()?;
        Ok(Self {
            cleanup: services.cleanup,
            cache: services.cache,
            analyses: services.analyses,
            assistant: services.assistant,
            sources: services.sources,
            musicbrainz: services.musicbrainz,
            library_root,
            config,
            http,
        })
    }

    async fn run(
        &self,
        context: &JobExecutionContext,
        parameters: EnrichmentParameters,
    ) -> Result<Map<String, Value>, JobHandlerError> {
        let scope = parameters
            .scope
            .to_scope()
            .map_err(|_| JobHandlerError::new("cleanup enrichment scope is invalid"))?;
        let tracks =
            self.cleanup.tracks(scope).await.map_err(|_| {
                JobHandlerError::new("cleanup enrichment scope could not be loaded")
            })?;
        if tracks.len() > MAX_CLEANUP_ENRICHMENT_TRACKS {
            return Err(JobHandlerError::new(format!(
                "cleanup enrichment is limited to {MAX_CLEANUP_ENRICHMENT_TRACKS} tracks per run; choose a smaller folder"
            )));
        }
        let source_states = self
            .sources
            .sources()
            .await
            .map_err(|_| JobHandlerError::new("cleanup source settings are unavailable"))?;
        let source = |id: &str| source_states.iter().find(|source| source.id == id);
        let musicbrainz_enabled =
            source(MUSICBRAINZ_SOURCE_ID).is_some_and(|source| source.enabled && source.available);
        if !musicbrainz_enabled {
            return Err(JobHandlerError::new(
                "MusicBrainz must be enabled before tracks can be identified",
            ));
        }
        let acoustid_enabled =
            source(ACOUSTID_SOURCE_ID).is_some_and(|source| source.enabled && source.available);
        let lastfm_enabled =
            source(LASTFM_SOURCE_ID).is_some_and(|source| source.enabled && source.available);
        let active_sources = active_source_ids(acoustid_enabled, lastfm_enabled);
        let vocabulary = if lastfm_enabled {
            Some(
                self.assistant
                    .vocabulary()
                    .await
                    .map_err(|_| JobHandlerError::new("tag vocabulary is unavailable"))?,
            )
        } else {
            None
        };
        let total = u64::try_from(tracks.len())
            .map_err(|_| JobHandlerError::new("cleanup enrichment scope is too large"))?;
        context
            .update_progress(
                JobProgress::new(0, Some(total), "identify", "Preparing catalog lookup")
                    .map_err(|_| JobHandlerError::new("job progress is invalid"))?,
            )
            .await
            .map_err(JobHandlerError::from_execution)?;

        let mut plans = Vec::with_capacity(tracks.len());
        let mut identified = 0_u64;
        let mut fingerprinted = 0_u64;
        let mut unmatched = 0_u64;
        let mut failed = 0_u64;
        let mut cached = 0_u64;
        for (index, track) in tracks.iter().enumerate() {
            context
                .check_cancelled()
                .await
                .map_err(JobHandlerError::from_execution)?;
            let signature =
                cleanup_enrichment_source_signature(track).map_err(JobHandlerError::new)?;
            let result = if !parameters.force {
                self.cache
                    .cleanup_enrichment(track.id)
                    .await
                    .map_err(|_| JobHandlerError::new("cleanup enrichment cache is unavailable"))?
                    .filter(|record| {
                        record.source_signature == signature
                            && cached_sources_match(&record.result, &active_sources)
                    })
                    .map(|record| record.result)
            } else {
                None
            };
            let result = if let Some(result) = result {
                cached = cached.saturating_add(1);
                result
            } else {
                match self
                    .enrich_track(
                        track,
                        acoustid_enabled,
                        lastfm_enabled,
                        vocabulary.as_ref(),
                        context.job_id(),
                    )
                    .await
                {
                    Ok(result) => {
                        if result_is_cacheable(&result) {
                            let record = CleanupEnrichmentRecord {
                                track_id: track.id,
                                source_signature: signature,
                                result: result.clone(),
                            };
                            self.cache
                                .store_cleanup_enrichment(&record)
                                .await
                                .map_err(|_| {
                                    JobHandlerError::new(
                                        "cleanup enrichment cache could not be updated",
                                    )
                                })?;
                        }
                        result
                    }
                    Err(error) => {
                        failed = failed.saturating_add(1);
                        failed_result(track, error.code())
                    }
                }
            };
            match result.get("status").and_then(Value::as_str) {
                Some("identified") => identified = identified.saturating_add(1),
                Some("fingerprinted") => {
                    identified = identified.saturating_add(1);
                    fingerprinted = fingerprinted.saturating_add(1);
                }
                Some("unmatched") => unmatched = unmatched.saturating_add(1),
                _ => {}
            }
            plans.push(Value::Object(result));
            let done = u64::try_from(index + 1)
                .map_err(|_| JobHandlerError::new("job progress overflowed"))?;
            context
                .update_progress(
                    JobProgress::new(
                        done,
                        Some(total),
                        "identify",
                        format!("Processed {done} of {total} tracks"),
                    )
                    .map_err(|_| JobHandlerError::new("job progress is invalid"))?,
                )
                .await
                .map_err(JobHandlerError::from_execution)?;
            if done % 10 == 0 {
                context
                    .checkpoint(enrichment_result(
                        total,
                        identified,
                        fingerprinted,
                        unmatched,
                        failed,
                        cached,
                        plans.clone(),
                    ))
                    .await
                    .map_err(JobHandlerError::from_execution)?;
            }
        }
        Ok(enrichment_result(
            total,
            identified,
            fingerprinted,
            unmatched,
            failed,
            cached,
            plans,
        ))
    }

    async fn enrich_track(
        &self,
        track: &IndexedTrack,
        acoustid_enabled: bool,
        lastfm_enabled: bool,
        vocabulary: Option<&TagVocabularySnapshot>,
        job_id: &str,
    ) -> Result<Map<String, Value>, CatalogError> {
        let metadata_match = self.search_metadata(track).await?;
        let (recording_id, method, confidence) = if let Some(candidate) = metadata_match {
            (candidate.id, "metadata", candidate.local_score)
        } else if acoustid_enabled {
            let recording_id = self.fingerprint_identity(track).await?;
            let Some((recording_id, score)) = recording_id else {
                return Ok(unmatched_result(track, acoustid_enabled, lastfm_enabled));
            };
            (recording_id, "fingerprint", score)
        } else {
            return Ok(unmatched_result(track, acoustid_enabled, lastfm_enabled));
        };

        let recording = self.recording(&recording_id).await?;
        let release = choose_release(track, &recording.releases);
        let mut partial = false;
        let release_detail = match release {
            Some(release) => match self.release(&release.id, &recording_id).await {
                Ok(detail) => Some(detail),
                Err(_) => {
                    partial = true;
                    None
                }
            },
            None => None,
        };
        let metadata = canonical_metadata(&recording, release_detail.as_ref());
        let operations = metadata_operations(track, &metadata, &recording_id);
        let mut tag_suggestions = Vec::new();
        let mut notes = vec![format!(
            "Matched {} — {} via {} evidence ({:.0}% confidence).",
            recording.artist,
            recording.title,
            method,
            confidence * 100.0,
        )];
        if release.is_some() && release_detail.is_none() {
            notes.push(
                "Release details were unavailable; recording-level metadata is still available and the release lookup will retry next run."
                    .to_owned(),
            );
        }
        if lastfm_enabled && let Some(vocabulary) = vocabulary {
            match self
                .lastfm_tags(&recording.artist, &recording.title, vocabulary)
                .await
            {
                Ok(mut suggestions) => {
                    let source_signature = metadata_source_signature(track)
                        .map_err(|_| CatalogError::InvalidResponse)?;
                    for suggestion in &mut suggestions {
                        if let Some(suggestion) = suggestion.as_object_mut() {
                            suggestion.insert(
                                "source_signature".to_owned(),
                                Value::String(source_signature.clone()),
                            );
                        }
                    }
                    if self
                        .store_catalog_tags(track, &recording_id, &suggestions, job_id)
                        .await
                        .is_ok()
                    {
                        tag_suggestions = suggestions;
                    } else {
                        partial = true;
                        notes.push(
                            "Community tags could not be stored for review; metadata proposals are still available."
                                .to_owned(),
                        );
                    }
                }
                Err(_) => {
                    partial = true;
                    notes.push(
                        "Last.fm tag evidence was unavailable; metadata proposals are still available and tags will retry next run."
                            .to_owned(),
                    );
                }
            }
        }
        let status = if method == "fingerprint" {
            "fingerprinted"
        } else {
            "identified"
        };
        json!({
            "schema": CLEANUP_ENRICHMENT_SCHEMA,
            "partial": partial,
            "sources": active_source_ids(acoustid_enabled, lastfm_enabled),
            "track_id": track.id.get(),
            "path": track.path.as_str(),
            "status": status,
            "identity": {
                "recording_mbid": recording_id,
                "method": method,
                "confidence": confidence,
                "title": recording.title,
                "artist": recording.artist,
                "release_mbid": release_detail.as_ref().map(|release| release.id.as_str()),
            },
            "ops": operations,
            "tag_suggestions": tag_suggestions,
            "notes": notes,
        })
        .as_object()
        .cloned()
        .ok_or(CatalogError::InvalidResponse)
    }

    async fn search_metadata(
        &self,
        track: &IndexedTrack,
    ) -> Result<Option<Candidate>, CatalogError> {
        let title = if track.metadata.title.trim().is_empty() {
            track.display_title.trim()
        } else {
            track.metadata.title.trim()
        };
        let artist = track.metadata.artist.trim();
        if title.is_empty() || artist.is_empty() {
            return Ok(None);
        }
        let duration_ms = u64::try_from(track.duration.as_millis()).unwrap_or(u64::MAX);
        let query = format!(
            "recording:{} AND artist:{} AND qdur:{}",
            lucene_quote(title),
            lucene_quote(artist),
            duration_ms / 2_000,
        );
        let payload = self
            .musicbrainz
            .fetch_json(
                "recording",
                &[
                    ("query", query),
                    ("fmt", "json".to_owned()),
                    ("limit", "5".to_owned()),
                ],
            )
            .await
            .map_err(|_| CatalogError::MusicBrainz)?;
        select_candidate(track, &payload)
    }

    async fn recording(&self, recording_id: &str) -> Result<Recording, CatalogError> {
        let payload = self
            .musicbrainz
            .fetch_json(
                &format!("recording/{recording_id}"),
                &[
                    ("fmt", "json".to_owned()),
                    (
                        "inc",
                        "artist-credits+releases+release-groups+genres+tags".to_owned(),
                    ),
                ],
            )
            .await
            .map_err(|_| CatalogError::MusicBrainz)?;
        parse_recording(&payload, recording_id)
    }

    async fn release(
        &self,
        release_id: &str,
        recording_id: &str,
    ) -> Result<ReleaseDetail, CatalogError> {
        let payload = self
            .musicbrainz
            .fetch_json(
                &format!("release/{release_id}"),
                &[
                    ("fmt", "json".to_owned()),
                    (
                        "inc",
                        "recordings+artist-credits+release-groups+media".to_owned(),
                    ),
                ],
            )
            .await
            .map_err(|_| CatalogError::MusicBrainz)?;
        parse_release_detail(&payload, release_id, recording_id)
    }

    async fn fingerprint_identity(
        &self,
        track: &IndexedTrack,
    ) -> Result<Option<(String, f64)>, CatalogError> {
        let key = self
            .config
            .acoustid_api_key
            .as_ref()
            .ok_or(CatalogError::AcoustIdUnavailable)?;
        let absolute = self
            .library_root
            .resolve_existing(&track.path)
            .map_err(|_| CatalogError::Fingerprint)?;
        let command = Command::new(&self.config.fpcalc_path)
            .arg("-json")
            .arg("-length")
            .arg("120")
            .arg("--")
            .arg(absolute)
            .kill_on_drop(true)
            .output();
        let output = tokio::time::timeout(FINGERPRINT_TIMEOUT, command)
            .await
            .map_err(|_| CatalogError::Fingerprint)?
            .map_err(|_| CatalogError::Fingerprint)?;
        if !output.status.success() || output.stdout.len() > MAX_RESPONSE_BYTES {
            return Err(CatalogError::Fingerprint);
        }
        let fingerprint: FingerprintOutput =
            serde_json::from_slice(&output.stdout).map_err(|_| CatalogError::Fingerprint)?;
        if fingerprint.fingerprint.is_empty() || !(1.0..=86_400.0).contains(&fingerprint.duration) {
            return Err(CatalogError::Fingerprint);
        }
        let response = self
            .http
            .post(ACOUSTID_ENDPOINT)
            .form(&[
                ("client", key.expose_secret().to_owned()),
                ("duration", fingerprint.duration.round().to_string()),
                ("fingerprint", fingerprint.fingerprint),
                ("meta", "recordingids".to_owned()),
                ("format", "json".to_owned()),
            ])
            .send()
            .await
            .map_err(|_| CatalogError::AcoustId)?
            .error_for_status()
            .map_err(|_| CatalogError::AcoustId)?;
        let payload = bounded_json(response)
            .await
            .map_err(|_| CatalogError::AcoustId)?;
        parse_acoustid_identity(&payload)
    }

    async fn lastfm_tags(
        &self,
        artist: &str,
        title: &str,
        vocabulary: &TagVocabularySnapshot,
    ) -> Result<Vec<Value>, CatalogError> {
        let key = self
            .config
            .lastfm_api_key
            .as_ref()
            .ok_or(CatalogError::LastFmUnavailable)?;
        let response = self
            .http
            .post(LASTFM_ENDPOINT)
            .form(&[
                ("method", "track.gettoptags"),
                ("artist", artist),
                ("track", title),
                ("api_key", key.expose_secret()),
                ("autocorrect", "0"),
                ("format", "json"),
            ])
            .send()
            .await
            .map_err(|_| CatalogError::LastFm)?
            .error_for_status()
            .map_err(|_| CatalogError::LastFm)?;
        let payload = bounded_json(response)
            .await
            .map_err(|_| CatalogError::LastFm)?;
        map_lastfm_tags(&payload, vocabulary)
    }

    async fn store_catalog_tags(
        &self,
        track: &IndexedTrack,
        recording_id: &str,
        suggestions: &[Value],
        job_id: &str,
    ) -> Result<(), CatalogError> {
        let moods = suggestions
            .iter()
            .filter_map(|suggestion| suggestion.get("tag").and_then(Value::as_str))
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let evidence = if suggestions.is_empty() {
            vec!["No Last.fm community tags matched the controlled vocabulary.".to_owned()]
        } else {
            suggestions
                .iter()
                .filter_map(|suggestion| {
                    Some(format!(
                        "Last.fm community tag: {} (count {})",
                        suggestion.get("source_tag")?.as_str()?,
                        suggestion.get("count")?.as_u64()?,
                    ))
                })
                .collect()
        };
        let source_signature =
            metadata_source_signature(track).map_err(|_| CatalogError::InvalidResponse)?;
        let profile = AnalysisWrite {
            track_id: track.id,
            source_signature,
            energy: 0.5,
            brightness: 0.5,
            tension: 0.5,
            moods,
            evidence,
            metrics: json!({
                "schema": CATALOG_TAG_ANALYZER_ID,
                "recording_mbid": recording_id,
            })
            .as_object()
            .cloned()
            .ok_or(CatalogError::InvalidResponse)?,
            confidence: Confidence::Medium,
        };
        let stored = self
            .analyses
            .store_metadata_analysis(CATALOG_TAG_ANALYZER_ID, job_id, &[profile])
            .await
            .map_err(|_| CatalogError::Storage)?;
        if stored != 1 {
            return Err(CatalogError::Storage);
        }
        Ok(())
    }
}

impl JobHandler for CleanupEnrichmentJobHandler {
    fn definition(&self) -> JobDefinition {
        JobDefinition {
            kind: CLEANUP_ENRICHMENT_JOB_KIND,
            schema_version: 1,
            lane: JobLane::Provider,
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
            let parameters = serde_json::from_value::<EnrichmentParameters>(Value::Object(
                parameters,
            ))
            .map_err(|_| JobHandlerError::new("cleanup enrichment parameters are invalid"))?;
            self.run(context, parameters).await.map(Value::Object)
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrichmentParameters {
    scope: EnrichmentScope,
    #[serde(default)]
    force: bool,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
enum EnrichmentScope {
    All,
    Folder {
        #[serde(default)]
        path: String,
        #[serde(default = "default_true")]
        recursive: bool,
    },
    Tracks {
        track_ids: Vec<i64>,
    },
}

impl EnrichmentScope {
    fn to_scope(&self) -> Result<CleanupScope, ()> {
        match self {
            Self::All => Ok(CleanupScope::All),
            Self::Folder { path, recursive } => Ok(CleanupScope::Folder {
                path: if path.trim().is_empty() {
                    None
                } else {
                    Some(music_domain::LibraryPath::parse(path).map_err(|_| ())?)
                },
                recursive: *recursive,
            }),
            Self::Tracks { track_ids } => track_ids
                .iter()
                .map(|id| TrackId::new(*id).map_err(|_| ()))
                .collect::<Result<Vec<_>, _>>()
                .map(CleanupScope::Tracks),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FingerprintOutput {
    duration: f64,
    fingerprint: String,
}

#[derive(Debug, Clone)]
struct Candidate {
    id: String,
    title: String,
    artist: String,
    length_ms: Option<u64>,
    releases: Vec<ReleaseSummary>,
    provider_score: f64,
    local_score: f64,
}

#[derive(Debug, Clone)]
struct Recording {
    title: String,
    artist: String,
    first_release_date: Option<String>,
    releases: Vec<ReleaseSummary>,
}

#[derive(Debug, Clone)]
struct ReleaseSummary {
    id: String,
    title: String,
    status: Option<String>,
}

#[derive(Debug, Clone)]
struct ReleaseDetail {
    id: String,
    title: String,
    artist: String,
    date: Option<String>,
    track_no: Option<u32>,
    disc_no: Option<u32>,
}

#[derive(Debug, Clone)]
struct CanonicalMetadata {
    title: String,
    artist: String,
    album_artist: String,
    album: String,
    track_no: Option<u32>,
    disc_no: Option<u32>,
    year: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
enum CatalogError {
    MusicBrainz,
    AcoustIdUnavailable,
    AcoustId,
    Fingerprint,
    LastFmUnavailable,
    LastFm,
    Storage,
    InvalidResponse,
}

impl CatalogError {
    const fn code(self) -> &'static str {
        match self {
            Self::MusicBrainz => "musicbrainz_unavailable",
            Self::AcoustIdUnavailable => "acoustid_not_configured",
            Self::AcoustId => "acoustid_unavailable",
            Self::Fingerprint => "fingerprint_failed",
            Self::LastFmUnavailable => "lastfm_not_configured",
            Self::LastFm => "lastfm_unavailable",
            Self::Storage => "catalog_suggestions_not_stored",
            Self::InvalidResponse => "catalog_response_invalid",
        }
    }
}

impl Display for CatalogError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

async fn bounded_json(response: reqwest::Response) -> Result<Value, CatalogError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(CatalogError::InvalidResponse);
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream
        .try_next()
        .await
        .map_err(|_| CatalogError::InvalidResponse)?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(CatalogError::InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| CatalogError::InvalidResponse)
}

fn select_candidate(
    track: &IndexedTrack,
    payload: &Value,
) -> Result<Option<Candidate>, CatalogError> {
    let entries = payload
        .get("recordings")
        .and_then(Value::as_array)
        .ok_or(CatalogError::InvalidResponse)?;
    let mut candidates = entries
        .iter()
        .filter_map(parse_candidate)
        .map(|mut candidate| {
            candidate.local_score = candidate_score(track, &candidate);
            candidate
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.local_score.total_cmp(&left.local_score));
    let Some(best) = candidates.first() else {
        return Ok(None);
    };
    let margin = candidates
        .get(1)
        .map_or(1.0, |next| best.local_score - next.local_score);
    let title = if track.metadata.title.trim().is_empty() {
        track.display_title.as_str()
    } else {
        track.metadata.title.as_str()
    };
    let exact_title = loose_equal(title, &best.title);
    let exact_artist = loose_equal(&track.metadata.artist, &best.artist);
    let duration_close = best.length_ms.is_none_or(|length| {
        let expected = track.duration.as_millis() as i128;
        (expected - i128::from(length)).abs() <= 10_000
    });
    if best.local_score >= METADATA_MIN_SCORE
        && margin >= METADATA_MIN_MARGIN
        && exact_title
        && exact_artist
        && duration_close
    {
        Ok(Some(best.clone()))
    } else {
        Ok(None)
    }
}

fn parse_candidate(value: &Value) -> Option<Candidate> {
    let id = parse_mbid(value.get("id")?)?;
    let title = bounded_catalog_text(value.get("title")?.as_str()?)?;
    let artist = artist_credit(value.get("artist-credit")?);
    if artist.is_empty() {
        return None;
    }
    let provider_score = parse_number(value.get("score"))? / 100.0;
    if !(0.0..=1.0).contains(&provider_score) {
        return None;
    }
    Some(Candidate {
        id,
        title,
        artist,
        length_ms: value.get("length").and_then(Value::as_u64),
        releases: parse_releases(value.get("releases")),
        provider_score,
        local_score: 0.0,
    })
}

fn candidate_score(track: &IndexedTrack, candidate: &Candidate) -> f64 {
    let title = if track.metadata.title.trim().is_empty() {
        track.display_title.as_str()
    } else {
        track.metadata.title.as_str()
    };
    let title_score = if loose_equal(title, &candidate.title) {
        1.0
    } else {
        0.0
    };
    let artist_score = if loose_equal(&track.metadata.artist, &candidate.artist) {
        1.0
    } else {
        0.0
    };
    let album_score = if track.metadata.album.trim().is_empty() {
        0.5
    } else if candidate
        .releases
        .iter()
        .any(|release| loose_equal(&track.metadata.album, &release.title))
    {
        1.0
    } else {
        0.0
    };
    let duration_score = candidate.length_ms.map_or(0.5, |length| {
        let delta = (track.duration.as_millis() as i128 - i128::from(length)).abs();
        if delta <= 2_000 {
            1.0
        } else if delta <= 10_000 {
            0.6
        } else {
            0.0
        }
    });
    (candidate.provider_score * 0.4
        + title_score * 0.25
        + artist_score * 0.2
        + album_score * 0.05
        + duration_score * 0.1)
        .min(1.0)
}

fn parse_recording(value: &Value, expected_id: &str) -> Result<Recording, CatalogError> {
    if value.get("id").and_then(Value::as_str) != Some(expected_id) {
        return Err(CatalogError::InvalidResponse);
    }
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .and_then(bounded_catalog_text)
        .ok_or(CatalogError::InvalidResponse)?;
    let artist = artist_credit(
        value
            .get("artist-credit")
            .ok_or(CatalogError::InvalidResponse)?,
    );
    if artist.is_empty() {
        return Err(CatalogError::InvalidResponse);
    }
    Ok(Recording {
        title,
        artist,
        first_release_date: value
            .get("first-release-date")
            .and_then(Value::as_str)
            .and_then(bounded_catalog_text),
        releases: parse_releases(value.get("releases")),
    })
}

fn parse_releases(value: Option<&Value>) -> Vec<ReleaseSummary> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_RELEASES)
        .filter_map(|release| {
            Some(ReleaseSummary {
                id: parse_mbid(release.get("id")?)?,
                title: bounded_catalog_text(release.get("title")?.as_str()?)?,
                status: release
                    .get("status")
                    .and_then(Value::as_str)
                    .and_then(bounded_catalog_text),
            })
        })
        .collect()
}

fn choose_release<'a>(
    track: &IndexedTrack,
    releases: &'a [ReleaseSummary],
) -> Option<&'a ReleaseSummary> {
    if !track.metadata.album.trim().is_empty() {
        return releases
            .iter()
            .filter(|release| {
                release
                    .status
                    .as_deref()
                    .is_none_or(|status| status == "Official")
            })
            .find(|release| loose_equal(&track.metadata.album, &release.title));
    }
    let official = releases
        .iter()
        .filter(|release| {
            release
                .status
                .as_deref()
                .is_none_or(|status| status == "Official")
        })
        .collect::<Vec<_>>();
    let titles = official
        .iter()
        .map(|release| music_domain::cleanup_loose_key(&release.title))
        .collect::<BTreeSet<_>>();
    (titles.len() == 1)
        .then(|| official.first().copied())
        .flatten()
}

fn parse_release_detail(
    value: &Value,
    expected_release_id: &str,
    recording_id: &str,
) -> Result<ReleaseDetail, CatalogError> {
    if value.get("id").and_then(Value::as_str) != Some(expected_release_id) {
        return Err(CatalogError::InvalidResponse);
    }
    let mut track_no = None;
    let mut disc_no = None;
    for medium in value
        .get("media")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_MEDIA)
    {
        let medium_position = parse_u32(medium.get("position"));
        for track in medium
            .get("tracks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(MAX_TRACKS_PER_MEDIUM)
        {
            if track
                .get("recording")
                .and_then(|recording| recording.get("id"))
                .and_then(Value::as_str)
                == Some(recording_id)
            {
                track_no = parse_u32(track.get("position"));
                disc_no = medium_position;
                break;
            }
        }
        if track_no.is_some() {
            break;
        }
    }
    Ok(ReleaseDetail {
        id: expected_release_id.to_owned(),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .and_then(bounded_catalog_text)
            .ok_or(CatalogError::InvalidResponse)?,
        artist: value
            .get("artist-credit")
            .map_or_else(String::new, artist_credit),
        date: value
            .get("date")
            .and_then(Value::as_str)
            .and_then(bounded_catalog_text),
        track_no,
        disc_no,
    })
}

fn canonical_metadata(recording: &Recording, release: Option<&ReleaseDetail>) -> CanonicalMetadata {
    let date = release
        .and_then(|release| release.date.as_deref())
        .or(recording.first_release_date.as_deref());
    CanonicalMetadata {
        title: recording.title.clone(),
        artist: recording.artist.clone(),
        album_artist: release.map_or_else(String::new, |release| release.artist.clone()),
        album: release.map_or_else(String::new, |release| release.title.clone()),
        track_no: release.and_then(|release| release.track_no),
        disc_no: release.and_then(|release| release.disc_no),
        year: date
            .and_then(|date| date.get(..4))
            .and_then(|year| year.parse().ok()),
    }
}

fn metadata_operations(
    track: &IndexedTrack,
    metadata: &CanonicalMetadata,
    recording_id: &str,
) -> Vec<Value> {
    let mut operations = Vec::new();
    push_text_operation(
        &mut operations,
        track,
        "title",
        &track.metadata.title,
        &metadata.title,
        recording_id,
    );
    push_text_operation(
        &mut operations,
        track,
        "artist",
        &track.metadata.artist,
        &metadata.artist,
        recording_id,
    );
    push_text_operation(
        &mut operations,
        track,
        "album_artist",
        &track.metadata.album_artist,
        &metadata.album_artist,
        recording_id,
    );
    push_text_operation(
        &mut operations,
        track,
        "album",
        &track.metadata.album,
        &metadata.album,
        recording_id,
    );
    push_number_operation(
        &mut operations,
        track,
        "track_no",
        track.metadata.track_no,
        metadata.track_no,
        recording_id,
    );
    push_number_operation(
        &mut operations,
        track,
        "disc_no",
        track.metadata.disc_no,
        metadata.disc_no,
        recording_id,
    );
    push_number_operation(
        &mut operations,
        track,
        "year",
        track.metadata.year,
        metadata.year,
        recording_id,
    );
    operations
}

fn push_text_operation(
    operations: &mut Vec<Value>,
    track: &IndexedTrack,
    field: &str,
    old: &str,
    new: &str,
    recording_id: &str,
) {
    if new.trim().is_empty() || old == new {
        return;
    }
    operations.push(json!({
        "op_id": format!("catalog:{}:{field}:{recording_id}", track.id.get()),
        "track_id": track.id.get(),
        "kind": "tag",
        "field": field,
        "old": old,
        "new": new,
        "rules": ["catalog_identity"],
        "confidence": "low",
        "verified": true,
    }));
}

fn push_number_operation(
    operations: &mut Vec<Value>,
    track: &IndexedTrack,
    field: &str,
    old: Option<u32>,
    new: Option<u32>,
    recording_id: &str,
) {
    if new.is_none() || old == new {
        return;
    }
    operations.push(json!({
        "op_id": format!("catalog:{}:{field}:{recording_id}", track.id.get()),
        "track_id": track.id.get(),
        "kind": "tag",
        "field": field,
        "old": old,
        "new": new,
        "rules": ["catalog_identity"],
        "confidence": "low",
        "verified": true,
    }));
}

fn parse_acoustid_identity(payload: &Value) -> Result<Option<(String, f64)>, CatalogError> {
    if payload.get("status").and_then(Value::as_str) != Some("ok") {
        return Err(CatalogError::InvalidResponse);
    }
    let mut matches = payload
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|result| {
            let score = parse_number(result.get("score"))?;
            if !(0.0..=1.0).contains(&score) {
                return None;
            }
            let recordings = result.get("recordings")?.as_array()?;
            if recordings.len() != 1 {
                return None;
            }
            let id = parse_mbid(recordings.first()?.get("id")?)?;
            Some((id, score))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| right.1.total_cmp(&left.1));
    let Some(best) = matches.first() else {
        return Ok(None);
    };
    let margin = matches.get(1).map_or(1.0, |next| best.1 - next.1);
    if best.1 >= ACOUSTID_MIN_SCORE && margin >= ACOUSTID_MIN_MARGIN {
        Ok(Some(best.clone()))
    } else {
        Ok(None)
    }
}

fn map_lastfm_tags(
    payload: &Value,
    vocabulary: &TagVocabularySnapshot,
) -> Result<Vec<Value>, CatalogError> {
    if payload.get("error").is_some() {
        return Err(CatalogError::LastFm);
    }
    let mut vocabulary_terms = BTreeMap::<String, String>::new();
    for entry in vocabulary.entries() {
        vocabulary_terms.insert(entry.name.clone(), entry.name.clone());
        for alias in &entry.aliases {
            vocabulary_terms.insert(alias.clone(), entry.name.clone());
        }
    }
    let mut resolved = BTreeMap::<String, (String, u64)>::new();
    for tag in payload
        .get("toptags")
        .and_then(|tags| tags.get("tag"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_LASTFM_TAGS)
    {
        let Some(raw_name) = tag.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(count) = parse_u64(tag.get("count")) else {
            continue;
        };
        if count < LASTFM_MIN_TAG_COUNT {
            continue;
        }
        let Ok(source_tag) = normalize_manual_tag(raw_name) else {
            continue;
        };
        let Some(canonical) = vocabulary_terms.get(&source_tag) else {
            continue;
        };
        let current = resolved
            .entry(canonical.clone())
            .or_insert((source_tag.clone(), count));
        if count > current.1 {
            *current = (source_tag, count);
        }
    }
    let mut values = resolved
        .into_iter()
        .map(|(tag, (source_tag, count))| {
            json!({
                "tag": tag,
                "source_tag": source_tag,
                "count": count,
                "analyzer_id": CATALOG_TAG_ANALYZER_ID,
                "confidence": "medium",
            })
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        right
            .get("count")
            .and_then(Value::as_u64)
            .cmp(&left.get("count").and_then(Value::as_u64))
    });
    values.truncate(MAX_CATALOG_TAGS);
    Ok(values)
}

fn artist_credit(value: &Value) -> String {
    let mut rendered = String::new();
    for credit in value.as_array().into_iter().flatten() {
        let Some(name) = credit.get("name").and_then(Value::as_str) else {
            continue;
        };
        rendered.push_str(name);
        if let Some(join_phrase) = credit.get("joinphrase").and_then(Value::as_str) {
            rendered.push_str(join_phrase);
        }
        if rendered.len() > MAX_CATALOG_TEXT_BYTES {
            return String::new();
        }
    }
    rendered.trim().to_owned()
}

fn bounded_catalog_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= MAX_CATALOG_TEXT_BYTES
        && !value.chars().any(char::is_control))
    .then(|| value.to_owned())
}

fn parse_mbid(value: &Value) -> Option<String> {
    let value = value.as_str()?;
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return None;
    }
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 8 | 13 | 18 | 23) {
            if *byte != b'-' {
                return None;
            }
        } else if !byte.is_ascii_hexdigit() {
            return None;
        }
    }
    Some(value.to_ascii_lowercase())
}

fn parse_number(value: Option<&Value>) -> Option<f64> {
    value.and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    })
}

fn parse_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    })
}

fn parse_u32(value: Option<&Value>) -> Option<u32> {
    parse_u64(value).and_then(|value| u32::try_from(value).ok())
}

fn loose_equal(left: &str, right: &str) -> bool {
    let left = music_domain::cleanup_loose_key(left);
    !left.is_empty() && left == music_domain::cleanup_loose_key(right)
}

fn lucene_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn active_source_ids(acoustid_enabled: bool, lastfm_enabled: bool) -> Vec<&'static str> {
    let mut sources = vec![MUSICBRAINZ_SOURCE_ID];
    if acoustid_enabled {
        sources.push(ACOUSTID_SOURCE_ID);
    }
    if lastfm_enabled {
        sources.push(LASTFM_SOURCE_ID);
    }
    sources
}

fn cached_sources_match(result: &Map<String, Value>, expected: &[&str]) -> bool {
    result
        .get("sources")
        .and_then(Value::as_array)
        .is_some_and(|sources| {
            sources
                .iter()
                .filter_map(Value::as_str)
                .eq(expected.iter().copied())
        })
}

fn result_is_cacheable(result: &Map<String, Value>) -> bool {
    result.get("partial").and_then(Value::as_bool) != Some(true)
}

fn unmatched_result(
    track: &IndexedTrack,
    acoustid_enabled: bool,
    lastfm_enabled: bool,
) -> Map<String, Value> {
    json!({
        "schema": CLEANUP_ENRICHMENT_SCHEMA,
        "sources": active_source_ids(acoustid_enabled, lastfm_enabled),
        "track_id": track.id.get(),
        "path": track.path.as_str(),
        "status": "unmatched",
        "identity": null,
        "ops": [],
        "tag_suggestions": [],
        "notes": ["No single catalog recording met the local confidence and margin thresholds."],
    })
    .as_object()
    .cloned()
    .unwrap_or_default()
}

fn failed_result(track: &IndexedTrack, code: &str) -> Map<String, Value> {
    json!({
        "schema": CLEANUP_ENRICHMENT_SCHEMA,
        "sources": [MUSICBRAINZ_SOURCE_ID],
        "track_id": track.id.get(),
        "path": track.path.as_str(),
        "status": "failed",
        "error_code": code,
        "identity": null,
        "ops": [],
        "tag_suggestions": [],
        "notes": [format!("Catalog enrichment failed ({code}); no change was proposed.")],
    })
    .as_object()
    .cloned()
    .unwrap_or_default()
}

fn enrichment_result(
    scanned: u64,
    identified: u64,
    fingerprinted: u64,
    unmatched: u64,
    failed: u64,
    cached: u64,
    plans: Vec<Value>,
) -> Map<String, Value> {
    json!({
        "schema": CLEANUP_ENRICHMENT_SCHEMA,
        "scanned": scanned,
        "identified": identified,
        "fingerprinted": fingerprinted,
        "unmatched": unmatched,
        "failed": failed,
        "cached": cached,
        "plans": plans,
    })
    .as_object()
    .cloned()
    .unwrap_or_default()
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use music_application::assistant::TagVocabularySnapshot;
    use music_domain::{IndexedTrack, LibraryPath, TrackId, TrackMetadata};

    use super::{
        ACOUSTID_MIN_MARGIN, ACOUSTID_MIN_SCORE, Candidate, ReleaseSummary, artist_credit,
        candidate_score, map_lastfm_tags, parse_acoustid_identity, result_is_cacheable,
        select_candidate,
    };

    fn track() -> IndexedTrack {
        IndexedTrack {
            id: TrackId::new(1).ok().unwrap_or_else(|| unreachable!()),
            path: LibraryPath::parse("album/song.mp3")
                .ok()
                .unwrap_or_else(|| unreachable!()),
            metadata: TrackMetadata {
                title: "Song".to_owned(),
                artist: "Artist".to_owned(),
                album_artist: String::new(),
                album: "Album".to_owned(),
                track_no: None,
                disc_no: None,
                year: None,
                genre: String::new(),
                bpm: None,
            },
            duration: Duration::from_secs(180),
            display_title: String::new(),
            origin: String::new(),
            size_bytes: 1,
            mtime_unix_seconds: 2,
            added_at_unix_seconds: 3,
        }
    }

    #[test]
    fn metadata_identity_requires_exact_local_title_artist_and_a_clear_margin() {
        let payload = serde_json::json!({
            "recordings": [{
                "id": "00000000-0000-0000-0000-000000000001",
                "title": "Song",
                "artist-credit": [{"name": "Artist"}],
                "length": 180000,
                "score": 100,
                "releases": [{"id":"10000000-0000-0000-0000-000000000001","title":"Album","status":"Official"}]
            }]
        });
        let selected = select_candidate(&track(), &payload).ok().flatten();
        assert_eq!(
            selected.map(|candidate| candidate.id),
            Some("00000000-0000-0000-0000-000000000001".to_owned())
        );

        let ambiguous = serde_json::json!({
            "recordings": [
                {"id":"00000000-0000-0000-0000-000000000001","title":"Song","artist-credit":[{"name":"Artist"}],"length":180000,"score":100,"releases":[{"id":"10000000-0000-0000-0000-000000000001","title":"Album"}]},
                {"id":"00000000-0000-0000-0000-000000000002","title":"Song","artist-credit":[{"name":"Artist"}],"length":180000,"score":99,"releases":[{"id":"10000000-0000-0000-0000-000000000002","title":"Album"}]}
            ]
        });
        assert!(
            select_candidate(&track(), &ambiguous)
                .ok()
                .flatten()
                .is_none()
        );
    }

    #[test]
    fn acoustid_identity_requires_score_and_margin() {
        let accepted = serde_json::json!({"status":"ok","results":[
            {"score":ACOUSTID_MIN_SCORE + 0.1,"recordings":[{"id":"00000000-0000-0000-0000-000000000001"}]},
            {"score":ACOUSTID_MIN_SCORE - ACOUSTID_MIN_MARGIN,"recordings":[{"id":"00000000-0000-0000-0000-000000000002"}]}
        ]});
        assert_eq!(
            parse_acoustid_identity(&accepted)
                .ok()
                .flatten()
                .map(|item| item.0),
            Some("00000000-0000-0000-0000-000000000001".to_owned())
        );
        let ambiguous_recording = serde_json::json!({"status":"ok","results":[
            {"score":0.99,"recordings":[
                {"id":"00000000-0000-0000-0000-000000000001"},
                {"id":"00000000-0000-0000-0000-000000000002"}
            ]}
        ]});
        assert!(
            parse_acoustid_identity(&ambiguous_recording)
                .ok()
                .flatten()
                .is_none()
        );
    }

    #[test]
    fn artist_credit_preserves_provider_join_phrases() {
        let credit = serde_json::json!([
            {"name":"Lead", "joinphrase":" feat. "},
            {"name":"Guest", "joinphrase":""}
        ]);
        assert_eq!(artist_credit(&credit), "Lead feat. Guest");
    }

    #[test]
    fn partial_connector_results_are_not_cached() {
        let complete = serde_json::json!({"partial": false})
            .as_object()
            .cloned()
            .unwrap_or_default();
        let partial = serde_json::json!({"partial": true})
            .as_object()
            .cloned()
            .unwrap_or_default();
        assert!(result_is_cacheable(&complete));
        assert!(!result_is_cacheable(&partial));
    }

    #[test]
    fn lastfm_tags_only_map_exact_vocabulary_names_or_declared_aliases() {
        let vocabulary = music_application::assistant::default_vocabulary()
            .ok()
            .map(|document| TagVocabularySnapshot {
                revision: 1,
                fingerprint: "fixture".to_owned(),
                document,
            })
            .unwrap_or_else(|| unreachable!());
        let payload = serde_json::json!({"toptags":{"tag":[
            {"name":"dark","count":80},
            {"name":"invented nearby concept","count":999},
            {"name":"calm","count":1}
        ]}});
        let mapped = map_lastfm_tags(&payload, &vocabulary).unwrap_or_default();
        assert!(mapped.iter().any(|value| value["tag"] == "dark"));
        assert!(
            !mapped
                .iter()
                .any(|value| value["source_tag"] == "invented nearby concept")
        );
    }

    #[test]
    fn candidate_score_uses_album_and_duration_as_supporting_evidence() {
        let candidate = Candidate {
            id: "id".to_owned(),
            title: "Song".to_owned(),
            artist: "Artist".to_owned(),
            length_ms: Some(180_000),
            releases: vec![ReleaseSummary {
                id: "release".to_owned(),
                title: "Album".to_owned(),
                status: None,
            }],
            provider_score: 1.0,
            local_score: 0.0,
        };
        assert!(candidate_score(&track(), &candidate) > 0.95);
    }
}
