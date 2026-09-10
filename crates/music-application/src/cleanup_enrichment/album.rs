//! Bounded album assignment with explicit unmatched slots and deterministic ties.
use super::catalog::{ReleaseDetail, ReleaseSlot};
use music_domain::{IndexedTrack, TrackId, cleanup_loose_key};
use serde::Serialize;
use std::collections::BTreeMap;

const UNMATCHED: i64 = 100;

#[derive(Debug, Serialize)]
pub struct AlbumAssignment {
    pub classification: &'static str,
    pub considered: usize,
    pub matched: usize,
    pub unmatched_tracks: Vec<i64>,
    pub unmatched_slots: Vec<String>,
    pub slots: BTreeMap<i64, String>,
}

pub fn assign_album(
    tracks: &[IndexedTrack],
    release: &ReleaseDetail,
    known: (TrackId, &str),
) -> AlbumAssignment {
    let rows = tracks
        .iter()
        .filter(|t| t.id == known.0)
        .chain(tracks.iter().filter(|t| t.id != known.0))
        .take(100)
        .collect::<Vec<_>>();
    let slots = release.slots.iter().take(500).collect::<Vec<_>>();
    let costs = rows
        .iter()
        .map(|track| {
            let mut costs = slots
                .iter()
                .map(|slot| slot_cost(track, slot, known))
                .collect::<Vec<_>>();
            costs.extend(std::iter::repeat_n(UNMATCHED, rows.len()));
            costs
        })
        .collect::<Vec<_>>();
    let assignment = minimum_assignment(&costs);
    let mut selected = BTreeMap::new();
    for (i, column) in assignment.into_iter().enumerate() {
        if column >= slots.len() || costs[i][column] >= UNMATCHED {
            continue;
        }
        // Equal occurrences remain ambiguous even when the global algorithm
        // can manufacture a permutation. A position/length discriminator is required.
        if costs[i][..slots.len()]
            .iter()
            .enumerate()
            .any(|(other, cost)| other != column && (*cost - costs[i][column]).abs() < 5)
        {
            continue;
        }
        selected.insert(rows[i].id.get(), slots[column].id.clone());
    }
    let albums = rows
        .iter()
        .map(|t| cleanup_loose_key(&t.metadata.album))
        .filter(|v| !v.is_empty())
        .collect::<std::collections::BTreeSet<_>>();
    let artists = rows
        .iter()
        .map(|t| cleanup_loose_key(&t.metadata.artist))
        .filter(|v| !v.is_empty())
        .collect::<std::collections::BTreeSet<_>>();
    let classification = if rows.len() < 2 {
        "single"
    } else if albums.len() > 1 {
        "mixed"
    } else if artists.len() > 1 {
        "compilation"
    } else {
        "album"
    };
    AlbumAssignment {
        classification,
        considered: rows.len(),
        matched: selected.len(),
        unmatched_tracks: rows
            .iter()
            .filter(|t| !selected.contains_key(&t.id.get()))
            .map(|t| t.id.get())
            .collect(),
        unmatched_slots: slots
            .iter()
            .filter(|s| !selected.values().any(|id| id == &s.id))
            .map(|s| s.id.clone())
            .collect(),
        slots: selected,
    }
}

fn equal(a: &str, b: &str) -> bool {
    let a = cleanup_loose_key(a);
    !a.is_empty() && a == cleanup_loose_key(b)
}

fn slot_cost(track: &IndexedTrack, slot: &ReleaseSlot, known: (TrackId, &str)) -> i64 {
    let known_recording = track.id == known.0;
    if known_recording && slot.recording_id != known.1 {
        return 1000;
    }
    if !known_recording
        && (!equal(&track.metadata.title, &slot.title)
            || !equal(&track.metadata.artist, &slot.artist))
    {
        return 1000;
    }
    let delta = slot
        .length_ms
        .filter(|_| !track.duration.is_zero())
        .map(|length| track.duration.as_millis().abs_diff(u128::from(length)));
    if delta.is_some_and(|delta| delta > 10_000) {
        return 1000;
    }
    let mut cost = if known_recording { 0 } else { 20 };
    if delta.is_some_and(|delta| delta > 2000) {
        cost += 10;
    }
    for (local, catalog) in [
        (track.metadata.track_no, slot.track_no),
        (track.metadata.disc_no, slot.disc_no),
    ] {
        if local.is_some() && catalog.is_some() && local != catalog {
            cost += 30;
        }
    }
    cost
}

/// Rectangular Hungarian algorithm. Every row has its own unmatched alternative.
fn minimum_assignment(costs: &[Vec<i64>]) -> Vec<usize> {
    let n = costs.len();
    if n == 0 {
        return Vec::new();
    }
    let m = costs[0].len();
    let (mut u, mut v, mut p, mut way) = (
        vec![0; n + 1],
        vec![0; m + 1],
        vec![0; m + 1],
        vec![0; m + 1],
    );
    for i in 1..=n {
        p[0] = i;
        let mut j0 = 0;
        let mut min = vec![i64::MAX; m + 1];
        let mut used = vec![false; m + 1];
        loop {
            used[j0] = true;
            let i0 = p[j0];
            let mut delta = i64::MAX;
            let mut j1 = 0;
            for j in 1..=m {
                if !used[j] {
                    let cur = costs[i0 - 1][j - 1] - u[i0] - v[j];
                    if cur < min[j] {
                        min[j] = cur;
                        way[j] = j0;
                    }
                    if min[j] < delta {
                        delta = min[j];
                        j1 = j;
                    }
                }
            }
            for j in 0..=m {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    min[j] -= delta;
                }
            }
            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }
    let mut result = vec![m; n];
    for j in 1..=m {
        if p[j] > 0 {
            result[p[j] - 1] = j - 1;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn assignment_is_global_and_can_leave_tracks_unmatched() {
        // Greedy row-first would consume slot 0 and strand row 1.
        assert_eq!(
            minimum_assignment(&[vec![1, 2, 100, 100], vec![2, 1000, 100, 100]]),
            vec![1, 0]
        );
        assert_eq!(
            minimum_assignment(&[vec![1000, 100, 100], vec![1000, 100, 100]]).len(),
            2
        );
        assert!(minimum_assignment(&[]).is_empty());
    }
}
