use music_domain::IndexedTrack;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;

/// Bounded connector observations. Scoring, fallback policy, vocabulary mapping,
/// caching and proposal persistence belong to the application workflow.
pub trait CatalogConnector: std::fmt::Debug + Send + Sync {
    fn begin_lookup(&self, _refresh: bool) -> CatalogFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn community_tags_for_recording<'a>(
        &'a self,
        _recording_id: &'a str,
        artist: &'a str,
        title: &'a str,
        api_key: &'a str,
    ) -> CatalogFuture<'a, Vec<CommunityTag>> {
        self.community_tags(artist, title, api_key)
    }
    fn local_evidence<'a>(
        &'a self,
        _track: &'a IndexedTrack,
    ) -> CatalogFuture<'a, super::evidence::LocalEvidence> {
        Box::pin(async { Ok(super::evidence::LocalEvidence::default()) })
    }
    fn search_isrc<'a>(&'a self, _isrc: &'a str) -> CatalogFuture<'a, Vec<Candidate>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    /// Retrieve by song title within a release ID, or an album title when no ID is supplied.
    /// Artist may be missing; the application still owns identity acceptance.
    fn search_album_metadata<'a>(
        &'a self,
        _track: &'a IndexedTrack,
        _release_id: Option<&'a str>,
    ) -> CatalogFuture<'a, Vec<Candidate>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn search_artists<'a>(&'a self, _name: &'a str) -> CatalogFuture<'a, Vec<Artist>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn artist<'a>(&'a self, _artist_id: &'a str) -> CatalogFuture<'a, Artist> {
        Box::pin(async { Err(CatalogError::InvalidResponse) })
    }
    fn search_artist_recordings<'a>(
        &'a self,
        _track: &'a IndexedTrack,
        _artist_id: &'a str,
    ) -> CatalogFuture<'a, Vec<Candidate>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn runtime_credential(&self, source: CatalogCredentialSource) -> Option<&str>;
    fn search_metadata<'a>(&'a self, track: &'a IndexedTrack) -> CatalogFuture<'a, Vec<Candidate>>;
    fn recording<'a>(&'a self, recording_id: &'a str) -> CatalogFuture<'a, Recording>;
    fn release<'a>(
        &'a self,
        release_id: &'a str,
        recording_id: &'a str,
    ) -> CatalogFuture<'a, ReleaseDetail>;
    fn fingerprint_candidates<'a>(
        &'a self,
        track: &'a IndexedTrack,
        api_key: &'a str,
    ) -> CatalogFuture<'a, Vec<AcousticCandidate>>;
    fn community_tags<'a>(
        &'a self,
        artist: &'a str,
        title: &'a str,
        api_key: &'a str,
    ) -> CatalogFuture<'a, Vec<CommunityTag>>;
}

pub type CatalogFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, CatalogError>> + Send + 'a>>;

#[derive(Debug, Clone, Copy)]
pub enum CatalogCredentialSource {
    AcoustId,
    LastFm,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcousticCandidate {
    pub recording_ids: Vec<String>,
    pub score: f64,
}

#[derive(Debug, Clone)]
pub struct CommunityTag {
    pub name: String,
    pub count: u64,
}

/// Catalog spellings are retrieval observations, not replacements for track credits.
#[derive(Debug, Clone, Default)]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub sort_name: Option<String>,
    pub aliases: Vec<String>,
    /// Only explicitly typed artist-name aliases, excluding search hints and untyped entries.
    pub credit_aliases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub length_ms: Option<u64>,
    pub releases: Vec<ReleaseSummary>,
    pub provider_score: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Recording {
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub artist_credits: Vec<ArtistCredit>,
    #[serde(default)]
    pub lookup_notes: Vec<String>,
    pub first_release_date: Option<String>,
    pub releases: Vec<ReleaseSummary>,
    pub releases_complete: bool,
    pub genres: Vec<String>,
    pub credits: Vec<String>,
    #[serde(default)]
    pub composers: Vec<String>,
    pub length_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseSummary {
    pub id: String,
    pub title: String,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReleaseDetail {
    pub id: String,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub artist_credits: Vec<ArtistCredit>,
    pub date: Option<String>,
    #[serde(default)]
    pub original_release_date: Option<String>,
    #[serde(default)]
    pub release_group_id: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub country: Option<String>,
    pub barcode: Option<String>,
    pub catalog_numbers: Vec<String>,
    pub slots: Vec<ReleaseSlot>,
}

/// Every credit member and its join phrase must survive before alias comparison.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistCredit {
    pub artist_id: String,
    pub name: String,
    pub join_phrase: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseSlot {
    pub id: String,
    pub recording_id: String,
    pub title: String,
    pub artist: String,
    pub length_ms: Option<u64>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub enum CatalogFailure {
    CoolingDown,
    HttpStatus(u16),
    Timeout,
    Transport,
    InvalidJson,
    InvalidPayload,
    ResponseTooLarge,
    ProviderCode(u32),
    HttpProviderCode { status: u16, code: u32 },
}

impl Display for CatalogFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::CoolingDown => formatter.write_str("provider cooldown; retry later"),
            Self::HttpStatus(status) => write!(formatter, "HTTP {status}"),
            Self::Timeout => formatter.write_str("request timed out"),
            Self::Transport => formatter.write_str("connection or response transfer failed"),
            Self::InvalidJson => formatter.write_str("invalid JSON response"),
            Self::InvalidPayload => formatter.write_str("unexpected response structure"),
            Self::ResponseTooLarge => formatter.write_str("response exceeded the size limit"),
            Self::ProviderCode(code) => write!(formatter, "provider error code {code}"),
            Self::HttpProviderCode { status, code } => {
                write!(formatter, "HTTP {status}; provider error code {code}")
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CatalogError {
    StaleSource,
    MusicBrainz,
    AcoustIdUnavailable,
    AcoustId,
    Fingerprint,
    LastFmUnavailable,
    LastFm,
    Storage,
    InvalidResponse,
    MusicBrainzFailure(CatalogFailure),
    AcoustIdFailure(CatalogFailure),
    LastFmFailure(CatalogFailure),
}

impl CatalogError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::StaleSource => "cleanup_source_changed_rescan_required",
            Self::MusicBrainz | Self::MusicBrainzFailure(_) => "musicbrainz_unavailable",
            Self::AcoustIdUnavailable => "acoustid_not_configured",
            Self::AcoustId | Self::AcoustIdFailure(_) => "acoustid_unavailable",
            Self::Fingerprint => "fingerprint_failed",
            Self::LastFmUnavailable => "lastfm_not_configured",
            Self::LastFm | Self::LastFmFailure(_) => "lastfm_unavailable",
            Self::Storage => "catalog_suggestions_not_stored",
            Self::InvalidResponse => "catalog_response_invalid",
        }
    }

    /// Only typed categories and numeric codes reach retained review evidence.
    /// Raw HTTP errors, request URLs and provider messages can contain secrets.
    pub fn annotate(self, note: &str) -> String {
        let (provider, failure) = match self {
            Self::MusicBrainzFailure(failure) => ("MusicBrainz", failure),
            Self::AcoustIdFailure(failure) => ("AcoustID", failure),
            Self::LastFmFailure(failure) => ("Last.fm", failure),
            _ => return note.to_owned(),
        };
        format!("{note} Details: {provider}: {failure}.")
    }
}

impl Display for CatalogError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for CatalogError {}
