//! Read-only observations: entity identifiers never share a namespace with titles.
use music_domain::{
    CleanupConfidence, CleanupTagField, CleanupTrackPlan, CleanupValue, IndexedTrack,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceField {
    Title,
    Artist,
    AlbumArtist,
    Album,
    TrackNo,
    DiscNo,
    Date,
    OriginalDate,
    Composer,
    Genre,
    RecordingMbid,
    ReleaseMbid,
    ReleaseTrackMbid,
    ReleaseGroupMbid,
    Isrc,
    Barcode,
    CatalogNumber,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalObservation {
    pub field: EvidenceField,
    pub value: String,
    pub source: String,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalEvidence {
    pub observations: Vec<LocalObservation>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedTrackEvidence {
    pub track_id: i64,
    pub fields: BTreeMap<EvidenceField, String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub propose: bool,
}

impl ImportedTrackEvidence {
    pub fn valid(&self) -> bool {
        self.track_id > 0
            && self.source.as_ref().is_none_or(|source| {
                !source.trim().is_empty()
                    && source.len() <= 512
                    && !source.chars().any(char::is_control)
            })
            && (!self.propose || self.source.is_some())
            && !self.fields.is_empty()
            && self.fields.len() <= 17
            && self.fields.iter().all(|(field, value)| {
                normalized_value(*field, value).is_some()
                    && (!self.propose
                        || !matches!(field, EvidenceField::Date | EvidenceField::OriginalDate)
                        || super::imported::release_year(value).is_some())
            })
    }
}

pub fn normalized_value(field: EvidenceField, value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return None;
    }
    match field {
        EvidenceField::RecordingMbid
        | EvidenceField::ReleaseMbid
        | EvidenceField::ReleaseTrackMbid
        | EvidenceField::ReleaseGroupMbid => {
            let id = uuid::Uuid::parse_str(value).ok()?;
            (!id.is_nil()).then(|| id.hyphenated().to_string())
        }
        EvidenceField::Isrc => {
            let value = value.replace('-', "").to_ascii_uppercase();
            let bytes = value.as_bytes();
            (bytes.len() == 12
                && bytes[..2].iter().all(u8::is_ascii_alphabetic)
                && bytes[2..5].iter().all(u8::is_ascii_alphanumeric)
                && bytes[5..].iter().all(u8::is_ascii_digit))
            .then_some(value)
        }
        EvidenceField::TrackNo | EvidenceField::DiscNo => value
            .split('/')
            .next()?
            .parse::<u32>()
            .ok()
            .filter(|v| (1..=9999).contains(v))
            .map(|v| v.to_string()),
        _ => Some(value.to_owned()),
    }
}

impl LocalEvidence {
    pub fn values(&self, field: EvidenceField) -> BTreeSet<String> {
        self.observations
            .iter()
            .filter(|o| o.field == field)
            .filter_map(|o| normalized_value(field, &o.value))
            .collect()
    }

    pub fn single(&self, field: EvidenceField) -> Option<String> {
        let values = self.values(field);
        (values.len() == 1)
            .then(|| values.into_iter().next())
            .flatten()
    }

    pub fn add_import(&mut self, imported: &ImportedTrackEvidence) {
        self.observations.extend(
            imported
                .fields
                .iter()
                .map(|(field, value)| LocalObservation {
                    field: *field,
                    value: value.clone(),
                    source: imported.source.as_ref().map_or_else(
                        || "imported sidecar".to_owned(),
                        |source| {
                            format!(
                                "imported {}: {}",
                                if imported.propose {
                                    "review source"
                                } else {
                                    "evidence"
                                },
                                source.trim()
                            )
                        },
                    ),
                }),
        );
    }

    pub fn signature(&self, hypothesis: &IndexedTrack) -> Result<String, String> {
        let bytes = serde_json::to_vec(&(
            self,
            super::cleanup_enrichment_source_signature(hypothesis)?,
        ))
        .map_err(|_| "local evidence is invalid".to_owned())?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

/// Local cleanup proposals are retrieval hypotheses only. They never update the index.
pub fn retrieval_hypothesis(
    track: &IndexedTrack,
    plan: Option<&CleanupTrackPlan>,
    evidence: &LocalEvidence,
) -> IndexedTrack {
    let mut query = track.clone();
    if let Some(plan) = plan {
        for op in &plan.operations {
            match (&op.field, &op.new) {
                (Some(CleanupTagField::Title), Some(CleanupValue::Text(v))) => {
                    query.metadata.title.clone_from(v)
                }
                (Some(CleanupTagField::Artist), Some(CleanupValue::Text(v))) => {
                    query.metadata.artist.clone_from(v)
                }
                (Some(CleanupTagField::Album), Some(CleanupValue::Text(v))) => {
                    query.metadata.album.clone_from(v)
                }
                _ => {}
            }
        }
    }
    for (field, original, target) in [
        (
            EvidenceField::Title,
            &track.metadata.title,
            &mut query.metadata.title,
        ),
        (
            EvidenceField::Artist,
            &track.metadata.artist,
            &mut query.metadata.artist,
        ),
        (
            EvidenceField::Album,
            &track.metadata.album,
            &mut query.metadata.album,
        ),
        (
            EvidenceField::AlbumArtist,
            &track.metadata.album_artist,
            &mut query.metadata.album_artist,
        ),
    ] {
        if original.trim().is_empty()
            && let Some(value) = evidence.single(field)
        {
            *target = value;
        }
    }
    for (field, tag_field, target) in [
        (
            EvidenceField::TrackNo,
            CleanupTagField::TrackNumber,
            &mut query.metadata.track_no,
        ),
        (
            EvidenceField::DiscNo,
            CleanupTagField::DiscNumber,
            &mut query.metadata.disc_no,
        ),
    ] {
        if target.is_none() {
            *target = evidence.single(field).and_then(|v| v.parse().ok());
        }
        // Explicit numbers and conflicting observations take precedence over
        // filename hypotheses. Only corroborated local positions may break ties.
        if target.is_none()
            && evidence.values(field).is_empty()
            && let Some(plan) = plan
        {
            *target = plan.operations.iter().find_map(|op| {
                if op.field == Some(tag_field)
                    && op.confidence == CleanupConfidence::High
                    && let Some(CleanupValue::Number(value)) = op.new
                    && value > 0
                {
                    Some(value)
                } else {
                    None
                }
            });
        }
    }
    query
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identifiers_are_typed_normalized_and_conflicts_remain_visible() {
        let recording = "00000000-0000-0000-0000-000000000001";
        let release = "00000000-0000-0000-0000-000000000002";
        let mut evidence = LocalEvidence {
            observations: vec![
                LocalObservation {
                    field: EvidenceField::RecordingMbid,
                    value: recording.into(),
                    source: "ID3v2".into(),
                },
                LocalObservation {
                    field: EvidenceField::ReleaseMbid,
                    value: release.into(),
                    source: "ID3v2".into(),
                },
            ],
            notes: vec![],
        };
        assert_eq!(
            evidence.single(EvidenceField::RecordingMbid).as_deref(),
            Some(recording)
        );
        evidence.observations.push(LocalObservation {
            field: EvidenceField::RecordingMbid,
            value: release.into(),
            source: "APE".into(),
        });
        assert!(evidence.single(EvidenceField::RecordingMbid).is_none());
        assert_eq!(
            normalized_value(EvidenceField::Isrc, "US-ABC-26-12345").as_deref(),
            Some("USABC2612345")
        );
        assert!(normalized_value(EvidenceField::RecordingMbid, "../../release").is_none());
    }
}
