//! Bounded, source-attributed catalog claims shared by tagging and freshness checks.
use music_domain::IndexedTrack;
use serde_json::{Value, json};

use crate::cleanup_enrichment::{CleanupEnrichmentRecord, cleanup_enrichment_source_signature};

/// Identity validation never turns a community label into a verified mood.
#[must_use]
pub fn song_catalog_evidence(
    track: &IndexedTrack,
    record: &CleanupEnrichmentRecord,
    musicbrainz_enabled: bool,
    lastfm_enabled: bool,
) -> Option<Value> {
    if !musicbrainz_enabled
        || record.track_id != track.id
        || cleanup_enrichment_source_signature(track).ok().as_deref()
            != Some(&record.source_signature)
        || !matches!(
            record.result.get("status").and_then(Value::as_str),
            Some("identified" | "fingerprinted")
        )
    {
        return None;
    }
    let result = Value::Object(record.result.clone());
    let recording_id = result["identity"]["recording_mbid"].as_str()?;
    if recording_id.len() > 64 || recording_id.is_empty() {
        return None;
    }
    let retrieved_at = result["retrieved_at"].as_u64()?;
    let mut claims = Vec::new();
    for (field, scope) in [("genres", "recording"), ("composers", "recording_credit")] {
        let values = bounded_strings(&result["recording_observations"][field], 12);
        if !values.is_empty() {
            claims.push(json!({"id":format!("catalog.musicbrainz.{field}"), "source":"musicbrainz", "scope":scope,
                "kind":field, "recording_id":recording_id, "retrieved_at":retrieved_at, "value":values}));
        }
    }
    if let Some(date) = result["recording_observations"]["first_release_date"]
        .as_str()
        .filter(|date| music_domain::metadata_date_year(date).is_some())
    {
        claims.push(json!({"id":"catalog.musicbrainz.first_release_date", "source":"musicbrainz", "scope":"recording",
            "kind":"first_release_date", "recording_id":recording_id, "retrieved_at":retrieved_at, "value":date}));
    }
    let community = &result["community_observations"];
    if lastfm_enabled
        && community["status"] == "available"
        && community["recording_mbid"] == recording_id
        && community["evidence_revision"].as_i64() == Some(record.evidence_revision)
        && community["policy_contract"]
            == crate::cleanup_enrichment::CATALOG_EVIDENCE_POLICY_CONTRACT
    {
        let tags = community["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .take(12)
            .filter_map(|tag| {
                let name = tag["name"].as_str().filter(|name| valid_text(name))?;
                let count = tag["count"].as_u64()?;
                Some(json!({"name":name,"count":count}))
            })
            .collect::<Vec<_>>();
        if !tags.is_empty() {
            claims.push(
                json!({"id":"catalog.lastfm.community_tags", "source":"lastfm", "scope":"recording",
                "kind":"weak_community_labels", "recording_id":recording_id,
                "retrieved_at":community["retrieved_at"], "value":tags}),
            );
        }
    }
    if claims.is_empty() {
        return None;
    }
    Some(
        json!({"schema_version":"song-catalog-evidence/v1", "evidence_revision":record.evidence_revision, "claims":claims}),
    )
}

fn valid_text(value: &str) -> bool {
    !value.trim().is_empty() && value.chars().count() <= 128 && !value.chars().any(char::is_control)
}
fn bounded_strings(value: &Value, limit: usize) -> Vec<&str> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|value| valid_text(value))
        .take(limit)
        .collect()
}
