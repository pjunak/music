use std::error::Error;
use std::future::Future;
use std::pin::Pin;

use music_domain::{IndexedTrack, TrackId};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

pub const CLEANUP_ENRICHMENT_JOB_KIND: &str = "library.cleanup-enrichment";
pub const CLEANUP_ENRICHMENT_SCHEMA: &str = "library-cleanup-enrichment/v1";
pub const MAX_CLEANUP_ENRICHMENT_TRACKS: usize = 500;

pub type CleanupEnrichmentDependencyError = Box<dyn Error + Send + Sync>;
pub type CleanupEnrichmentFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, CleanupEnrichmentDependencyError>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq)]
pub struct CleanupEnrichmentRecord {
    pub track_id: TrackId,
    pub evidence_revision: i64,
    pub source_signature: String,
    pub result: Map<String, Value>,
}

pub trait CleanupEnrichmentRepository: std::fmt::Debug + Send + Sync {
    fn catalog_evidence_revision(&self) -> CleanupEnrichmentFuture<'_, i64>;

    fn cleanup_enrichment(
        &self,
        track_id: TrackId,
    ) -> CleanupEnrichmentFuture<'_, Option<CleanupEnrichmentRecord>>;

    /// Stores a result only while the indexed metadata still has the source
    /// signature and catalog evidence revision consumed by the connector job.
    fn store_cleanup_enrichment<'a>(
        &'a self,
        record: &'a CleanupEnrichmentRecord,
    ) -> CleanupEnrichmentFuture<'a, bool>;
}

pub fn cleanup_enrichment_source_signature(track: &IndexedTrack) -> Result<String, String> {
    let value = serde_json::json!([
        CLEANUP_ENRICHMENT_SCHEMA,
        track.path.as_str(),
        track.metadata.title,
        track.display_title,
        track.metadata.artist,
        track.metadata.album_artist,
        track.metadata.album,
        track.metadata.track_no,
        track.metadata.disc_no,
        track.metadata.year,
        track.metadata.genre,
        track.metadata.bpm,
        track.duration.as_millis(),
        track.size_bytes,
        track.mtime_unix_seconds,
    ]);
    serde_json::to_vec(&value)
        .map(|encoded| format!("{:x}", Sha256::digest(encoded)))
        .map_err(|_| "cleanup enrichment source signature could not be encoded".to_owned())
}
