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
                for candidate in values.into_iter().take(25) {
                    merge_candidate(&mut candidates, candidate);
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
    let releases = evidence.values(EvidenceField::ReleaseMbid);
    if matched.is_none()
        && let Some(id) = evidence.single(EvidenceField::ReleaseMbid)
    {
        retrieve_album_candidates(
            connector,
            track,
            hypothesis,
            Some(&id),
            &mut candidates,
            &mut result,
        )
        .await;
    }
    if matched.is_none() {
        let mut seen = std::collections::BTreeSet::new();
        for query in [track, hypothesis] {
            let title = if query.metadata.title.trim().is_empty() {
                query.display_title.trim()
            } else {
                query.metadata.title.trim()
            };
            if !seen.insert((title, query.metadata.artist.trim())) {
                continue;
            }
            match connector.search_metadata(query).await {
                Ok(values) => {
                    for candidate in values.into_iter().take(25) {
                        merge_candidate(&mut candidates, candidate);
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
    // Album titles expand an unresolved search only. They do not relax the exact
    // title/artist gates, discard competitors or select an album edition.
    if matched.is_none() && releases.is_empty() {
        retrieve_album_candidates(
            connector,
            track,
            hypothesis,
            None,
            &mut candidates,
            &mut result,
        )
        .await;
        matched = select_text_identity(
            track,
            hypothesis,
            &candidates.values().cloned().collect::<Vec<_>>(),
        );
    } else if matched.is_none() && releases.len() > 1 {
        result.notes.push("Release IDs disagree; album-scoped retrieval was withheld while independent recording lookup remains available.".into());
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
        let text_conflict = matches!(method, "metadata" | "local_hypothesis")
            && ![track, hypothesis].iter().any(|query| {
                let title = if query.metadata.title.trim().is_empty() {
                    query.display_title.as_str()
                } else {
                    query.metadata.title.as_str()
                };
                loose_equal(title, &recording.title)
                    && loose_equal(&query.metadata.artist, &recording.artist)
            });
        if text_conflict || !duration_compatible(track, recording.length_ms) {
            // Do not offer the obsolete search observation to model review after
            // the recording lookup has contradicted the evidence used to select it.
            result.candidates.retain(|candidate| candidate.id != id);
            result.notes.push("Recording details contradict the title, artist or duration used for matching; the conflicting candidate was withheld.".into());
        } else {
            result.identity = Some((id, method, score, recording));
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

async fn retrieve_album_candidates(
    connector: &dyn CatalogConnector,
    track: &IndexedTrack,
    hypothesis: &IndexedTrack,
    release_id: Option<&str>,
    candidates: &mut BTreeMap<String, Candidate>,
    result: &mut IdentityResolution,
) {
    let mut seen = std::collections::BTreeSet::new();
    for query in [track, hypothesis] {
        let title = if query.metadata.title.trim().is_empty() {
            query.display_title.trim()
        } else {
            query.metadata.title.trim()
        };
        let album = release_id.unwrap_or(query.metadata.album.trim());
        if title.is_empty() || album.is_empty() || !seen.insert((title, album)) {
            continue;
        }
        let scope = if release_id.is_some() {
            "release ID"
        } else {
            "album title"
        };
        match connector.search_album_metadata(query, release_id).await {
            Ok(values) => {
                result.notes.push(format!("Lookup by {scope} {album:?} and song title {title:?} returned {} candidates; existing identity checks still apply.", values.len().min(25)));
                for candidate in values.into_iter().take(25) {
                    merge_candidate(candidates, candidate);
                }
            }
            Err(_) => {
                result.partial = true;
                result.notes.push(format!("Lookup by {scope} was unavailable; independent recording and fingerprint lookup remain available."));
            }
        }
    }
}

fn merge_candidate(candidates: &mut BTreeMap<String, Candidate>, mut incoming: Candidate) {
    if !(0.0..=1.0).contains(&incoming.provider_score) {
        return;
    }
    let Some(current) = candidates.get_mut(&incoming.id) else {
        candidates.insert(incoming.id.clone(), incoming);
        return;
    };
    // Keep each response's identity fields together, but retain linked releases
    // from every query. Response order must not discard album evidence on a tie.
    let mut releases = BTreeMap::new();
    for release in current.releases.iter().chain(&incoming.releases) {
        let existing = releases
            .entry(release.id.clone())
            .or_insert_with(|| release.clone());
        if (&release.title, &release.status) < (&existing.title, &existing.status) {
            *existing = release.clone();
        }
    }
    let prefer_incoming = incoming.provider_score > current.provider_score
        || (incoming.provider_score == current.provider_score
            && (&incoming.title, &incoming.artist, incoming.length_ms)
                < (&current.title, &current.artist, current.length_ms));
    incoming.releases = releases.into_values().take(100).collect();
    if prefer_incoming {
        *current = incoming;
    } else {
        current.releases = incoming.releases;
    }
}

#[cfg(test)]
mod tests {
    use super::super::catalog::ReleaseSummary;
    use super::*;

    #[test]
    fn duplicate_recordings_keep_album_evidence_independent_of_query_order()
    -> Result<(), serde_json::Error> {
        let candidate = |title: &str, album: &str| Candidate {
            id: "recording".into(),
            title: title.into(),
            artist: "Composer".into(),
            length_ms: Some(120_000),
            provider_score: 1.0,
            releases: vec![ReleaseSummary {
                id: album.into(),
                title: album.into(),
                status: Some("Official".into()),
            }],
        };
        let first = candidate("Finale", "Original Soundtrack");
        let second = candidate("Finale", "Compilation");
        let merge = |values: Vec<Candidate>| {
            let mut merged = BTreeMap::new();
            for candidate in values {
                merge_candidate(&mut merged, candidate);
            }
            merged
        };
        let forward = merge(vec![first.clone(), second.clone()]);
        let reverse = merge(vec![second, first]);
        assert_eq!(
            serde_json::to_value(&forward)?,
            serde_json::to_value(&reverse)?
        );
        assert_eq!(forward["recording"].releases.len(), 2);
        let mut distinct = candidate("Finale", "Original Soundtrack");
        distinct.id = "competing recording".into();
        let mut merged = forward;
        merge_candidate(&mut merged, distinct);
        assert_eq!(merged.len(), 2);
        let mut invalid = candidate("Other title", "Bogus");
        invalid.provider_score = f64::NAN;
        merge_candidate(&mut merged, invalid);
        assert_eq!(merged["recording"].title, "Finale");
        assert_eq!(merged["recording"].releases.len(), 2);
        Ok(())
    }
}
