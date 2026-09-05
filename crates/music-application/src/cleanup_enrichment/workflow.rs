use super::catalog::{
    AcousticCandidate, Candidate, CatalogConnector, CatalogCredentialSource, CatalogError,
    CommunityTag, Recording, ReleaseDetail, ReleaseSummary,
};
use super::{
    CLEANUP_ENRICHMENT_JOB_KIND, CLEANUP_ENRICHMENT_SCHEMA, CleanupEnrichmentRecord,
    CleanupEnrichmentRepository, MAX_CLEANUP_ENRICHMENT_TRACKS,
    cleanup_enrichment_source_signature,
};
use crate::assistant::{
    AnalysisWrite, AssistantService, CATALOG_TAG_ANALYZER_ID, Confidence, LocalAnalysisRepository,
    TagVocabularySnapshot, catalog_tag_source_signature, normalize_manual_tag,
};
use crate::cleanup::{CleanupScope, CleanupService};
use crate::cleanup_sources::{
    ACOUSTID_SOURCE_ID, CleanupSourceService, LASTFM_SOURCE_ID, MUSICBRAINZ_SOURCE_ID,
};
use crate::jobs::{
    JobCheckpointPolicy, JobDefinition, JobExecutionContext, JobHandler, JobHandlerError,
    JobHandlerFuture, JobLane, JobProgress,
};
use music_domain::{IndexedTrack, TrackId};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const ACOUSTID_MIN_SCORE: f64 = 0.85;
const ACOUSTID_MIN_MARGIN: f64 = 0.10;
const METADATA_MIN_SCORE: f64 = 0.86;
const METADATA_MIN_MARGIN: f64 = 0.05;
const LASTFM_MIN_TAG_COUNT: u64 = 10;
const MAX_LASTFM_TAGS: usize = 50;
const MAX_CATALOG_TAGS: usize = 8;

#[derive(Debug)]
pub struct CleanupEnrichmentJobHandler {
    services: CleanupEnrichmentServices,
    connector: Arc<dyn CatalogConnector>,
}

#[derive(Debug)]
pub struct CleanupEnrichmentServices {
    pub cleanup: Arc<CleanupService>,
    pub cache: Arc<dyn CleanupEnrichmentRepository>,
    pub analyses: Arc<dyn LocalAnalysisRepository>,
    pub assistant: Arc<AssistantService>,
    pub sources: Arc<CleanupSourceService>,
}

#[derive(Clone, Copy)]
struct CatalogAccess<'a> {
    evidence_revision: i64,
    acoustid_enabled: bool,
    acoustid_api_key: Option<&'a str>,
    lastfm_enabled: bool,
    lastfm_api_key: Option<&'a str>,
}

impl CleanupEnrichmentJobHandler {
    #[must_use]
    pub fn new(services: CleanupEnrichmentServices, connector: Arc<dyn CatalogConnector>) -> Self {
        Self {
            services,
            connector,
        }
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
            self.services.cleanup.tracks(scope).await.map_err(|_| {
                JobHandlerError::new("cleanup enrichment scope could not be loaded")
            })?;
        if tracks.len() > MAX_CLEANUP_ENRICHMENT_TRACKS {
            return Err(JobHandlerError::new(format!(
                "cleanup enrichment is limited to {MAX_CLEANUP_ENRICHMENT_TRACKS} tracks per run; choose a smaller folder"
            )));
        }
        let _source_lease = self.services.sources.execution_lease().await;
        let source_states = self
            .services
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
        let acoustid_saved = if acoustid_enabled {
            self.services
                .sources
                .saved_credential(ACOUSTID_SOURCE_ID)
                .await
                .map_err(|_| JobHandlerError::new("AcoustID credential is unavailable"))?
        } else {
            None
        };
        let lastfm_saved = if lastfm_enabled {
            self.services
                .sources
                .saved_credential(LASTFM_SOURCE_ID)
                .await
                .map_err(|_| JobHandlerError::new("Last.fm credential is unavailable"))?
        } else {
            None
        };
        let acoustid_api_key = acoustid_saved
            .as_ref()
            .map(|secret| secret.expose_secret())
            .or_else(|| {
                self.connector
                    .runtime_credential(CatalogCredentialSource::AcoustId)
            });
        let lastfm_api_key = lastfm_saved
            .as_ref()
            .map(|secret| secret.expose_secret())
            .or_else(|| {
                self.connector
                    .runtime_credential(CatalogCredentialSource::LastFm)
            });
        if acoustid_enabled && acoustid_api_key.is_none() {
            return Err(JobHandlerError::new("AcoustID credential is unavailable"));
        }
        if lastfm_enabled && lastfm_api_key.is_none() {
            return Err(JobHandlerError::new("Last.fm credential is unavailable"));
        }
        // Capture before vocabulary loading: an edit during snapshot loading
        // makes this revision stale and fails the run before any request.
        let evidence_revision = self
            .services
            .cache
            .catalog_evidence_revision()
            .await
            .map_err(|_| JobHandlerError::new("catalog evidence revision is unavailable"))?;
        let catalog = CatalogAccess {
            evidence_revision,
            acoustid_enabled,
            acoustid_api_key,
            lastfm_enabled,
            lastfm_api_key,
        };
        let active_sources = active_source_ids(acoustid_enabled, lastfm_enabled);
        let vocabulary = if lastfm_enabled {
            Some(
                self.services
                    .assistant
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
            if self
                .services
                .cache
                .catalog_evidence_revision()
                .await
                .map_err(|_| JobHandlerError::new("catalog evidence revision is unavailable"))?
                != evidence_revision
            {
                return Err(JobHandlerError::new(
                    "Catalog settings or vocabulary changed; start a fresh lookup.",
                ));
            }
            let signature =
                cleanup_enrichment_source_signature(track).map_err(JobHandlerError::new)?;
            let result = if !parameters.force {
                self.services
                    .cache
                    .cleanup_enrichment(track.id)
                    .await
                    .map_err(|_| JobHandlerError::new("cleanup enrichment cache is unavailable"))?
                    .filter(|record| {
                        record.source_signature == signature
                            && record.evidence_revision == evidence_revision
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
                    .enrich_track(track, catalog, vocabulary.as_ref(), context.job_id())
                    .await
                {
                    Ok(mut result) => {
                        result.insert("evidence_revision".to_owned(), json!(evidence_revision));
                        result.insert(
                            "vocabulary_fingerprint".to_owned(),
                            json!(vocabulary.as_ref().map(|v| &v.fingerprint)),
                        );
                        if result_is_cacheable(&result) {
                            let record = CleanupEnrichmentRecord {
                                track_id: track.id,
                                evidence_revision,
                                source_signature: signature,
                                result: result.clone(),
                            };
                            let stored = self
                                .services
                                .cache
                                .store_cleanup_enrichment(&record)
                                .await
                                .map_err(|_| {
                                    JobHandlerError::new(
                                        "cleanup enrichment cache could not be updated",
                                    )
                                })?;
                            if !stored {
                                return Err(JobHandlerError::new(
                                    "Catalog evidence became stale; start a fresh lookup.",
                                ));
                            }
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
        catalog: CatalogAccess<'_>,
        vocabulary: Option<&TagVocabularySnapshot>,
        job_id: &str,
    ) -> Result<Map<String, Value>, CatalogError> {
        let candidates = self.connector.search_metadata(track).await?;
        let metadata_match = select_candidate(track, candidates);
        let (recording_id, method, confidence) = if let Some((candidate, score)) = metadata_match {
            (candidate.id, "metadata", score)
        } else if catalog.acoustid_enabled {
            let recording_id = self
                .connector
                .fingerprint_candidates(
                    track,
                    catalog
                        .acoustid_api_key
                        .ok_or(CatalogError::AcoustIdUnavailable)?,
                )
                .await?;
            let Some((recording_id, score)) = select_acoustic_candidate(recording_id) else {
                return Ok(unmatched_result(
                    track,
                    catalog.acoustid_enabled,
                    catalog.lastfm_enabled,
                ));
            };
            (recording_id, "fingerprint", score)
        } else {
            return Ok(unmatched_result(
                track,
                catalog.acoustid_enabled,
                catalog.lastfm_enabled,
            ));
        };

        let recording = self.connector.recording(&recording_id).await?;
        let release = choose_release(track, &recording.releases);
        let mut partial = false;
        let release_detail = match release {
            Some(release) => match self.connector.release(&release.id, &recording_id).await {
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
        if catalog.lastfm_enabled
            && let Some(vocabulary) = vocabulary
        {
            match self
                .connector
                .community_tags(
                    &recording.artist,
                    &recording.title,
                    catalog
                        .lastfm_api_key
                        .ok_or(CatalogError::LastFmUnavailable)?,
                )
                .await
            {
                Ok(tags) => {
                    let mut suggestions = map_community_tags(&tags, vocabulary);
                    let source_signature =
                        catalog_tag_source_signature(track, catalog.evidence_revision)
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
                        .store_catalog_tags(
                            track,
                            &recording_id,
                            &suggestions,
                            job_id,
                            catalog.evidence_revision,
                            vocabulary,
                        )
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
            "sources": active_source_ids(catalog.acoustid_enabled, catalog.lastfm_enabled),
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

    async fn store_catalog_tags(
        &self,
        track: &IndexedTrack,
        recording_id: &str,
        suggestions: &[Value],
        job_id: &str,
        evidence_revision: i64,
        vocabulary: &TagVocabularySnapshot,
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
        let source_signature = catalog_tag_source_signature(track, evidence_revision)
            .map_err(|_| CatalogError::InvalidResponse)?;
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
                "policy_contract": super::CATALOG_EVIDENCE_POLICY_CONTRACT,
                "recording_mbid": recording_id,
                "evidence_revision": evidence_revision,
                "vocabulary_fingerprint": vocabulary.fingerprint,
            })
            .as_object()
            .cloned()
            .ok_or(CatalogError::InvalidResponse)?,
            confidence: Confidence::Medium,
        };
        let stored = self
            .services
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

fn select_candidate(track: &IndexedTrack, candidates: Vec<Candidate>) -> Option<(Candidate, f64)> {
    let mut candidates = candidates
        .into_iter()
        .map(|candidate| {
            let score = candidate_score(track, &candidate);
            (candidate, score)
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.1.total_cmp(&left.1));
    let (best, score) = candidates.first()?;
    let margin = candidates.get(1).map_or(1.0, |next| score - next.1);
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
    if *score >= METADATA_MIN_SCORE
        && margin >= METADATA_MIN_MARGIN
        && exact_title
        && exact_artist
        && duration_close
    {
        Some((best.clone(), *score))
    } else {
        None
    }
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

fn select_acoustic_candidate(candidates: Vec<AcousticCandidate>) -> Option<(String, f64)> {
    let mut matches = candidates
        .into_iter()
        .filter_map(|candidate| {
            if !(0.0..=1.0).contains(&candidate.score) || candidate.recording_ids.len() != 1 {
                return None;
            }
            Some((candidate.recording_ids.into_iter().next()?, candidate.score))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| right.1.total_cmp(&left.1));
    let best = matches.first()?;
    let margin = matches.get(1).map_or(1.0, |next| best.1 - next.1);
    (best.1 >= ACOUSTID_MIN_SCORE && margin >= ACOUSTID_MIN_MARGIN).then(|| best.clone())
}

fn map_community_tags(tags: &[CommunityTag], vocabulary: &TagVocabularySnapshot) -> Vec<Value> {
    let mut vocabulary_terms = BTreeMap::<String, String>::new();
    for entry in vocabulary.entries() {
        vocabulary_terms.insert(entry.name.clone(), entry.name.clone());
        for alias in &entry.aliases {
            vocabulary_terms.insert(alias.clone(), entry.name.clone());
        }
    }
    let mut resolved = BTreeMap::<String, (String, u64)>::new();
    for tag in tags.iter().take(MAX_LASTFM_TAGS) {
        let raw_name = &tag.name;
        let count = tag.count;
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
    values
}

fn loose_equal(left: &str, right: &str) -> bool {
    let left = music_domain::cleanup_loose_key(left);
    !left.is_empty() && left == music_domain::cleanup_loose_key(right)
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
    use super::*;
    use music_domain::{LibraryPath, TrackMetadata};
    use std::time::Duration;

    fn track() -> Result<IndexedTrack, Box<dyn std::error::Error>> {
        Ok(IndexedTrack {
            id: TrackId::new(1)?,
            path: LibraryPath::parse("album/song.mp3")?,
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
        })
    }

    fn candidate() -> Candidate {
        Candidate {
            id: "00000000-0000-0000-0000-000000000001".to_owned(),
            title: "Song".to_owned(),
            artist: "Artist".to_owned(),
            length_ms: Some(180_000),
            releases: vec![ReleaseSummary {
                id: "release".to_owned(),
                title: "Album".to_owned(),
                status: None,
            }],
            provider_score: 1.0,
        }
    }

    #[test]
    fn metadata_identity_requires_exact_local_title_artist_and_a_clear_margin()
    -> Result<(), Box<dyn std::error::Error>> {
        assert!(select_candidate(&track()?, vec![candidate()]).is_some());
        let mut other = candidate();
        other.id.push('2');
        other.provider_score = 0.99;
        assert!(select_candidate(&track()?, vec![candidate(), other]).is_none());
        let mut wrong = candidate();
        wrong.title = "Different Song".to_owned();
        assert!(select_candidate(&track()?, vec![wrong]).is_none());
        Ok(())
    }

    #[test]
    fn acoustid_identity_requires_score_margin_and_one_recording() {
        let candidate = |score, ids: &[&str]| AcousticCandidate {
            score,
            recording_ids: ids.iter().map(|id| (*id).to_owned()).collect(),
        };
        assert_eq!(
            select_acoustic_candidate(vec![candidate(0.95, &["one"]), candidate(0.70, &["two"])]),
            Some(("one".to_owned(), 0.95))
        );
        assert!(select_acoustic_candidate(vec![candidate(0.99, &["one", "two"])]).is_none());
        assert!(
            select_acoustic_candidate(vec![candidate(0.95, &["one"]), candidate(0.90, &["two"])])
                .is_none()
        );
        assert!(select_acoustic_candidate(vec![candidate(0.84, &["one"])]).is_none());
    }

    #[test]
    fn partial_connector_results_are_not_cached() {
        let complete = Map::from_iter([("partial".to_owned(), json!(false))]);
        let partial = Map::from_iter([("partial".to_owned(), json!(true))]);
        assert!(result_is_cacheable(&complete));
        assert!(!result_is_cacheable(&partial));
    }

    #[test]
    fn community_tags_only_map_exact_names_or_declared_aliases()
    -> Result<(), Box<dyn std::error::Error>> {
        let vocabulary = crate::assistant::default_vocabulary_snapshot()?;
        let tags = [("dark", 80), ("invented nearby concept", 999), ("calm", 1)]
            .into_iter()
            .map(|(name, count)| CommunityTag {
                name: name.to_owned(),
                count,
            })
            .collect::<Vec<_>>();
        let mapped = map_community_tags(&tags, &vocabulary);
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0]["tag"], "dark");
        Ok(())
    }

    #[test]
    fn candidate_score_uses_album_and_duration_as_supporting_evidence()
    -> Result<(), Box<dyn std::error::Error>> {
        let candidate = candidate();
        assert!(candidate_score(&track()?, &candidate) > 0.95);
        let mut wrong_album = track()?;
        wrong_album.metadata.album = "Other Album".to_owned();
        assert!(choose_release(&wrong_album, &candidate.releases).is_none());
        Ok(())
    }
}
