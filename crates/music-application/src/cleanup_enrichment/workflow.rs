use super::catalog::{
    AcousticCandidate, Candidate, CatalogConnector, CatalogCredentialSource, CatalogError,
    CommunityTag, Recording, ReleaseDetail,
};
use super::credits::preserve_credit;
use super::editions::resolve_editions;
use super::evidence::{ImportedTrackEvidence, LocalEvidence, retrieval_hypothesis};
use super::resolution::resolve_identity;
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
use std::time::{SystemTime, UNIX_EPOCH};

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
        if parameters.imports.len() > MAX_CLEANUP_ENRICHMENT_TRACKS
            || parameters
                .imports
                .iter()
                .any(|i| !i.valid() || !tracks.iter().any(|t| t.id.get() == i.track_id))
            || parameters
                .imports
                .iter()
                .map(|i| i.track_id)
                .collect::<BTreeSet<_>>()
                .len()
                != parameters.imports.len()
        {
            return Err(JobHandlerError::new(
                "Imported evidence must contain unique tracks within the selected scope and bounded typed fields.",
            ));
        }
        let all_tracks = self
            .services
            .cleanup
            .tracks(CleanupScope::All)
            .await
            .map_err(|_| JobHandlerError::new("folder context is unavailable"))?;
        let local_plans = music_domain::analyze_cleanup(
            &all_tracks,
            &all_tracks,
            music_domain::DEFAULT_CLEANUP_RULES,
            None,
        );
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
        self.connector
            .begin_lookup(parameters.force)
            .await
            .map_err(|e| JobHandlerError::new(e.code()))?;
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
            let mut evidence = match self.connector.local_evidence(track).await {
                Ok(evidence) => evidence,
                Err(CatalogError::StaleSource) => return Err(JobHandlerError::new("Library files changed; rescan the library before enrichment.")),
                Err(_) => LocalEvidence { observations: Vec::new(), notes: vec!["Additional embedded tags could not be read; indexed metadata remains available.".into()] },
            };
            if let Some(imported) = parameters
                .imports
                .iter()
                .find(|i| i.track_id == track.id.get())
            {
                evidence.add_import(imported);
            }
            let hypothesis = retrieval_hypothesis(
                track,
                local_plans.iter().find(|p| p.track_id == track.id),
                &evidence,
            );
            let evidence_signature = evidence
                .signature(&hypothesis)
                .map_err(JobHandlerError::new)?;
            let signature =
                cleanup_enrichment_source_signature(track).map_err(JobHandlerError::new)?;
            let folder = track
                .path
                .as_str()
                .rsplit_once('/')
                .map_or("", |(parent, _)| parent);
            let indexed_siblings = super::discovery::indexed_context(track, all_tracks.iter());
            let indexed_folder_signature =
                super::discovery::indexed_folder_signature(track, indexed_siblings.iter().copied())
                    .map_err(JobHandlerError::new)?;
            let siblings = indexed_siblings
                .iter()
                .copied()
                // Edition assignment/review still applies to one folder. Only
                // raw recording-retrieval evidence may cross disc folders.
                .filter(|t| {
                    t.path
                        .as_str()
                        .rsplit_once('/')
                        .map_or("", |(parent, _)| parent)
                        == folder
                })
                .map(|t| {
                    if t.id == track.id {
                        return hypothesis.clone();
                    }
                    let mut sibling_evidence = LocalEvidence::default();
                    if let Some(imported) =
                        parameters.imports.iter().find(|i| i.track_id == t.id.get())
                    {
                        sibling_evidence.add_import(imported);
                    }
                    retrieval_hypothesis(
                        t,
                        local_plans.iter().find(|p| p.track_id == t.id),
                        &sibling_evidence,
                    )
                })
                .collect::<Vec<_>>();
            let signatures = indexed_siblings
                .iter()
                .copied()
                .chain(siblings.iter())
                .map(cleanup_enrichment_source_signature)
                .collect::<Result<Vec<_>, _>>()
                .map_err(JobHandlerError::new)?;
            let context_signature = {
                use sha2::{Digest, Sha256};
                format!("{:x}", Sha256::digest(signatures.join(":")))
            };
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
                            && record
                                .result
                                .get("local_evidence_signature")
                                .and_then(Value::as_str)
                                == Some(&evidence_signature)
                            && record.result.get("folder_context_signature")
                                == Some(&json!(context_signature))
                            && cache_is_fresh(&record.result)
                    })
                    .map(|record| record.result)
            } else {
                None
            };
            let mut result = if let Some(result) = result {
                cached = cached.saturating_add(1);
                result
            } else {
                match self
                    .enrich_track(
                        track,
                        &hypothesis,
                        &evidence,
                        &siblings,
                        &indexed_siblings,
                        catalog,
                        vocabulary.as_ref(),
                        context.job_id(),
                    )
                    .await
                {
                    Ok(mut result) => {
                        result.insert("source_signature".into(), json!(signature));
                        result.insert("local_evidence_signature".into(), json!(evidence_signature));
                        result.insert("folder_context_signature".into(), json!(context_signature));
                        result.insert(
                            "indexed_folder_signature".into(),
                            json!(indexed_folder_signature),
                        );
                        result.insert("local_evidence".into(), json!(evidence));
                        result.insert("retrieved_at".into(), json!(now_seconds()));
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
            super::imported::append_review_import(
                &mut result,
                track,
                parameters
                    .imports
                    .iter()
                    .find(|i| i.track_id == track.id.get()),
            );
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

    #[allow(clippy::too_many_arguments)]
    async fn enrich_track(
        &self,
        track: &IndexedTrack,
        hypothesis: &IndexedTrack,
        evidence: &LocalEvidence,
        siblings: &[IndexedTrack],
        indexed_siblings: &[&IndexedTrack],
        catalog: CatalogAccess<'_>,
        vocabulary: Option<&TagVocabularySnapshot>,
        job_id: &str,
    ) -> Result<Map<String, Value>, CatalogError> {
        let mut resolution = resolve_identity(
            self.connector.as_ref(),
            track,
            hypothesis,
            evidence,
            indexed_siblings,
            if catalog.acoustid_enabled {
                catalog.acoustid_api_key
            } else {
                None
            },
        )
        .await?;
        let Some((recording_id, method, confidence, recording)) = resolution.identity.take() else {
            let mut result =
                unmatched_result(track, catalog.acoustid_enabled, catalog.lastfm_enabled);
            result.insert("partial".into(), json!(resolution.partial));
            result.insert("candidates".into(), json!(resolution.candidates));
            if !resolution.notes.is_empty() {
                result
                    .get_mut("notes")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .for_each(|notes| notes.extend(resolution.notes.iter().map(|n| json!(n))));
            }
            return Ok(result);
        };
        let credit = preserve_credit(
            self.connector.as_ref(),
            &track.metadata.artist,
            &recording.artist,
            &recording.artist_credits,
        )
        .await;
        let editions = resolve_editions(
            self.connector.as_ref(),
            track,
            hypothesis,
            &recording_id,
            &recording,
            evidence,
            siblings,
        )
        .await;
        // Proposals compare with the original indexed values, never retrieval hypotheses.
        let metadata = canonical_metadata(&recording, editions.selected.as_ref());
        let mut operations = metadata_operations(track, &metadata, &recording_id);
        operations.retain(|op| {
            !(credit.preserve && op["field"] == "artist"
                || editions.preserve_album_artist && op["field"] == "album_artist")
        });
        for op in &mut operations {
            op["evidence"] = json!({"source": "musicbrainz", "entity": if matches!(op["field"].as_str(), Some("title" | "artist" | "genre")) { "recording" } else if op["field"] == "composer" { "work" } else if op["field"] == "original_release_date" { "release_group" } else { "release" },
                "recording_id": recording_id, "release_id": editions.selected.as_ref().map(|r| &r.id), "release_group_id": editions.selected.as_ref().and_then(|r| r.release_group_id.as_deref()), "method": method});
        }
        let choices = editions.choices;
        let mut partial = resolution.partial || editions.partial || credit.partial;
        let mut tag_suggestions = Vec::new();
        let mut community_observations = json!({"source_id":"lastfm", "status":if catalog.lastfm_enabled {"unavailable"} else {"disabled"}});
        let mut notes = resolution.notes;
        notes.extend(recording.lookup_notes.iter().cloned());
        notes.extend(credit.notes);
        notes.push(format!(
            "Matched {} — {} via {} evidence (match score {:.2}; not a probability).",
            recording.artist, recording.title, method, confidence
        ));
        notes.extend(editions.notes);
        if catalog.lastfm_enabled
            && let Some(vocabulary) = vocabulary
        {
            match self
                .connector
                .community_tags_for_recording(
                    &recording_id,
                    &recording.artist,
                    &recording.title,
                    catalog
                        .lastfm_api_key
                        .ok_or(CatalogError::LastFmUnavailable)?,
                )
                .await
            {
                Ok(tags) => {
                    community_observations =
                        community_tag_observations(&tags, &recording_id, catalog.evidence_revision);
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
                Err(error) => {
                    partial = true;
                    notes.push(error.annotate(
                        "Last.fm tag evidence was unavailable; metadata proposals are still available and tags will retry next run."
                    ));
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
                "release_mbid": editions.selected.as_ref().map(|release| release.id.as_str()),
            },
            "candidates": resolution.candidates,
            "release_choices": choices,
            "recording_observations": {"first_release_date": recording.first_release_date, "genres": recording.genres, "credits": recording.credits, "composers": recording.composers},
            "ops": operations,
            "community_observations": community_observations,
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
    #[serde(default)]
    imports: Vec<ImportedTrackEvidence>,
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
pub(super) struct CanonicalMetadata {
    title: String,
    artist: String,
    album_artist: String,
    album: String,
    track_no: Option<u32>,
    disc_no: Option<u32>,
    release_date: String,
    original_release_date: String,
    composer: String,
    genre: String,
}

pub(super) fn select_candidate(
    track: &IndexedTrack,
    candidates: Vec<Candidate>,
) -> Option<(Candidate, f64)> {
    let mut unique = BTreeMap::<String, Candidate>::new();
    for candidate in candidates {
        if !(0.0..=1.0).contains(&candidate.provider_score) {
            continue;
        }
        let current = unique
            .entry(candidate.id.clone())
            .or_insert_with(|| candidate.clone());
        if candidate_score(track, &candidate) > candidate_score(track, current) {
            *current = candidate;
        }
    }
    let mut candidates = unique
        .into_values()
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
    let duration_close = track.duration.is_zero()
        || best.length_ms.is_none_or(|length| {
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

pub(super) fn candidate_score(track: &IndexedTrack, candidate: &Candidate) -> f64 {
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
    let duration_score = candidate
        .length_ms
        .filter(|_| !track.duration.is_zero())
        .map_or(0.5, |length| {
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

#[cfg(test)]
fn choose_release<'a>(
    track: &IndexedTrack,
    releases: &'a [super::catalog::ReleaseSummary],
) -> Option<&'a super::catalog::ReleaseSummary> {
    // Titles identify an album, not its edition. Deduplicate linked IDs, then
    // require a single eligible edition before proposing edition-owned fields.
    let eligible = releases
        .iter()
        .filter(|release| {
            release
                .status
                .as_deref()
                .is_none_or(|status| status == "Official")
                && (track.metadata.album.trim().is_empty()
                    || loose_equal(&track.metadata.album, &release.title))
        })
        .map(|release| (&release.id, release))
        .collect::<BTreeMap<_, _>>();
    (eligible.len() == 1)
        .then(|| eligible.values().next().copied())
        .flatten()
}

pub(super) fn canonical_metadata(
    recording: &Recording,
    release: Option<&ReleaseDetail>,
) -> CanonicalMetadata {
    // The editable year describes this release; an original recording date
    // must not silently become the date of an unidentified edition.
    let date = release.and_then(|release| release.date.as_deref());
    CanonicalMetadata {
        title: recording.title.clone(),
        artist: recording.artist.clone(),
        album_artist: release.map_or_else(String::new, |release| release.artist.clone()),
        album: release.map_or_else(String::new, |release| release.title.clone()),
        track_no: release.and_then(|release| release.track_no),
        disc_no: release.and_then(|release| release.disc_no),
        genre: recording
            .genres
            .iter()
            .fold(String::new(), |mut joined, genre| {
                let separator = if joined.is_empty() { "" } else { "; " };
                if joined.len() + separator.len() + genre.len() <= 128 {
                    joined.push_str(separator);
                    joined.push_str(genre);
                }
                joined
            }),
        release_date: date
            .filter(|date| music_domain::metadata_date_year(date).is_some())
            .unwrap_or_default()
            .to_owned(),
        original_release_date: release
            .filter(|r| r.release_group_id.is_some())
            .and_then(|r| r.original_release_date.as_deref())
            .filter(|date| music_domain::metadata_date_year(date).is_some())
            .unwrap_or_default()
            .to_owned(),
        composer: recording.composers.join("; "),
    }
}

pub(super) fn metadata_operations(
    track: &IndexedTrack,
    metadata: &CanonicalMetadata,
    recording_id: &str,
) -> Vec<Value> {
    let mut operations = Vec::new();
    push_text_operation(
        &mut operations,
        track,
        "genre",
        &track.metadata.genre,
        &metadata.genre,
        recording_id,
    );
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
    for (field, old, new) in [
        (
            "release_date",
            &track.metadata.release_date,
            &metadata.release_date,
        ),
        (
            "original_release_date",
            &track.metadata.original_release_date,
            &metadata.original_release_date,
        ),
    ] {
        // A catalog year/month cannot erase a more precise matching authored date.
        if !(music_domain::metadata_date_year(old).is_some()
            && old.len() > new.len()
            && old.starts_with(new))
        {
            push_text_operation(&mut operations, track, field, old, new, recording_id);
        }
    }
    if track.metadata.composer.trim().is_empty() {
        push_text_operation(
            &mut operations,
            track,
            "composer",
            &track.metadata.composer,
            &metadata.composer,
            recording_id,
        );
    }
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
    // Catalog typography is not evidence that an authored spelling is wrong.
    // Deliberate case repair remains a separate local cleanup rule.
    if field == "title" && title_spelling(old) == title_spelling(new) {
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

fn title_spelling(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|character| match character {
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{2010}' | '\u{2011}' => '-',
            _ => character,
        })
        .collect()
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

pub(super) fn select_acoustic_candidate(
    candidates: Vec<AcousticCandidate>,
) -> Option<(String, f64)> {
    let mut scores = BTreeMap::<String, f64>::new();
    for candidate in candidates {
        if !(0.0..=1.0).contains(&candidate.score) {
            continue;
        }
        // A fingerprint linked to several recordings supports all of them.
        // Discarding it would make weaker, unrelated evidence appear decisive.
        for id in candidate
            .recording_ids
            .into_iter()
            .filter(|id| !id.is_empty())
        {
            let score = scores.entry(id).or_default();
            *score = score.max(candidate.score);
        }
    }
    let mut matches = scores.into_iter().collect::<Vec<_>>();
    matches.sort_by(|left, right| right.1.total_cmp(&left.1));
    let best = matches.first()?;
    let margin = matches.get(1).map_or(1.0, |next| best.1 - next.1);
    (best.1 >= ACOUSTID_MIN_SCORE && margin >= ACOUSTID_MIN_MARGIN).then(|| best.clone())
}

// Preserve bounded source observations before vocabulary mapping; counts are community
// salience, never musical truth or calibrated probabilities.
fn community_tag_observations(
    tags: &[CommunityTag],
    recording_id: &str,
    evidence_revision: i64,
) -> Value {
    let mut unique = BTreeMap::<&str, u64>::new();
    for tag in tags.iter().take(MAX_LASTFM_TAGS) {
        if tag.name.trim().is_empty()
            || tag.name.chars().count() > 128
            || tag.name.chars().any(char::is_control)
        {
            continue;
        }
        unique
            .entry(&tag.name)
            .and_modify(|count| *count = (*count).max(tag.count))
            .or_insert(tag.count);
    }
    let mut observations = unique.into_iter().collect::<Vec<_>>();
    observations.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    json!({"source_id":"lastfm", "status":"available", "claim_kind":"community_tag", "entity_scope":"recording",
        "recording_mbid":recording_id, "retrieved_at":now_seconds(), "evidence_revision":evidence_revision,
        "policy_contract":super::CATALOG_EVIDENCE_POLICY_CONTRACT,
        "tags":observations.into_iter().map(|(name,count)| json!({"name":name,"count":count})).collect::<Vec<_>>()})
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

pub(super) fn loose_equal(left: &str, right: &str) -> bool {
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

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(super) fn cache_is_fresh(result: &Map<String, Value>) -> bool {
    let ttl = if result.get("status").and_then(Value::as_str) == Some("unmatched") {
        6 * 3600
    } else {
        7 * 86400
    };
    result
        .get("retrieved_at")
        .and_then(Value::as_u64)
        .is_some_and(|time| time <= now_seconds() && now_seconds().saturating_sub(time) < ttl)
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
        "notes": ["No single catalog recording met the local matching and margin thresholds."],
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
        "notes": [format!("Catalog enrichment failed ({code}); catalog metadata was not proposed.")],
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
    use crate::cleanup_enrichment::catalog::ReleaseSummary;
    use music_domain::{LibraryPath, TrackMetadata};
    use std::time::Duration;

    fn track() -> Result<IndexedTrack, Box<dyn std::error::Error>> {
        Ok(IndexedTrack {
            id: TrackId::new(1)?,
            path: LibraryPath::parse("album/song.mp3")?,
            metadata: TrackMetadata {
                release_date: String::new(),
                original_release_date: String::new(),
                composer: String::new(),
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
    fn rich_proposals_keep_edition_precision_and_authored_composers()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut track = track()?;
        track.metadata.release_date = "2024-02-29".into();
        track.metadata.year = Some(2024);
        track.metadata.composer = "Authored credit".into();
        let recording = Recording {
            composers: vec!["Catalog composer".into()],
            first_release_date: Some("1900".into()),
            ..Recording::default()
        };
        let release = ReleaseDetail {
            date: Some("2024".into()),
            original_release_date: Some("1998-07".into()),
            release_group_id: Some("group".into()),
            ..ReleaseDetail::default()
        };
        let canonical = canonical_metadata(&recording, Some(&release));
        let ops = metadata_operations(&track, &canonical, "recording");
        assert!(ops.iter().all(|op| op["field"] != "release_date"
            && op["field"] != "year"
            && op["field"] != "composer"));
        assert!(
            ops.iter()
                .any(|op| op["field"] == "original_release_date" && op["new"] == "1998-07")
        );
        track.metadata.composer.clear();
        let no_edition = canonical_metadata(&recording, None);
        assert!(no_edition.release_date.is_empty());
        assert!(no_edition.original_release_date.is_empty());
        let ops = metadata_operations(&track, &no_edition, "recording");
        assert!(
            ops.iter()
                .any(|op| op["field"] == "composer" && op["new"] == "Catalog composer")
        );
        Ok(())
    }

    #[test]
    fn catalog_titles_preserve_style_but_still_repair_substantive_differences()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut track = track()?;
        for (old, new, expected) in [
            ("Song (Alternate)", "Song (alternate)", false),
            ("Élan", "élan", false),
            ("SONG", "Song", false),
            ("Hunter's Dream", "Hunter’s Dream", false),
            ("Blood-starved Beast", "Blood‐Starved Beast", false),
            ("Long-Term", "Long‑Term", false),
            ("", "Song", true),
            ("Artist - Song", "Song", true),
            ("Song ", "Song", true),
            ("Song (live)", "Song", true),
            ("Hunters Dream", "Hunter's Dream", true),
            ("Song - Part 1", "Song – Part 1", true),
        ] {
            track.metadata.title = old.into();
            let metadata = canonical_metadata(
                &Recording {
                    title: new.into(),
                    ..Recording::default()
                },
                None,
            );
            let ops = metadata_operations(&track, &metadata, "recording");
            assert_eq!(
                ops.iter().any(|op| op["field"] == "title"),
                expected,
                "{old} -> {new}"
            );
            assert_eq!(
                metadata.title, new,
                "catalog spelling remains available as evidence"
            );
            assert_eq!(track.metadata.title, old);
        }
        Ok(())
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
    fn imported_positions_support_assignment_without_mutating_indexed_values()
    -> Result<(), Box<dyn std::error::Error>> {
        use super::super::evidence::{EvidenceField, ImportedTrackEvidence};
        let authored = track()?;
        let mut evidence = LocalEvidence::default();
        evidence.add_import(&ImportedTrackEvidence {
            track_id: 1,
            fields: BTreeMap::from([
                (EvidenceField::TrackNo, "5".into()),
                (EvidenceField::DiscNo, "2".into()),
            ]),
            ..ImportedTrackEvidence::default()
        });
        let hypothesis = retrieval_hypothesis(&authored, None, &evidence);
        assert_eq!(
            (hypothesis.metadata.track_no, hypothesis.metadata.disc_no),
            (Some(5), Some(2))
        );
        assert_eq!(
            (authored.metadata.track_no, authored.metadata.disc_no),
            (None, None)
        );
        Ok(())
    }

    #[test]
    fn corroborated_filename_positions_help_album_assignment_but_never_override_evidence()
    -> Result<(), Box<dyn std::error::Error>> {
        use super::super::{
            album::assign_album,
            catalog::ReleaseSlot,
            evidence::{EvidenceField, LocalObservation},
        };
        use music_domain::{
            CleanupConfidence, CleanupTagField, DEFAULT_CLEANUP_RULES, analyze_cleanup,
        };
        let mut first = track()?;
        first.path = LibraryPath::parse("Album/Disc 2/01 - Song.mp3")?;
        let mut second = first.clone();
        second.id = TrackId::new(2)?;
        second.path = LibraryPath::parse("Album/Disc 2/02 - Other.mp3")?;
        let tracks = vec![first.clone(), second];
        let mut plan =
            analyze_cleanup(&tracks[..1], &tracks, DEFAULT_CLEANUP_RULES, None).remove(0);
        let hypothesis = retrieval_hypothesis(&first, Some(&plan), &LocalEvidence::default());
        assert_eq!(
            (hypothesis.metadata.track_no, hypothesis.metadata.disc_no),
            (Some(1), Some(2))
        );
        assert_eq!(
            (first.metadata.track_no, first.metadata.disc_no),
            (None, None)
        );
        let release = ReleaseDetail {
            slots: [1, 2]
                .into_iter()
                .map(|disc| ReleaseSlot {
                    id: format!("disc-{disc}"),
                    recording_id: "r".into(),
                    title: "Song".into(),
                    artist: "Artist".into(),
                    length_ms: Some(180_000),
                    track_no: Some(1),
                    disc_no: Some(disc),
                })
                .collect(),
            ..ReleaseDetail::default()
        };
        assert!(
            assign_album(std::slice::from_ref(&first), &release, (first.id, "r"))
                .slots
                .is_empty()
        );
        assert_eq!(
            assign_album(&[hypothesis], &release, (first.id, "r"))
                .slots
                .get(&first.id.get())
                .map(String::as_str),
            Some("disc-2")
        );

        let mut evidence = LocalEvidence {
            observations: vec![LocalObservation {
                field: EvidenceField::TrackNo,
                value: "9".into(),
                source: "imported sidecar".into(),
            }],
            notes: vec![],
        };
        assert_eq!(
            retrieval_hypothesis(&first, Some(&plan), &evidence)
                .metadata
                .track_no,
            Some(9)
        );
        evidence.observations.push(LocalObservation {
            field: EvidenceField::TrackNo,
            value: "7".into(),
            source: "ID3".into(),
        });
        assert_eq!(
            retrieval_hypothesis(&first, Some(&plan), &evidence)
                .metadata
                .track_no,
            None
        );
        for op in &mut plan.operations {
            if matches!(
                op.field,
                Some(CleanupTagField::TrackNumber | CleanupTagField::DiscNumber)
            ) {
                op.confidence = CleanupConfidence::Low;
            }
        }
        let uncertain = retrieval_hypothesis(&first, Some(&plan), &LocalEvidence::default());
        assert_eq!(
            (uncertain.metadata.track_no, uncertain.metadata.disc_no),
            (None, None)
        );
        first.metadata.track_no = Some(4);
        assert_eq!(
            retrieval_hypothesis(&first, Some(&plan), &evidence)
                .metadata
                .track_no,
            Some(4)
        );
        Ok(())
    }

    #[test]
    fn conflicting_retrieval_hypotheses_abstain_regardless_of_order()
    -> Result<(), Box<dyn std::error::Error>> {
        use super::super::resolution::select_text_identity;
        let mut authored = track()?;
        authored.metadata.title = "01 - Song".into();
        let hypothesis = track()?;
        let mut authored_candidate = candidate();
        authored_candidate.id = "other-recording".into();
        authored_candidate.title = authored.metadata.title.clone();
        assert!(
            select_text_identity(
                &authored,
                &hypothesis,
                &[authored_candidate.clone(), candidate()]
            )
            .is_none()
        );
        assert!(
            select_text_identity(&authored, &hypothesis, &[candidate(), authored_candidate])
                .is_none()
        );
        assert_eq!(
            select_text_identity(&authored, &hypothesis, &[candidate()]).map(|v| v.0),
            Some(candidate().id)
        );
        Ok(())
    }

    #[test]
    fn unknown_duration_is_neutral_but_known_duration_conflicts_veto()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut local = track()?;
        local.duration = Duration::ZERO;
        assert!(select_candidate(&local, vec![candidate()]).is_some());
        local.duration = Duration::from_secs(30);
        assert!(select_candidate(&local, vec![candidate()]).is_none());
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
    fn stronger_ambiguous_fingerprints_veto_weaker_unique_mappings_and_repeats_merge() {
        let candidate = |score, ids: &[&str]| AcousticCandidate {
            score,
            recording_ids: ids.iter().map(|id| (*id).into()).collect(),
        };
        assert!(
            select_acoustic_candidate(vec![candidate(0.99, &["A", "B"]), candidate(0.87, &["C"])])
                .is_none()
        );
        assert_eq!(
            select_acoustic_candidate(vec![
                candidate(0.95, &["A"]),
                candidate(0.94, &["A"]),
                candidate(0.7, &["B"])
            ]),
            Some(("A".into(), 0.95))
        );
        assert!(select_acoustic_candidate(vec![candidate(f64::NAN, &["A"])]).is_none());
    }

    #[test]
    fn editions_and_dates_remain_distinct_and_repeated_candidates_do_not_compete()
    -> Result<(), Box<dyn std::error::Error>> {
        let track = track()?;
        assert!(select_candidate(&track, vec![candidate(), candidate()]).is_some());
        let mut editions = candidate().releases;
        let mut other = editions[0].clone();
        other.id = "other-edition".into();
        editions.push(other);
        assert!(choose_release(&track, &editions).is_none());
        editions.reverse();
        assert!(choose_release(&track, &editions).is_none());
        let recording = Recording {
            first_release_date: Some("1960-02-03".into()),
            ..Recording::default()
        };
        assert_eq!(canonical_metadata(&recording, None).release_date, "");
        assert_eq!(
            canonical_metadata(
                &recording,
                Some(&ReleaseDetail {
                    date: Some("2026-09-10".into()),
                    ..ReleaseDetail::default()
                })
            )
            .release_date,
            "2026-09-10"
        );
        Ok(())
    }

    #[test]
    fn partial_compilations_and_repeated_recordings_do_not_force_track_positions()
    -> Result<(), Box<dyn std::error::Error>> {
        use super::super::{album::assign_album, catalog::ReleaseSlot};
        let first = track()?;
        let mut second = first.clone();
        second.id = TrackId::new(2)?;
        second.metadata.title = "Elsewhere".into();
        second.metadata.artist = "Another Artist".into();
        let mut missing = first.clone();
        missing.id = TrackId::new(3)?;
        missing.metadata.title = "Unlisted".into();
        let slot = |id: &str, recording: &str, title: &str, artist: &str, number| ReleaseSlot {
            id: id.into(),
            recording_id: recording.into(),
            title: title.into(),
            artist: artist.into(),
            length_ms: Some(180000),
            track_no: Some(number),
            disc_no: Some(1),
        };
        let mut release = ReleaseDetail {
            slots: vec![
                slot("slot-1", "r1", "Song", "Artist", 1),
                slot("slot-2", "r2", "Elsewhere", "Another Artist", 2),
                slot("slot-4", "r4", "Missing file", "Artist", 4),
            ],
            ..ReleaseDetail::default()
        };
        let result = assign_album(
            &[first.clone(), second, missing],
            &release,
            (first.id, "r1"),
        );
        assert_eq!(result.classification, "compilation");
        assert_eq!(result.matched, 2);
        assert_eq!(result.unmatched_tracks, vec![3]);
        assert_eq!(result.unmatched_slots, vec!["slot-4"]);
        release
            .slots
            .push(slot("repeat", "r1", "Song", "Artist", 5));
        assert_eq!(
            assign_album(std::slice::from_ref(&first), &release, (first.id, "r1")).matched,
            0
        );
        let mut positioned = first.clone();
        positioned.metadata.track_no = Some(5);
        assert_eq!(
            assign_album(&[positioned], &release, (first.id, "r1"))
                .slots
                .get(&1)
                .map(String::as_str),
            Some("repeat")
        );
        Ok(())
    }

    #[test]
    fn negative_results_expire_and_future_timestamps_are_not_trusted() {
        assert!(!cache_is_fresh(&Map::from_iter([
            ("status".into(), json!("unmatched")),
            ("retrieved_at".into(), json!(now_seconds() - 21601))
        ])));
        assert!(!cache_is_fresh(&Map::from_iter([(
            "retrieved_at".into(),
            json!(now_seconds() + 60)
        )])));
        assert!(cache_is_fresh(&Map::from_iter([(
            "retrieved_at".into(),
            json!(now_seconds())
        )])));
    }

    #[test]
    fn raw_community_claims_preserve_weak_and_unmapped_tags_with_source_identity() {
        let tags = [
            ("Dream Pop", 90),
            ("unmapped musical phrase", 10),
            ("calm", 1),
            ("Dream Pop", 20),
            ("bad\nclaim", 99),
        ]
        .into_iter()
        .map(|(name, count)| CommunityTag {
            name: name.into(),
            count,
        })
        .collect::<Vec<_>>();
        let result = community_tag_observations(&tags, "recording-1", 7);
        assert_eq!(result["recording_mbid"], "recording-1");
        assert_eq!(result["evidence_revision"], 7);
        assert_eq!(result["claim_kind"], "community_tag");
        assert_eq!(
            result["tags"],
            json!([{"name":"Dream Pop","count":90},{"name":"unmapped musical phrase","count":10},{"name":"calm","count":1}])
        );
        let many = (0..500)
            .map(|i| CommunityTag {
                name: format!("tag-{i}"),
                count: 1,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            community_tag_observations(&many, "recording-1", 7)["tags"]
                .as_array()
                .map(Vec::len),
            Some(50)
        );
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
