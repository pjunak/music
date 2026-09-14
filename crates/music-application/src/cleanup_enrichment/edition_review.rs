//! Descriptive album comparison, separate from recording identity and writable assignments.
use super::catalog::ReleaseDetail;
use music_domain::{IndexedTrack, cleanup_loose_key};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistinguishingTrack {
    pub title: String,
    pub disc_no: Option<u32>,
    pub track_no: Option<u32>,
    pub present: bool,
    pub duration_agrees: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditionReview {
    pub release_id: String,
    pub compared_release_ids: Vec<String>,
    pub title: String,
    pub description: String,
    pub formats: Vec<String>,
    pub labels: Vec<String>,
    pub folder_tracks: usize,
    pub compared_tracks: usize,
    pub release_tracks: usize,
    pub title_matches: usize,
    pub duration_matches: usize,
    pub duration_conflicts: usize,
    pub distinguishing_tracks: Vec<DistinguishingTrack>,
    pub distinguishing_tracks_total: usize,
    pub missing_titles: Vec<String>,
    pub missing_titles_total: usize,
    pub complete: bool,
    pub recommended: bool,
}

pub fn compare_editions(
    releases: &[ReleaseDetail],
    tracks: &[IndexedTrack],
    alternatives_complete: bool,
) -> Vec<EditionReview> {
    let local = tracks.iter().take(100).collect::<Vec<_>>();
    let keys = releases
        .iter()
        .map(|release| {
            release
                .slots
                .iter()
                .take(500)
                .map(|slot| cleanup_loose_key(&slot.title))
                .collect::<BTreeSet<_>>()
        })
        .collect::<Vec<_>>();
    releases
        .iter()
        .enumerate()
        .map(|(index, release)| {
            let mut title_matches = 0;
            let mut duration_matches = 0;
            let mut duration_conflicts = 0;
            for track in &local {
                let key = cleanup_loose_key(&track.metadata.title);
                let matches = release
                    .slots
                    .iter()
                    .take(500)
                    .filter(|slot| !key.is_empty() && cleanup_loose_key(&slot.title) == key)
                    .collect::<Vec<_>>();
                // Repeated local/remote titles need recording/position evidence; never guess a permutation here.
                if matches.len() != 1
                    || local
                        .iter()
                        .filter(|other| cleanup_loose_key(&other.metadata.title) == key)
                        .count()
                        != 1
                {
                    continue;
                }
                title_matches += 1;
                if let Some(length) = matches[0].length_ms.filter(|_| !track.duration.is_zero()) {
                    let delta = track.duration.as_millis().abs_diff(u128::from(length));
                    duration_matches += usize::from(delta <= 2000);
                    duration_conflicts += usize::from(delta > 10_000);
                }
            }
            let mut missing = Vec::new();
            let mut differences = Vec::new();
            for slot in release.slots.iter().take(500) {
                let key = cleanup_loose_key(&slot.title);
                if key.is_empty() {
                    continue;
                }
                let found = local
                    .iter()
                    .filter(|track| cleanup_loose_key(&track.metadata.title) == key)
                    .collect::<Vec<_>>();
                if found.is_empty() {
                    missing.push(slot.title.clone());
                }
                if releases.len() > 1
                    && keys
                        .iter()
                        .enumerate()
                        .all(|(other, keys)| other == index || !keys.contains(&key))
                {
                    differences.push(DistinguishingTrack {
                        title: slot.title.clone(),
                        disc_no: slot.disc_no,
                        track_no: slot.track_no,
                        present: found.len() == 1,
                        duration_agrees: found.len() == 1
                            && slot.length_ms.is_some_and(|length| {
                                !found[0].duration.is_zero()
                                    && found[0].duration.as_millis().abs_diff(u128::from(length))
                                        <= 2000
                            }),
                    });
                }
            }
            let complete = alternatives_complete
                && tracks.len() <= 100
                && releases
                    .iter()
                    .all(|r| r.tracklist_complete && r.slots.len() <= 500);
            // This is an advisory among retrieved editions, never proof of provenance or an automatic choice.
            let recommended = complete
                && !local.is_empty()
                && local.len() == release.slots.len()
                && duration_matches == local.len()
                && differences.iter().any(|d| d.present && d.duration_agrees);
            let missing_titles_total = missing.len();
            let distinguishing_tracks_total = differences.len();
            missing.truncate(8);
            // Put useful positive distinctions first so the bounded AI disclosure retains them.
            differences.sort_by_key(|d| !(d.present && d.duration_agrees));
            differences.truncate(8);
            EditionReview {
                release_id: release.id.clone(),
                compared_release_ids: releases.iter().map(|r| r.id.clone()).collect(),
                title: release.title.clone(),
                description: release.disambiguation.clone(),
                formats: release.formats.clone(),
                labels: release.labels.clone(),
                folder_tracks: tracks.len(),
                compared_tracks: local.len(),
                release_tracks: release.slots.len(),
                title_matches,
                duration_matches,
                duration_conflicts,
                distinguishing_tracks: differences,
                distinguishing_tracks_total,
                missing_titles: missing,
                missing_titles_total,
                complete,
                recommended,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup_enrichment::catalog::ReleaseSlot;
    use music_domain::{LibraryPath, TrackId, TrackMetadata};
    fn track(id: i64, title: &str) -> Result<IndexedTrack, Box<dyn std::error::Error>> {
        Ok(IndexedTrack {
            id: TrackId::new(id)?,
            path: LibraryPath::parse(format!("Album/{id}.mp3"))?,
            metadata: TrackMetadata {
                title: title.into(),
                artist: "Old artist spelling".into(),
                album: "Album".into(),
                album_artist: String::new(),
                release_date: String::new(),
                original_release_date: String::new(),
                composer: String::new(),
                genre: String::new(),
                track_no: None,
                disc_no: None,
                year: None,
                bpm: None,
            },
            duration: std::time::Duration::from_secs(180),
            display_title: title.into(),
            origin: String::new(),
            size_bytes: 1,
            mtime_unix_seconds: 1,
            added_at_unix_seconds: 1,
        })
    }
    fn release(id: &str, second: &str) -> ReleaseDetail {
        ReleaseDetail {
            id: id.into(),
            title: "Album".into(),
            tracklist_complete: true,
            slots: ["Main Theme", second]
                .into_iter()
                .enumerate()
                .map(|(i, title)| ReleaseSlot {
                    id: format!("{id}-{i}"),
                    recording_id: format!("recording-{title}"),
                    title: title.into(),
                    artist: "Canonical artist".into(),
                    length_ms: Some(180000),
                    track_no: Some(i as u32 + 1),
                    disc_no: Some(1),
                })
                .collect(),
            ..Default::default()
        }
    }
    #[test]
    fn distinguishing_song_supports_advice_without_changing_strict_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let tracks = vec![track(1, "Main Theme")?, track(2, "Streets and Faces")?];
        let releases = vec![
            release("wav", "Lead Your Way"),
            release("mp3", "Streets and Faces"),
        ];
        let reviews = compare_editions(&releases, &tracks, true);
        assert_eq!(reviews[1].title_matches, 2);
        assert!(reviews[1].recommended);
        assert!(!reviews[0].recommended);
        assert_eq!(reviews[0].missing_titles, ["Lead Your Way"]);
        assert!(reviews[1].distinguishing_tracks[0].present);
        let reversed = compare_editions(&[releases[1].clone(), releases[0].clone()], &tracks, true);
        assert!(reversed[0].recommended);
        assert!(
            !compare_editions(&releases, &tracks, false)
                .iter()
                .any(|r| r.recommended)
        );
        Ok(())
    }
    #[test]
    fn incomplete_and_conflicting_evidence_abstains() -> Result<(), Box<dyn std::error::Error>> {
        let base = vec![track(1, "Main Theme")?, track(2, "Finale")?];
        let mut releases = vec![release("a", "Finale"), release("b", "Bonus")];
        let mut tracks = base.clone();
        tracks[1].duration = std::time::Duration::ZERO;
        assert!(!compare_editions(&releases, &tracks, true)[0].recommended);
        tracks[1].duration = std::time::Duration::from_secs(240);
        assert_eq!(
            compare_editions(&releases, &tracks, true)[0].duration_conflicts,
            1
        );
        tracks = base.clone();
        tracks.push(track(3, "Finale")?);
        assert!(!compare_editions(&releases, &tracks, true)[0].recommended);
        tracks = base.clone();
        tracks.push(track(3, "Bonus")?);
        assert!(
            !compare_editions(&releases, &tracks, true)
                .iter()
                .any(|r| r.recommended)
        );
        releases[1].tracklist_complete = false;
        assert!(!compare_editions(&releases, &base, true)[0].recommended);
        releases[1] = release("b", "Finale");
        assert!(
            !compare_editions(&releases, &base, true)
                .iter()
                .any(|r| r.recommended)
        );
        Ok(())
    }
}
