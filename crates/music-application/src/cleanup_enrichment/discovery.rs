//! Siblings supply bounded retrieval hints, never a recording or edition decision.
use super::catalog::CatalogConnector;
use super::evidence::{EvidenceField, normalized_value};
use super::workflow::{loose_equal, select_candidate};
use music_domain::IndexedTrack;
use std::collections::BTreeSet;

const MAX_ANCHORS: usize = 3;
const MAX_RELEASES: usize = 2;

#[derive(Default)]
pub(super) struct Discovery {
    pub releases: Vec<String>,
    pub notes: Vec<String>,
    pub partial: bool,
}

pub(super) async fn discover_releases(
    connector: &dyn CatalogConnector,
    track: &IndexedTrack,
    hypothesis: &IndexedTrack,
    indexed_siblings: &[&IndexedTrack],
) -> Discovery {
    let mut result = Discovery::default();
    let folder = parent(track);
    // Root-level files and conflicting albums do not establish an album group.
    if folder.is_empty() {
        return result;
    }
    let siblings = indexed_siblings
        .iter()
        .copied()
        .filter(|sibling| parent(sibling) == folder)
        .collect::<Vec<_>>();
    let Some(album) = siblings
        .iter()
        .map(|sibling| sibling.metadata.album.trim())
        .find(|album| !album.is_empty())
    else {
        return result;
    };
    if siblings.iter().any(|sibling| {
        !sibling.metadata.album.trim().is_empty() && !loose_equal(album, &sibling.metadata.album)
    }) || (!hypothesis.metadata.album.trim().is_empty()
        && !loose_equal(album, &hypothesis.metadata.album))
    {
        result.notes.push("Sibling release discovery withheld: album tags in this folder disagree with each other or the current song's album evidence.".into());
        return result;
    }
    let mut anchors = siblings
        .into_iter()
        .filter(|sibling| {
            sibling.id != track.id
                && !sibling.metadata.title.trim().is_empty()
                && !sibling.metadata.artist.trim().is_empty()
                && loose_equal(album, &sibling.metadata.album)
        })
        .collect::<Vec<_>>();
    anchors.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.id.cmp(&right.id))
    });
    // Duplicate files or different artists' copies of one song cannot provide
    // the independent title evidence needed for album agreement.
    let mut titles = Vec::<&str>::new();
    let anchors = anchors
        .into_iter()
        .filter(|sibling| {
            let title = sibling.metadata.title.as_str();
            if titles.iter().any(|seen| loose_equal(seen, title)) {
                return false;
            }
            titles.push(title);
            true
        })
        .take(MAX_ANCHORS)
        .collect::<Vec<_>>();
    if anchors.len() < 2 {
        return result;
    }
    let mut recordings = BTreeSet::new();
    let mut common: Option<BTreeSet<String>> = None;
    for anchor in anchors {
        let values = match connector.search_metadata(anchor).await {
            Ok(values) => values,
            Err(_) => {
                result.partial = true;
                result.notes.push(format!("Sibling lookup for track {} was unavailable; no album hint will be inferred from this incomplete lookup.", anchor.id.get()));
                continue;
            }
        };
        let count = values.len().min(25);
        let Some((candidate, _)) = select_candidate(anchor, values.into_iter().take(25).collect())
        else {
            result.notes.push(format!("Sibling track {} ({:?}, {:?}) returned {count} candidates but no unambiguous title/artist/duration match.", anchor.id.get(), anchor.metadata.title, anchor.metadata.artist));
            continue;
        };
        if !recordings.insert(candidate.id.clone()) {
            continue;
        }
        let releases = candidate
            .releases
            .iter()
            .take(100)
            .filter(|release| {
                release
                    .status
                    .as_deref()
                    .is_none_or(|status| status == "Official")
                    && loose_equal(album, &release.title)
            })
            .filter_map(|release| normalized_value(EvidenceField::ReleaseMbid, &release.id))
            .collect::<BTreeSet<_>>();
        result.notes.push(format!("Sibling track {} ({:?}, {:?}) matched recording {} and supplied {} eligible release hints for album {album:?}.", anchor.id.get(), anchor.metadata.title, anchor.metadata.artist, candidate.id, releases.len()));
        common = Some(match common {
            None => releases,
            Some(previous) => previous.intersection(&releases).cloned().collect(),
        });
    }
    if recordings.len() < 2 || result.partial {
        return result;
    }
    let releases = common.unwrap_or_default();
    if releases.is_empty() || releases.len() > MAX_RELEASES {
        result.notes.push(format!("Sibling release discovery withheld: {} matched songs share {} eligible releases; between one and {MAX_RELEASES} shared releases are required for bounded retrieval.", recordings.len(), releases.len()));
        return result;
    }
    result.notes.push(format!("{} independently titled sibling recordings agree on {} release hints. These only expand the current song's search; recording and edition checks still apply.", recordings.len(), releases.len()));
    result.releases = releases.into_iter().collect();
    result
}

fn parent(track: &IndexedTrack) -> &str {
    track
        .path
        .as_str()
        .rsplit_once('/')
        .map_or("", |(folder, _)| folder)
}

/// Also checked around model review: candidates can depend on unselected neighbors.
pub(super) fn indexed_folder_signature<'a>(
    track: &IndexedTrack,
    tracks: impl IntoIterator<Item = &'a IndexedTrack>,
) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut siblings = tracks
        .into_iter()
        .filter(|sibling| parent(sibling) == parent(track))
        .collect::<Vec<_>>();
    siblings.sort_by(|left, right| left.path.cmp(&right.path));
    let signatures = siblings
        .into_iter()
        .map(super::cleanup_enrichment_source_signature)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(format!("{:x}", Sha256::digest(signatures.join(":"))))
}
