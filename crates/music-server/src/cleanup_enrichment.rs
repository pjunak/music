use crate::cleanup::MusicBrainzNameLookup;
use crate::cleanup_enrichment_cache::ObservationCache;
use futures_util::TryStreamExt;
use music_application::assistant::{AssistantService, LocalAnalysisRepository};
use music_application::cleanup::CleanupService;
use music_application::cleanup_enrichment::catalog::{
    AcousticCandidate, Artist, Candidate, CatalogConnector, CatalogCredentialSource, CatalogError,
    CatalogFuture, CommunityTag, Recording, ReleaseDetail, ReleaseSlot, ReleaseSummary,
};
use music_application::cleanup_enrichment::evidence::{
    EvidenceField, LocalEvidence, normalized_value,
};
use music_application::cleanup_enrichment::{
    CleanupEnrichmentJobHandler, CleanupEnrichmentRepository,
    CleanupEnrichmentServices as ApplicationServices,
};
use music_application::cleanup_sources::CleanupSourceService;
use music_domain::IndexedTrack;
use music_media::LibraryRoot;
use music_storage::SecretString;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Mutex;

#[cfg(test)]
#[path = "cleanup_enrichment_alias_tests.rs"]
mod alias_tests;

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const FINGERPRINT_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_CATALOG_TEXT_BYTES: usize = 512;
const MAX_RELEASES: usize = 100;
const MAX_MEDIA: usize = 100;
const MAX_TRACKS_PER_MEDIUM: usize = 1_000;
const ACOUSTID_ENDPOINT: &str = "https://api.acoustid.org/v2/lookup";
const LASTFM_ENDPOINT: &str = "https://ws.audioscrobbler.com/2.0/";
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

pub(crate) struct CleanupEnrichmentServices {
    pub(crate) cleanup: Arc<CleanupService>,
    pub(crate) cache: Arc<dyn CleanupEnrichmentRepository>,
    pub(crate) analyses: Arc<dyn LocalAnalysisRepository>,
    pub(crate) assistant: Arc<AssistantService>,
    pub(crate) sources: Arc<CleanupSourceService>,
    pub(crate) musicbrainz: Arc<MusicBrainzNameLookup>,
}

pub(crate) fn cleanup_enrichment_handler(
    services: CleanupEnrichmentServices,
    library_root: LibraryRoot,
    config: CleanupConnectorConfig,
) -> Result<CleanupEnrichmentJobHandler, reqwest::Error> {
    let http = Client::builder()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("music-dnd-orchestrator/0.1 (https://github.com/pjunak/music)")
        .build()?;
    let connector = Arc::new(HttpCatalogConnector {
        musicbrainz: services.musicbrainz,
        library_root,
        config,
        http,
        entities: Mutex::new(ObservationCache::default()),
        fingerprints: Mutex::new(ObservationCache::default()),
    });
    Ok(CleanupEnrichmentJobHandler::new(
        ApplicationServices {
            cleanup: services.cleanup,
            cache: services.cache,
            analyses: services.analyses,
            assistant: services.assistant,
            sources: services.sources,
        },
        connector,
    ))
}

#[derive(Debug)]
struct HttpCatalogConnector {
    musicbrainz: Arc<MusicBrainzNameLookup>,
    library_root: LibraryRoot,
    config: CleanupConnectorConfig,
    http: Client,
    entities: Mutex<ObservationCache<Value>>,
    fingerprints: Mutex<ObservationCache<FingerprintOutput>>,
}

impl HttpCatalogConnector {
    async fn entity_json(
        &self,
        resource: &str,
        query: &[(&str, String)],
    ) -> Result<Value, CatalogError> {
        let key = format!(
            "{resource}:{}",
            serde_json::to_string(query).map_err(|_| CatalogError::InvalidResponse)?
        );
        if let Some(value) = self.entities.lock().await.get(&key) {
            return Ok(value);
        }
        let value = self
            .musicbrainz
            .fetch_json(resource, query)
            .await
            .map_err(|_| CatalogError::MusicBrainz)?;
        // An invalid search response must be retried, not retained for the cache TTL.
        match resource {
            "recording" => {
                parse_candidates(&value)?;
            }
            "artist" => {
                parse_artist_candidates(&value)?;
            }
            _ => {
                if let Some(id) = resource.strip_prefix("artist/") {
                    parse_artist(&value, Some(id))?;
                }
            }
        }
        self.entities.lock().await.insert(key, value.clone());
        Ok(value)
    }

    async fn search_metadata(&self, track: &IndexedTrack) -> Result<Vec<Candidate>, CatalogError> {
        let title = if track.metadata.title.trim().is_empty() {
            track.display_title.trim()
        } else {
            track.metadata.title.trim()
        };
        let artist = track.metadata.artist.trim();
        if title.is_empty() || artist.is_empty() {
            return Ok(Vec::new());
        }
        let query = metadata_query(title, artist, track.duration);
        let payload = self
            .entity_json(
                "recording",
                &[
                    ("query", query),
                    ("fmt", "json".to_owned()),
                    ("limit", "25".to_owned()),
                ],
            )
            .await?;
        parse_candidates(&payload)
    }

    async fn recording(&self, recording_id: &str) -> Result<Recording, CatalogError> {
        let payload = self
            .entity_json(
                &format!("recording/{recording_id}"),
                &[
                    ("fmt", "json".to_owned()),
                    (
                        "inc",
                        "artist-credits+releases+release-groups+genres+artist-rels+work-rels+work-level-rels"
                            .to_owned(),
                    ),
                ],
            )
            .await
            .map_err(|_| CatalogError::MusicBrainz)?;
        let mut recording = parse_recording(&payload, recording_id)?;
        // Linked release lists are capped by MusicBrainz; browse explicitly.
        let mut releases = recording
            .releases
            .iter()
            .cloned()
            .map(|r| (r.id.clone(), r))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut offset = 0;
        recording.releases_complete = false;
        while offset < MAX_RELEASES {
            let Ok(page) = self
                .entity_json(
                    "release",
                    &[
                        ("recording", recording_id.to_owned()),
                        ("fmt", "json".into()),
                        ("limit", (MAX_RELEASES - offset).min(25).to_string()),
                        ("offset", offset.to_string()),
                    ],
                )
                .await
            else {
                break;
            };
            let Some(raw) = page.get("releases").and_then(Value::as_array) else {
                break;
            };
            for release in parse_releases(Some(&Value::Array(raw.clone()))) {
                releases.insert(release.id.clone(), release);
            }
            offset += raw.len();
            let Some(count) = page.get("release-count").and_then(Value::as_u64) else {
                break;
            };
            if offset as u64 >= count {
                recording.releases_complete = true;
                break;
            }
            if raw.is_empty() {
                break;
            }
        }
        recording.releases = releases.into_values().collect();
        Ok(recording)
    }

    async fn release(
        &self,
        release_id: &str,
        recording_id: &str,
    ) -> Result<ReleaseDetail, CatalogError> {
        let payload = self
            .entity_json(
                &format!("release/{release_id}"),
                &[
                    ("fmt", "json".to_owned()),
                    (
                        "inc",
                        "recordings+artist-credits+release-groups+media+labels".to_owned(),
                    ),
                ],
            )
            .await
            .map_err(|_| CatalogError::MusicBrainz)?;
        parse_release_detail(&payload, release_id, recording_id)
    }

    async fn fingerprint_candidates(
        &self,
        track: &IndexedTrack,
        api_key: &str,
    ) -> Result<Vec<AcousticCandidate>, CatalogError> {
        let absolute = self
            .library_root
            .resolve_existing(&track.path)
            .map_err(|_| CatalogError::Fingerprint)?;
        let metadata = tokio::fs::metadata(&absolute)
            .await
            .map_err(|_| CatalogError::Fingerprint)?;
        let modified = metadata.modified().map_err(|_| CatalogError::Fingerprint)?;
        let key = format!("{}:{}:{modified:?}", track.path.as_str(), metadata.len());
        let cached = self.fingerprints.lock().await.get(&key);
        let fingerprint = if let Some(fingerprint) = cached {
            fingerprint
        } else {
            let command = Command::new(&self.config.fpcalc_path)
                .arg("-json")
                .arg("-length")
                .arg("120")
                .arg("--")
                .arg(&absolute)
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
            if fingerprint.fingerprint.is_empty()
                || !(1.0..=86_400.0).contains(&fingerprint.duration)
            {
                return Err(CatalogError::Fingerprint);
            }
            let after = tokio::fs::metadata(&absolute)
                .await
                .map_err(|_| CatalogError::Fingerprint)?;
            if after.len() != metadata.len() || after.modified().ok() != Some(modified) {
                return Err(CatalogError::Fingerprint);
            }
            self.fingerprints
                .lock()
                .await
                .insert(key, fingerprint.clone());
            fingerprint
        };
        let response = self
            .http
            .post(ACOUSTID_ENDPOINT)
            .form(&[
                ("client", api_key.to_owned()),
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
        parse_acoustic_candidates(&payload)
    }

    async fn community_tags(
        &self,
        artist: &str,
        title: &str,
        api_key: &str,
        recording_id: Option<&str>,
    ) -> Result<Vec<CommunityTag>, CatalogError> {
        let mut form = vec![
            ("method", "track.gettoptags"),
            ("api_key", api_key),
            ("autocorrect", "0"),
            ("format", "json"),
        ];
        if let Some(id) = recording_id {
            form.push(("mbid", id));
        } else {
            form.extend([("artist", artist), ("track", title)]);
        }
        let response = self
            .http
            .post(LASTFM_ENDPOINT)
            .form(&form)
            .send()
            .await
            .map_err(|_| CatalogError::LastFm)?
            .error_for_status()
            .map_err(|_| CatalogError::LastFm)?;
        let payload = bounded_json(response)
            .await
            .map_err(|_| CatalogError::LastFm)?;
        parse_community_tags(&payload)
    }
}

impl CatalogConnector for HttpCatalogConnector {
    fn begin_lookup(&self, refresh: bool) -> CatalogFuture<'_, ()> {
        Box::pin(async move {
            if refresh {
                self.entities.lock().await.clear();
                self.fingerprints.lock().await.clear();
            }
            Ok(())
        })
    }
    fn community_tags_for_recording<'a>(
        &'a self,
        recording_id: &'a str,
        artist: &'a str,
        title: &'a str,
        api_key: &'a str,
    ) -> CatalogFuture<'a, Vec<CommunityTag>> {
        Box::pin(HttpCatalogConnector::community_tags(
            self,
            artist,
            title,
            api_key,
            Some(recording_id),
        ))
    }

    fn local_evidence<'a>(&'a self, track: &'a IndexedTrack) -> CatalogFuture<'a, LocalEvidence> {
        Box::pin(async move {
            let path = self
                .library_root
                .resolve_existing(&track.path)
                .map_err(|_| CatalogError::InvalidResponse)?;
            let before = tokio::fs::metadata(&path)
                .await
                .map_err(|_| CatalogError::StaleSource)?;
            let modified = before.modified().map_err(|_| CatalogError::StaleSource)?;
            let seconds = modified
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| CatalogError::StaleSource)?
                .as_secs();
            if before.len() != track.size_bytes
                || i64::try_from(seconds).ok() != Some(track.mtime_unix_seconds)
            {
                return Err(CatalogError::StaleSource);
            }
            let checked_path = path.clone();
            let evidence =
                tokio::task::spawn_blocking(move || music_media::read_cleanup_evidence(&path))
                    .await
                    .map_err(|_| CatalogError::InvalidResponse)?
                    .map_err(|_| CatalogError::InvalidResponse)?;
            let after = tokio::fs::metadata(checked_path)
                .await
                .map_err(|_| CatalogError::StaleSource)?;
            if after.len() != before.len() || after.modified().ok() != Some(modified) {
                return Err(CatalogError::StaleSource);
            }
            Ok(evidence)
        })
    }
    fn search_isrc<'a>(&'a self, isrc: &'a str) -> CatalogFuture<'a, Vec<Candidate>> {
        Box::pin(async move {
            let isrc =
                normalized_value(EvidenceField::Isrc, isrc).ok_or(CatalogError::InvalidResponse)?;
            let payload = self
                .entity_json(
                    "recording",
                    &[
                        ("query", format!("isrc:{}", lucene_quote(&isrc))),
                        ("fmt", "json".into()),
                        ("limit", "25".into()),
                    ],
                )
                .await?;
            parse_candidates(&payload)
        })
    }

    fn runtime_credential(&self, source: CatalogCredentialSource) -> Option<&str> {
        match source {
            CatalogCredentialSource::AcoustId => self.config.acoustid_api_key.as_ref(),
            CatalogCredentialSource::LastFm => self.config.lastfm_api_key.as_ref(),
        }
        .map(SecretString::expose_secret)
    }
    fn search_album_metadata<'a>(
        &'a self,
        track: &'a IndexedTrack,
        release_id: Option<&'a str>,
    ) -> CatalogFuture<'a, Vec<Candidate>> {
        Box::pin(async move {
            let title = if track.metadata.title.trim().is_empty() {
                track.display_title.trim()
            } else {
                track.metadata.title.trim()
            };
            let Some(query) =
                album_metadata_query(title, &track.metadata.album, release_id, track.duration)?
            else {
                return Ok(Vec::new());
            };
            let payload = self
                .entity_json(
                    "recording",
                    &[
                        ("query", query),
                        ("fmt", "json".into()),
                        ("limit", "25".into()),
                    ],
                )
                .await?;
            parse_candidates(&payload)
        })
    }
    fn search_metadata<'a>(&'a self, track: &'a IndexedTrack) -> CatalogFuture<'a, Vec<Candidate>> {
        Box::pin(HttpCatalogConnector::search_metadata(self, track))
    }

    fn search_artists<'a>(&'a self, name: &'a str) -> CatalogFuture<'a, Vec<Artist>> {
        Box::pin(async move {
            let Some(query) = artist_name_query(name) else {
                return Ok(Vec::new());
            };
            let payload = self
                .entity_json(
                    "artist",
                    &[
                        ("query", query),
                        ("fmt", "json".into()),
                        ("limit", "10".into()),
                    ],
                )
                .await?;
            parse_artist_candidates(&payload)
        })
    }

    fn artist<'a>(&'a self, artist_id: &'a str) -> CatalogFuture<'a, Artist> {
        Box::pin(async move {
            let id = catalog_artist_id(artist_id).ok_or(CatalogError::InvalidResponse)?;
            let payload = self
                .entity_json(
                    &format!("artist/{id}"),
                    &[("fmt", "json".into()), ("inc", "aliases".into())],
                )
                .await?;
            parse_artist(&payload, Some(&id))
        })
    }

    fn search_artist_recordings<'a>(
        &'a self,
        track: &'a IndexedTrack,
        artist_id: &'a str,
    ) -> CatalogFuture<'a, Vec<Candidate>> {
        Box::pin(async move {
            let title = if track.metadata.title.trim().is_empty() {
                &track.display_title
            } else {
                &track.metadata.title
            };
            let Some(query) = artist_recording_query(title, artist_id, track.duration)? else {
                return Ok(Vec::new());
            };
            let payload = self
                .entity_json(
                    "recording",
                    &[
                        ("query", query),
                        ("fmt", "json".into()),
                        ("limit", "25".into()),
                    ],
                )
                .await?;
            parse_artist_recordings(&payload, artist_id)
        })
    }

    fn recording<'a>(&'a self, recording_id: &'a str) -> CatalogFuture<'a, Recording> {
        Box::pin(HttpCatalogConnector::recording(self, recording_id))
    }

    fn release<'a>(
        &'a self,
        release_id: &'a str,
        recording_id: &'a str,
    ) -> CatalogFuture<'a, ReleaseDetail> {
        Box::pin(HttpCatalogConnector::release(
            self,
            release_id,
            recording_id,
        ))
    }

    fn fingerprint_candidates<'a>(
        &'a self,
        track: &'a IndexedTrack,
        api_key: &'a str,
    ) -> CatalogFuture<'a, Vec<AcousticCandidate>> {
        Box::pin(HttpCatalogConnector::fingerprint_candidates(
            self, track, api_key,
        ))
    }

    fn community_tags<'a>(
        &'a self,
        artist: &'a str,
        title: &'a str,
        api_key: &'a str,
    ) -> CatalogFuture<'a, Vec<CommunityTag>> {
        Box::pin(HttpCatalogConnector::community_tags(
            self, artist, title, api_key, None,
        ))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FingerprintOutput {
    duration: f64,
    fingerprint: String,
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

fn metadata_query(title: &str, artist: &str, duration: Duration) -> String {
    with_duration_query(
        format!(
            "recording:{} AND artist:{}",
            lucene_quote(title),
            lucene_quote(artist)
        ),
        duration,
    )
}

fn catalog_artist_id(value: &str) -> Option<String> {
    let id = uuid::Uuid::parse_str(value).ok()?;
    (!id.is_nil()).then(|| id.hyphenated().to_string())
}

fn artist_name_query(name: &str) -> Option<String> {
    let name = bounded_catalog_text(name)?;
    let quoted = lucene_quote(&name);
    Some(format!(
        "artist:{quoted} OR alias:{quoted} OR sortname:{quoted}"
    ))
}

fn artist_recording_query(
    title: &str,
    artist_id: &str,
    duration: Duration,
) -> Result<Option<String>, CatalogError> {
    let Some(title) = bounded_catalog_text(title) else {
        return Ok(None);
    };
    let id = catalog_artist_id(artist_id).ok_or(CatalogError::InvalidResponse)?;
    Ok(Some(with_duration_query(
        format!(
            "recording:{} AND arid:{}",
            lucene_quote(&title),
            lucene_quote(&id)
        ),
        duration,
    )))
}

fn parse_artist_candidates(payload: &Value) -> Result<Vec<Artist>, CatalogError> {
    let artists = payload
        .get("artists")
        .and_then(Value::as_array)
        .ok_or(CatalogError::InvalidResponse)?;
    Ok(artists
        .iter()
        .take(10)
        .filter_map(|artist| parse_artist(artist, None).ok())
        .collect())
}

fn parse_artist(value: &Value, expected_id: Option<&str>) -> Result<Artist, CatalogError> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .and_then(catalog_artist_id)
        .ok_or(CatalogError::InvalidResponse)?;
    if expected_id.is_some_and(|expected| expected != id) {
        return Err(CatalogError::InvalidResponse);
    }
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .and_then(bounded_catalog_text)
        .ok_or(CatalogError::InvalidResponse)?;
    let aliases = match value.get("aliases") {
        None if expected_id.is_none() => Vec::new(),
        Some(Value::Array(aliases)) => aliases
            .iter()
            .filter_map(|alias| {
                alias
                    .get("name")
                    .and_then(Value::as_str)
                    .and_then(bounded_catalog_text)
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .take(100)
            .collect(),
        _ => return Err(CatalogError::InvalidResponse),
    };
    Ok(Artist {
        id,
        name,
        aliases,
        sort_name: value
            .get("sort-name")
            .and_then(Value::as_str)
            .and_then(bounded_catalog_text),
    })
}

fn parse_artist_recordings(
    payload: &Value,
    artist_id: &str,
) -> Result<Vec<Candidate>, CatalogError> {
    let artist_id = catalog_artist_id(artist_id).ok_or(CatalogError::InvalidResponse)?;
    let recordings = payload
        .get("recordings")
        .and_then(Value::as_array)
        .ok_or(CatalogError::InvalidResponse)?;
    Ok(recordings
        .iter()
        .take(25)
        .filter(|recording| {
            recording
                .get("artist-credit")
                .and_then(Value::as_array)
                .is_some_and(|credits| {
                    credits.iter().any(|credit| {
                        credit
                            .get("artist")
                            .and_then(|artist| artist.get("id"))
                            .and_then(Value::as_str)
                            .and_then(catalog_artist_id)
                            .as_deref()
                            == Some(&artist_id)
                    })
                })
        })
        .filter_map(parse_candidate)
        .collect())
}

fn album_metadata_query(
    title: &str,
    album: &str,
    release_id: Option<&str>,
    duration: Duration,
) -> Result<Option<String>, CatalogError> {
    let title = title.trim();
    if title.is_empty() {
        return Ok(None);
    }
    let scope = if let Some(id) = release_id {
        let id = normalized_value(EvidenceField::ReleaseMbid, id)
            .ok_or(CatalogError::InvalidResponse)?;
        format!("reid:{}", lucene_quote(&id))
    } else {
        let album = album.trim();
        if album.is_empty() {
            return Ok(None);
        }
        format!("release:{}", lucene_quote(album))
    };
    Ok(Some(with_duration_query(
        format!("recording:{} AND {scope}", lucene_quote(title)),
        duration,
    )))
}

fn with_duration_query(mut query: String, duration: Duration) -> String {
    let ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    if ms > 0 {
        query.push_str(&format!(
            " AND dur:[{} TO {}]",
            ms.saturating_sub(10_000),
            ms.saturating_add(10_000)
        ));
    }
    query
}

fn parse_candidates(payload: &Value) -> Result<Vec<Candidate>, CatalogError> {
    let recordings = payload
        .get("recordings")
        .and_then(Value::as_array)
        .ok_or(CatalogError::InvalidResponse)?;
    Ok(recordings
        .iter()
        .take(100)
        .filter_map(parse_candidate)
        .collect())
}

fn parse_acoustic_candidates(payload: &Value) -> Result<Vec<AcousticCandidate>, CatalogError> {
    if payload.get("status").and_then(Value::as_str) != Some("ok") {
        return Err(CatalogError::InvalidResponse);
    }
    let results = payload
        .get("results")
        .and_then(Value::as_array)
        .ok_or(CatalogError::InvalidResponse)?;
    Ok(results
        .iter()
        .take(100)
        .filter_map(|result| {
            let score = parse_number(result.get("score"))?;
            let recordings = result.get("recordings")?.as_array()?;
            if recordings.len() > 100 {
                return None;
            }
            let recording_ids = recordings
                .iter()
                .map(|recording| parse_mbid(recording.get("id")?))
                .collect::<Option<Vec<_>>>()?;
            Some(AcousticCandidate {
                recording_ids,
                score,
            })
        })
        .collect())
}

fn parse_community_tags(payload: &Value) -> Result<Vec<CommunityTag>, CatalogError> {
    if payload.get("error").is_some() {
        return Err(CatalogError::LastFm);
    }
    let tags = payload
        .get("toptags")
        .and_then(|tags| tags.get("tag"))
        .and_then(Value::as_array)
        .ok_or(CatalogError::InvalidResponse)?;
    Ok(tags
        .iter()
        .take(50)
        .filter_map(|tag| {
            Some(CommunityTag {
                name: bounded_catalog_text(tag.get("name")?.as_str()?)?,
                count: parse_u64(tag.get("count"))?,
            })
        })
        .collect())
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
    })
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
        releases_complete: false,
        length_ms: value.get("length").and_then(Value::as_u64),
        genres: value
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(20)
            .filter_map(|g| {
                g.get("name")
                    .and_then(Value::as_str)
                    .and_then(bounded_catalog_text)
            })
            .collect(),
        credits: value
            .get("relations")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(40)
            .flat_map(|relation| {
                std::iter::once(relation).chain(
                    relation
                        .get("work")
                        .and_then(|work| work.get("relations"))
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .take(40),
                )
            })
            .take(40)
            .filter_map(|r| {
                let kind = bounded_catalog_text(r.get("type")?.as_str()?)?;
                let entity = r.get("artist").or_else(|| r.get("work"))?;
                let name = bounded_catalog_text(
                    entity
                        .get("name")
                        .or_else(|| entity.get("title"))?
                        .as_str()?,
                )?;
                Some(format!("{kind}: {name}"))
            })
            .collect(),
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
    let mut occurrences = 0;
    let mut slots = Vec::new();
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
            if let Some(recording) = track.get("recording")
                && let (Some(id), Some(recording_id)) = (
                    track.get("id").and_then(parse_mbid),
                    recording.get("id").and_then(parse_mbid),
                )
            {
                slots.push(ReleaseSlot {
                    id,
                    recording_id,
                    title: track
                        .get("title")
                        .or_else(|| recording.get("title"))
                        .and_then(Value::as_str)
                        .and_then(bounded_catalog_text)
                        .unwrap_or_default(),
                    artist: track
                        .get("artist-credit")
                        .or_else(|| recording.get("artist-credit"))
                        .map_or_else(String::new, artist_credit),
                    length_ms: track
                        .get("length")
                        .or_else(|| recording.get("length"))
                        .and_then(Value::as_u64),
                    track_no: parse_u32(track.get("position")),
                    disc_no: medium_position,
                });
            }
            if track
                .get("recording")
                .and_then(|recording| recording.get("id"))
                .and_then(Value::as_str)
                == Some(recording_id)
            {
                track_no = parse_u32(track.get("position"));
                disc_no = medium_position;
                occurrences += 1;
            }
        }
    }
    if occurrences != 1 {
        track_no = None;
        disc_no = None;
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
        country: value
            .get("country")
            .and_then(Value::as_str)
            .and_then(bounded_catalog_text),
        barcode: value
            .get("barcode")
            .and_then(Value::as_str)
            .and_then(bounded_catalog_text),
        catalog_numbers: value
            .get("label-info")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(20)
            .filter_map(|v| {
                v.get("catalog-number")
                    .and_then(Value::as_str)
                    .and_then(bounded_catalog_text)
            })
            .collect(),
        slots,
    })
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

fn lucene_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn artist_credit_preserves_provider_join_phrases() {
        assert_eq!(
            artist_credit(&json!([
                {"name": "Lead", "joinphrase": " feat. "}, {"name": "Guest"}
            ])),
            "Lead feat. Guest"
        );
    }

    #[test]
    fn missing_catalog_collections_are_errors_not_cached_abstentions() {
        assert!(parse_candidates(&json!({})).is_err());
        assert!(parse_acoustic_candidates(&json!({"status": "ok"})).is_err());
        assert!(parse_community_tags(&json!({"toptags": {}})).is_err());
        assert!(parse_community_tags(&json!({"error": 6})).is_err());
        assert!(
            parse_community_tags(&json!({"toptags": {"tag": []}}))
                .is_ok_and(|tags| tags.is_empty())
        );
    }

    #[test]
    fn duration_retrieval_spans_quantization_boundaries_and_handles_missing_duration() {
        let query = metadata_query("Song", "Artist", Duration::from_millis(179999));
        assert!(query.contains("dur:[169999 TO 189999]"));
        assert!(!query.contains("qdur"));
        assert!(!metadata_query("Song", "Artist", Duration::ZERO).contains("dur:"));
        assert!(
            metadata_query("Song", "Artist", Duration::from_millis(2)).contains("dur:[0 TO 10002]")
        );
    }

    #[test]
    fn album_queries_use_release_scope_without_inventing_an_artist()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            album_metadata_query("Finale", "Soundtrack", None, Duration::from_secs(180))?,
            Some(
                "recording:\"Finale\" AND release:\"Soundtrack\" AND dur:[170000 TO 190000]".into()
            )
        );
        let release = "00000000-0000-0000-0000-000000000009";
        assert_eq!(
            album_metadata_query(
                "Finale",
                "Wrong edition title",
                Some(release),
                Duration::ZERO
            )?,
            Some(format!("recording:\"Finale\" AND reid:\"{release}\""))
        );
        assert!(album_metadata_query("Finale", "", None, Duration::ZERO)?.is_none());
        assert!(album_metadata_query(" ", "Soundtrack", Some(release), Duration::ZERO)?.is_none());
        assert!(
            album_metadata_query("Finale", "Soundtrack", Some("bad-id"), Duration::ZERO).is_err()
        );
        let query = album_metadata_query("Finale\" OR *:*", "Game: II", None, Duration::ZERO)?
            .ok_or("missing query")?;
        assert_eq!(
            query,
            "recording:\"Finale\\\" OR *:*\" AND release:\"Game: II\""
        );
        assert!(!query.contains("artist:"));
        Ok(())
    }

    #[test]
    fn repeated_recording_occurrences_preserve_slots_without_inventing_position()
    -> Result<(), Box<dyn std::error::Error>> {
        let recording = "00000000-0000-0000-0000-000000000001";
        let release = "00000000-0000-0000-0000-000000000002";
        let payload = json!({"id":release,"title":"Album","date":"2026-09-10","country":"GB", "media":[
            {"position":1,"tracks":[{"id":"00000000-0000-0000-0000-000000000003","position":1,"recording":{"id":recording,"title":"Song"}}]},
            {"position":2,"tracks":[{"id":"00000000-0000-0000-0000-000000000004","position":5,"recording":{"id":recording,"title":"Song"}}]}
        ]});
        let detail = parse_release_detail(&payload, release, recording)?;
        assert_eq!(detail.slots.len(), 2);
        assert_eq!(detail.track_no, None);
        assert_eq!(detail.disc_no, None);
        assert_eq!(detail.date.as_deref(), Some("2026-09-10"));
        let recording = parse_recording(
            &json!({"id":recording,"title":"Song", "artist-credit":[{"name":"Artist"}],
            "genres":[{"name":"ambient"}],"relations":[{"type":"composer","artist":{"name":"Composer"}}]}),
            recording,
        )?;
        assert_eq!(recording.genres, vec!["ambient"]);
        assert_eq!(recording.credits, vec!["composer: Composer"]);
        Ok(())
    }

    #[test]
    fn connector_parsers_return_bounded_observations() -> Result<(), Box<dyn std::error::Error>> {
        let tags = parse_community_tags(
            &json!({"toptags": {"tag": vec![json!({"name": "dark", "count": "80"}); 60]}}),
        )?;
        assert_eq!(tags.len(), 50);
        assert_eq!(tags[0].count, 80);
        let candidates = parse_acoustic_candidates(&json!({"status": "ok", "results": [{
            "score": 0.99, "recordings": [
                {"id": "00000000-0000-0000-0000-000000000001"},
                {"id": "00000000-0000-0000-0000-000000000002"}]
        }]}))?;
        assert_eq!(candidates[0].recording_ids.len(), 2);
        assert!(parse_recording(&json!({"id": "different"}), "expected").is_err());
        assert!(
            parse_release_detail(&json!({"id": "different"}), "expected", "recording").is_err()
        );
        Ok(())
    }
}
