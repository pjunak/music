use crate::cleanup::MusicBrainzNameLookup;
use futures_util::TryStreamExt;
use music_application::assistant::{AssistantService, LocalAnalysisRepository};
use music_application::cleanup::CleanupService;
use music_application::cleanup_enrichment::catalog::{
    AcousticCandidate, Candidate, CatalogConnector, CatalogCredentialSource, CatalogError,
    CatalogFuture, CommunityTag, Recording, ReleaseDetail, ReleaseSummary,
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
}

impl HttpCatalogConnector {
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
        parse_candidates(&payload)
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

    async fn fingerprint_candidates(
        &self,
        track: &IndexedTrack,
        api_key: &str,
    ) -> Result<Vec<AcousticCandidate>, CatalogError> {
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
    ) -> Result<Vec<CommunityTag>, CatalogError> {
        let response = self
            .http
            .post(LASTFM_ENDPOINT)
            .form(&[
                ("method", "track.gettoptags"),
                ("artist", artist),
                ("track", title),
                ("api_key", api_key),
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
        parse_community_tags(&payload)
    }
}

impl CatalogConnector for HttpCatalogConnector {
    fn runtime_credential(&self, source: CatalogCredentialSource) -> Option<&str> {
        match source {
            CatalogCredentialSource::AcoustId => self.config.acoustid_api_key.as_ref(),
            CatalogCredentialSource::LastFm => self.config.lastfm_api_key.as_ref(),
        }
        .map(SecretString::expose_secret)
    }
    fn search_metadata<'a>(&'a self, track: &'a IndexedTrack) -> CatalogFuture<'a, Vec<Candidate>> {
        Box::pin(HttpCatalogConnector::search_metadata(self, track))
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
            self, artist, title, api_key,
        ))
    }
}

#[derive(Debug, Deserialize)]
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
