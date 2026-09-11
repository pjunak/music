use super::catalog::{Candidate, CatalogConnector};
use super::workflow::loose_equal;
use music_domain::IndexedTrack;
use std::collections::{BTreeMap, BTreeSet};

const MAX_ARTISTS: usize = 3;

#[derive(Default)]
pub(super) struct AliasRetrieval {
    pub candidates: Vec<Candidate>,
    pub notes: Vec<String>,
    pub partial: bool,
}

pub(super) async fn retrieve_alias_candidates(
    connector: &dyn CatalogConnector,
    track: &IndexedTrack,
    hypothesis: &IndexedTrack,
) -> AliasRetrieval {
    let mut result = AliasRetrieval::default();
    let mut names = BTreeSet::new();
    let mut queries = Vec::new();
    for query in [track, hypothesis] {
        let name = query.metadata.artist.trim();
        let title = title(query);
        if !name.is_empty()
            && !title.is_empty()
            && name.len() <= 512
            && !name.chars().any(char::is_control)
        {
            names.insert(name);
            queries.push(query);
        }
    }
    let mut artists = BTreeMap::<String, BTreeSet<&str>>::new();
    for name in names {
        match connector.search_artists(name).await {
            Ok(values) => {
                result.notes.push(format!("Artist name/alias search for {name:?} returned {} candidates; spelling and identity still require separate checks.", values.len().min(10)));
                for artist in values.into_iter().take(10) {
                    artists.entry(artist.id).or_default().insert(name);
                }
            }
            Err(_) => {
                result.partial = true;
                result.notes.push(format!(
                    "Artist name/alias search for {name:?} was unavailable."
                ));
            }
        }
    }
    if artists.len() > MAX_ARTISTS {
        result.notes.push(format!("Artist alias expansion withheld: {} artist IDs exceed the {MAX_ARTISTS}-artist review bound; no artist was chosen from this ambiguous list.", artists.len()));
        return result;
    }
    let mut seen = BTreeSet::new();
    for (id, names) in artists {
        let artist = match connector.artist(&id).await {
            Ok(artist) if artist.id == id => artist,
            _ => {
                result.partial = true;
                result.notes.push(format!("Artist alias details for {id} were unavailable or contradicted the requested ID."));
                continue;
            }
        };
        let verified_names = names
            .iter()
            .copied()
            .filter(|name| {
                std::iter::once(&artist.name)
                    .chain(artist.sort_name.iter())
                    .chain(artist.aliases.iter().take(100))
                    .any(|spelling| loose_equal(name, spelling))
            })
            .collect::<BTreeSet<_>>();
        for name in names.difference(&verified_names) {
            result.notes.push(format!("Artist {id} did not list spelling {name:?} in the retrieved name, sort name or aliases; its recordings were not searched for that spelling."));
        }
        for query in &queries {
            let name = query.metadata.artist.trim();
            if !verified_names.contains(name) || !seen.insert((id.clone(), title(query).to_owned()))
            {
                continue;
            }
            match connector.search_artist_recordings(query, &id).await {
                Ok(values) => {
                    result.notes.push(format!("Catalog artist {} ({:?}) lists spelling {name:?}; song-title {:?} lookup within that artist returned {} candidates. This does not authorize replacing the credited artist or selecting an album edition.", id, artist.name, title(query), values.len().min(25)));
                    result.candidates.extend(values.into_iter().take(25));
                }
                Err(_) => {
                    result.partial = true;
                    result.notes.push(format!("Song lookup within catalog artist {id} was unavailable; other evidence remains available for review."));
                }
            }
        }
    }
    result
}

fn title(track: &IndexedTrack) -> &str {
    if track.metadata.title.trim().is_empty() {
        track.display_title.trim()
    } else {
        track.metadata.title.trim()
    }
}
