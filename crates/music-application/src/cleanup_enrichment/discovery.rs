//! Siblings supply bounded retrieval hints, never a recording or edition decision.
use super::catalog::CatalogConnector;
use super::evidence::{EvidenceField, LocalEvidence, normalized_value};
use super::workflow::{loose_equal, select_candidate};
use music_domain::{IndexedTrack, cleanup_disc_folder_number};
use std::collections::{BTreeMap, BTreeSet};

const MAX_ANCHORS: usize = 3;
const MAX_RELEASES: usize = 2;
const MAX_DISC_FOLDERS: usize = 20;

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
    evidence: &LocalEvidence,
    indexed_siblings: &[&IndexedTrack],
) -> Discovery {
    let mut result = Discovery::default();
    let folder = parent(track);
    // Root-level files and conflicting albums do not establish an album group.
    if folder.is_empty() {
        return result;
    }
    let mut siblings = indexed_context(track, indexed_siblings.iter().copied());
    let mut across_discs = siblings.iter().any(|sibling| parent(sibling) != folder);
    if across_discs {
        if let Err(reason) = validate_disc_group(hypothesis, evidence, &siblings) {
            result.notes.push(format!("Discovery across disc folders withheld: {reason}. Same-folder evidence remains available."));
            siblings.retain(|sibling| parent(sibling) == folder);
            across_discs = false;
        } else {
            result.notes.push("Release discovery includes explicitly numbered sibling disc folders with consistent album and disc evidence. This does not select an edition or expand the review scope.".into());
        }
    }
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
        // Sample the current disc first, then the other folders in path order.
        (across_discs && parent(left) != folder)
            .cmp(&(across_discs && parent(right) != folder))
            .then_with(|| {
                left.path
                    .cmp(&right.path)
                    .then_with(|| left.id.cmp(&right.id))
            })
    });
    // Duplicate files or different artists' copies of one song cannot provide
    // the independent title evidence needed for album agreement.
    let mut chosen = Vec::<&IndexedTrack>::new();
    let mut folders = BTreeSet::new();
    while chosen.len() < MAX_ANCHORS {
        let distinct = |sibling: &&IndexedTrack| {
            !chosen
                .iter()
                .any(|seen| loose_equal(&seen.metadata.title, &sibling.metadata.title))
        };
        let next = anchors
            .iter()
            .copied()
            .filter(distinct)
            .find(|sibling| !folders.contains(parent(sibling)))
            .or_else(|| anchors.iter().copied().find(distinct));
        let Some(next) = next else { break };
        folders.insert(parent(next));
        chosen.push(next);
    }
    let anchors = chosen;
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

fn disc_location(track: &IndexedTrack) -> Option<(&str, u32)> {
    let (album_folder, disc_folder) = parent(track).rsplit_once('/')?;
    if album_folder.is_empty() {
        return None;
    }
    Some((album_folder, cleanup_disc_folder_number(disc_folder)?))
}

/// Include structural candidates even when their tags veto discovery: editing,
/// adding or moving those tracks must invalidate cached and model-review evidence.
pub(super) fn indexed_context<'a>(
    track: &IndexedTrack,
    tracks: impl IntoIterator<Item = &'a IndexedTrack>,
) -> Vec<&'a IndexedTrack> {
    let disc = disc_location(track);
    let mut siblings = tracks
        .into_iter()
        .filter(|sibling| {
            parent(sibling) == parent(track)
                || disc.is_some_and(|(album_folder, _)| {
                    parent(sibling)
                        .rsplit_once('/')
                        .is_some_and(|(other, name)| {
                            other == album_folder && cleanup_disc_folder_number(name).is_some()
                        })
                })
        })
        .collect::<Vec<_>>();
    siblings.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.id.cmp(&right.id))
    });
    siblings
}

fn validate_disc_group(
    hypothesis: &IndexedTrack,
    evidence: &LocalEvidence,
    siblings: &[&IndexedTrack],
) -> Result<(), &'static str> {
    let album = hypothesis.metadata.album.trim();
    if album.is_empty() {
        return Err("the current song has no album evidence");
    }
    if evidence
        .values(EvidenceField::Album)
        .iter()
        .any(|value| !loose_equal(album, value))
    {
        return Err("embedded or imported album observations disagree");
    }
    let Some((_, current_disc)) = disc_location(hypothesis) else {
        return Err("the current folder has no explicit disc number");
    };
    if evidence
        .values(EvidenceField::DiscNo)
        .iter()
        .any(|value| value.parse::<u32>().ok() != Some(current_disc))
    {
        return Err("an embedded or imported disc position contradicts its folder number");
    }
    let mut folders = BTreeMap::new();
    for sibling in siblings.iter().copied().chain(std::iter::once(hypothesis)) {
        let Some((_, disc)) = disc_location(sibling) else {
            return Err("a folder has no explicit disc number");
        };
        if folders
            .insert(disc, parent(sibling))
            .is_some_and(|previous| previous != parent(sibling))
        {
            return Err("multiple folders claim the same disc number");
        }
        if folders.len() > MAX_DISC_FOLDERS {
            return Err("more than 20 disc folders need manual grouping");
        }
        if sibling
            .metadata
            .disc_no
            .is_some_and(|number| number != disc)
        {
            return Err("a disc tag or imported position contradicts its folder number");
        }
        if !sibling.metadata.album.trim().is_empty() && !loose_equal(album, &sibling.metadata.album)
        {
            return Err("album tags or the current album evidence disagree");
        }
    }
    Ok(())
}

/// Also checked around model review: candidates can depend on unselected neighbors.
pub(super) fn indexed_folder_signature<'a>(
    track: &IndexedTrack,
    tracks: impl IntoIterator<Item = &'a IndexedTrack>,
) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let siblings = indexed_context(track, tracks);
    let signatures = siblings
        .into_iter()
        .map(super::cleanup_enrichment_source_signature)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(format!("{:x}", Sha256::digest(signatures.join(":"))))
}
