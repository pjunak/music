use super::album::assign_album;
use super::catalog::{CatalogConnector, Recording, ReleaseDetail};
use super::evidence::{EvidenceField, LocalEvidence};
use super::workflow::{canonical_metadata, loose_equal, metadata_operations};
use music_domain::IndexedTrack;
use serde_json::{Value, json};

pub(super) struct EditionResolution {
    pub selected: Option<ReleaseDetail>,
    pub choices: Vec<Value>,
    pub notes: Vec<String>,
    pub partial: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn resolve_editions(
    connector: &dyn CatalogConnector,
    track: &IndexedTrack,
    query: &IndexedTrack,
    recording_id: &str,
    recording: &Recording,
    evidence: &LocalEvidence,
    siblings: &[IndexedTrack],
) -> EditionResolution {
    let mut result = EditionResolution {
        selected: None,
        choices: Vec::new(),
        notes: Vec::new(),
        partial: !recording.releases_complete,
    };
    let pinned = evidence.single(EvidenceField::ReleaseMbid);
    if evidence.values(EvidenceField::ReleaseMbid).len() > 1 {
        result
            .notes
            .push("Release IDs disagree; edition-owned fields remain unresolved.".into());
        return result;
    }
    let mut ids = recording
        .releases
        .iter()
        .filter(|r| {
            r.status.as_deref().is_none_or(|s| s == "Official")
                && (query.metadata.album.is_empty() || loose_equal(&query.metadata.album, &r.title))
        })
        .map(|r| r.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    if let Some(id) = &pinned {
        ids.clear();
        ids.insert(id.clone());
    }
    let unique = ids.len() == 1 && (recording.releases_complete || pinned.is_some());
    if ids.len() > 5 || !recording.releases_complete {
        result.notes.push("Release browsing is bounded to 100 editions and five detailed alternatives. More editions may exist; import a release ID to target one explicitly.".into());
    }
    for id in ids.iter().take(5) {
        let mut detail = match connector.release(id, recording_id).await {
            Ok(detail) => detail,
            Err(_) => {
                result.partial = true;
                result.notes.push("Release details were unavailable; recording metadata remains reviewable and release lookup can retry.".into());
                continue;
            }
        };
        // An explicit release identifier must actually contain the identified recording.
        if !detail.slots.iter().any(|s| s.recording_id == recording_id) {
            result.partial = true;
            result.notes.push("The release did not provide a track list containing this recording; edition fields were withheld.".into());
            continue;
        }
        let assignment = assign_album(siblings, &detail, (track.id, recording_id));
        let slot_id = if evidence.values(EvidenceField::ReleaseTrackMbid).len() > 1 {
            result.notes.push(
                "Release-track IDs disagree; track and disc positions remain unresolved.".into(),
            );
            None
        } else {
            evidence
                .single(EvidenceField::ReleaseTrackMbid)
                .or_else(|| assignment.slots.get(&track.id.get()).cloned())
        };
        let slot = slot_id.as_ref().and_then(|id| {
            detail
                .slots
                .iter()
                .find(|s| &s.id == id && s.recording_id == recording_id)
        });
        if !detail.slots.is_empty() {
            detail.track_no = slot.and_then(|s| s.track_no);
            detail.disc_no = slot.and_then(|s| s.disc_no);
        }
        if evidence
            .single(EvidenceField::Barcode)
            .is_some_and(|b| detail.barcode.as_deref() != Some(&b))
        {
            continue;
        }
        if evidence
            .single(EvidenceField::CatalogNumber)
            .is_some_and(|n| !detail.catalog_numbers.contains(&n))
        {
            continue;
        }
        let metadata = canonical_metadata(recording, Some(&detail));
        let mut ops = metadata_operations(track, &metadata, recording_id);
        ops.retain(|op| {
            matches!(
                op["field"].as_str(),
                Some("album" | "album_artist" | "track_no" | "disc_no" | "year")
            )
        });
        for op in &mut ops {
            op["op_id"] = json!(format!(
                "edition:{}:{}:{}",
                track.id.get(),
                id,
                op["field"].as_str().unwrap_or_default()
            ));
            op["evidence"] = json!({"source": "musicbrainz", "entity": "release", "id": id, "release_track_id": slot_id, "date": detail.date});
        }
        result.choices.push(json!({
            "id": id, "title": detail.title, "artist": detail.artist, "date": detail.date,
            "country": detail.country, "barcode": detail.barcode, "catalog_numbers": detail.catalog_numbers,
            "assignment": assignment, "ops": ops,
        }));
        if unique {
            result.selected = Some(detail);
        }
    }
    if result.selected.is_none() && !result.choices.is_empty() {
        result.notes.push("Recording identified; choose an album edition to propose its date, album artist and track positions.".into());
    }
    result
}
