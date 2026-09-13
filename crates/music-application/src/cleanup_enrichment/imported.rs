//! Explicit source manifests propose existing writable fields without inventing catalog IDs.
use super::evidence::{EvidenceField, ImportedTrackEvidence, normalized_value};
use music_domain::IndexedTrack;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

pub(super) fn append_review_import(
    result: &mut Map<String, Value>,
    track: &IndexedTrack,
    imported: Option<&ImportedTrackEvidence>,
) {
    let Some(imported) =
        imported.filter(|i| i.propose && i.valid() && i.track_id == track.id.get())
    else {
        return;
    };
    let source = imported.source.as_deref().unwrap_or_default().trim();
    let digest = format!("{:x}", Sha256::digest(json!(imported).to_string()));
    let mut operations = Vec::new();
    for (field, value) in &imported.fields {
        let Some(value) = normalized_value(*field, value) else {
            continue;
        };
        let (field, old, new) = match field {
            EvidenceField::Title => ("title", json!(track.metadata.title), json!(value)),
            EvidenceField::Artist => ("artist", json!(track.metadata.artist), json!(value)),
            EvidenceField::AlbumArtist => (
                "album_artist",
                json!(track.metadata.album_artist),
                json!(value),
            ),
            EvidenceField::Album => ("album", json!(track.metadata.album), json!(value)),
            EvidenceField::Genre if value.len() <= 128 => {
                ("genre", json!(track.metadata.genre), json!(value))
            }
            EvidenceField::TrackNo => (
                "track_no",
                json!(track.metadata.track_no),
                json!(value.parse::<u32>().ok()),
            ),
            EvidenceField::DiscNo => (
                "disc_no",
                json!(track.metadata.disc_no),
                json!(value.parse::<u32>().ok()),
            ),
            EvidenceField::Date => (
                "release_date",
                json!(track.metadata.release_date),
                json!(value),
            ),
            EvidenceField::OriginalDate => (
                "original_release_date",
                json!(track.metadata.original_release_date),
                json!(value),
            ),
            EvidenceField::Composer => ("composer", json!(track.metadata.composer), json!(value)),
            _ => continue, // Identifiers remain evidence, not writable tags.
        };
        if old == new || new.is_null() {
            continue;
        }
        operations.push(json!({
            "op_id": format!("import:{}:{field}:{}", track.id.get(), &digest[..16]),
            "track_id": track.id.get(), "kind": "tag", "field": field,
            "old": old, "new": new, "confidence": "low", "verified": false,
            "rules": ["imported_metadata"],
            "evidence": {"source": "imported_metadata", "reference": source, "entity": "operator_source", "field": field}
        }));
    }
    let count = operations.len();
    if let Some(ops) = result
        .entry("ops")
        .or_insert_with(|| json!([]))
        .as_array_mut()
    {
        ops.extend(operations);
    }
    if let Some(notes) = result
        .entry("notes")
        .or_insert_with(|| json!([]))
        .as_array_mut()
    {
        notes.push(json!(format!("Imported source {source:?} supplies {count} optional field proposals. Confirm the source edition and file mapping; these are not catalog-verified recording identities.")));
    }
}

pub(super) fn release_year(value: &str) -> Option<u32> {
    music_domain::metadata_date_year(value.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_dates_keep_precision_and_reject_impossible_values() {
        for date in ["2025", "2025-10", "2025-10-17", "2024-02-29"] {
            assert!(release_year(date).is_some());
        }
        for date in [
            "2025-02-29",
            "2025-04-31",
            "0000",
            "2025junk",
            "2025-1",
            "2025-00",
            "2025-10-17-01",
        ] {
            assert!(release_year(date).is_none());
        }
    }
}
