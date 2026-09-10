use super::catalog::{Candidate, CatalogConnector, CatalogError, Recording};
use super::evidence::{EvidenceField, LocalEvidence};
use super::workflow::{candidate_score, loose_equal, select_acoustic_candidate, select_candidate};
use music_domain::IndexedTrack;
use std::collections::BTreeMap;

pub(super) struct IdentityResolution {
    pub identity: Option<(String, &'static str, f64, Recording)>,
    pub candidates: Vec<Candidate>,
    pub notes: Vec<String>,
    pub partial: bool,
}

pub(super) async fn resolve_identity(
    connector: &dyn CatalogConnector,
    track: &IndexedTrack,
    hypothesis: &IndexedTrack,
    evidence: &LocalEvidence,
    acoustid_key: Option<&str>,
) -> Result<IdentityResolution, CatalogError> {
    let mut result = IdentityResolution {
        identity: None,
        candidates: Vec::new(),
        notes: evidence.notes.clone(),
        partial: false,
    };
    let ids = evidence.values(EvidenceField::RecordingMbid);
    if ids.len() > 1 {
        result.notes.push("Embedded or imported recording IDs disagree. Resolve the conflicting IDs before catalog identification.".into());
        return Ok(result);
    }
    if let Some(id) = ids.first() {
        match connector.recording(id).await {
            Ok(recording) if duration_compatible(track, recording.length_ms) => {
                result.identity = Some((id.clone(), "identifier", 1.0, recording));
                result.notes.push("Recording identified by an embedded/imported MusicBrainz ID; verify the existing identifier belongs to this audio.".into());
                return Ok(result);
            }
            Ok(_) => {
                result.notes.push("The recording ID conflicts with the audio duration; no identity change was proposed.".into());
                return Ok(result);
            }
            Err(_) => {
                result.partial = true;
                result.notes.push(
                    "The embedded recording ID lookup failed; other evidence will still be tried."
                        .into(),
                );
            }
        }
    }
    let mut candidates = BTreeMap::<String, Candidate>::new();
    for isrc in evidence.values(EvidenceField::Isrc).iter().take(3) {
        match connector.search_isrc(isrc).await {
            Ok(values) => {
                for candidate in values {
                    candidates.insert(candidate.id.clone(), candidate);
                }
            }
            Err(_) => {
                result.partial = true;
                result.notes.push("ISRC lookup was unavailable.".into());
            }
        }
    }
    let isrc_match = (candidates.len() == 1)
        .then(|| candidates.values().next())
        .flatten()
        .filter(|c| {
            duration_compatible(track, c.length_ms)
                && (track.metadata.title.is_empty() || loose_equal(&track.metadata.title, &c.title))
                && (track.metadata.artist.is_empty()
                    || loose_equal(&track.metadata.artist, &c.artist))
        })
        .cloned();
    let mut matched = isrc_match.map(|c| (c.id, "isrc", 1.0));
    if matched.is_none() {
        for query in [track, hypothesis] {
            if std::ptr::eq(query, hypothesis) && query.metadata == track.metadata {
                continue;
            }
            match connector.search_metadata(query).await {
                Ok(values) => {
                    for candidate in values {
                        let current = candidates
                            .entry(candidate.id.clone())
                            .or_insert_with(|| candidate.clone());
                        if candidate.provider_score > current.provider_score {
                            *current = candidate;
                        }
                    }
                }
                Err(_) => {
                    result.partial = true;
                    result.notes.push("Text lookup was unavailable; fingerprint fallback remains available when enabled.".into());
                }
            }
        }
    }
    if matched.is_none() {
        matched = select_text_identity(
            track,
            hypothesis,
            &candidates.values().cloned().collect::<Vec<_>>(),
        );
    }
    result.candidates = candidates.into_values().collect();
    result.candidates.sort_by(|a, b| {
        candidate_score(hypothesis, b)
            .total_cmp(&candidate_score(hypothesis, a))
            .then_with(|| a.id.cmp(&b.id))
    });
    result.candidates.truncate(25);
    if matched.is_none()
        && let Some(key) = acoustid_key
    {
        match connector.fingerprint_candidates(track, key).await {
            Ok(candidates) => {
                matched = select_acoustic_candidate(candidates)
                    .map(|(id, score)| (id, "fingerprint", score));
            }
            Err(_) => {
                result.partial = true;
                result.notes.push(
                    "Fingerprint lookup failed; text candidates remain available for review."
                        .into(),
                );
            }
        }
    }
    if let Some((id, method, score)) = matched {
        let recording = match connector.recording(&id).await {
            Ok(recording) => recording,
            Err(_) => {
                result.partial = true;
                result.notes.push("Recording details were unavailable; retrieved candidates remain available for review.".into());
                return Ok(result);
            }
        };
        if duration_compatible(track, recording.length_ms) {
            result.identity = Some((id, method, score, recording));
        } else {
            result.notes.push("Recording details conflict with the audio duration; the candidate remains unresolved.".into());
        }
    }
    Ok(result)
}

pub(super) fn select_text_identity(
    track: &IndexedTrack,
    hypothesis: &IndexedTrack,
    candidates: &[Candidate],
) -> Option<(String, &'static str, f64)> {
    // Decide only after both retrieval strategies, so retrieval order cannot
    // hide a competing recording or choose a different aggregate winner.
    let mut winners = BTreeMap::new();
    for (query, method) in [(track, "metadata"), (hypothesis, "local_hypothesis")] {
        if let Some((candidate, score)) = select_candidate(query, candidates.to_vec()) {
            winners.entry(candidate.id).or_insert((method, score));
        }
    }
    if winners.len() != 1 {
        return None;
    }
    winners
        .into_iter()
        .next()
        .map(|(id, (method, score))| (id, method, score))
}

fn duration_compatible(track: &IndexedTrack, length_ms: Option<u64>) -> bool {
    track.duration.is_zero()
        || length_ms
            .is_none_or(|length| track.duration.as_millis().abs_diff(u128::from(length)) <= 10_000)
}
