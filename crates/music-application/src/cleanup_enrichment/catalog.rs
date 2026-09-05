use music_domain::IndexedTrack;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;

/// Bounded connector observations. Scoring, fallback policy, vocabulary mapping,
/// caching and proposal persistence belong to the application workflow.
pub trait CatalogConnector: std::fmt::Debug + Send + Sync {
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

#[derive(Debug, Clone)]
pub struct AcousticCandidate {
    pub recording_ids: Vec<String>,
    pub score: f64,
}

#[derive(Debug, Clone)]
pub struct CommunityTag {
    pub name: String,
    pub count: u64,
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub length_ms: Option<u64>,
    pub releases: Vec<ReleaseSummary>,
    pub provider_score: f64,
}

#[derive(Debug, Clone)]
pub struct Recording {
    pub title: String,
    pub artist: String,
    pub first_release_date: Option<String>,
    pub releases: Vec<ReleaseSummary>,
}

#[derive(Debug, Clone)]
pub struct ReleaseSummary {
    pub id: String,
    pub title: String,
    pub status: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ReleaseDetail {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub date: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub enum CatalogError {
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
    pub const fn code(self) -> &'static str {
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

impl std::error::Error for CatalogError {}
