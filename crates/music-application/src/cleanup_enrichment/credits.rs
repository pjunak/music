//! Alias evidence can preserve an authored full credit after recording identification.
//! It never contributes to identity acceptance or invents a translated artist name.
use super::catalog::{Artist, ArtistCredit, CatalogConnector};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct CreditPreservation {
    pub preserve: bool,
    pub partial: bool,
    pub notes: Vec<String>,
}

pub(super) async fn preserve_credit(
    connector: &dyn CatalogConnector,
    authored: &str,
    catalog: &str,
    credits: &[ArtistCredit],
) -> CreditPreservation {
    let mut result = CreditPreservation::default();
    if authored.trim().is_empty() || authored == catalog {
        return result;
    }
    // An incomplete/truncated credit must never equate a solo artist with a collaboration.
    if credits.is_empty()
        || credits.len() > 8
        || authored.len() > 512
        || credits
            .iter()
            .map(|c| format!("{}{}", c.name, c.join_phrase))
            .collect::<String>()
            .trim()
            != catalog
    {
        return result;
    }
    let mut artists = BTreeMap::new();
    for credit in credits {
        if artists.contains_key(&credit.artist_id) {
            continue;
        }
        match connector.artist(&credit.artist_id).await {
            Ok(artist) if artist.id == credit.artist_id => {
                artists.insert(credit.artist_id.clone(), artist);
            }
            response => {
                result.partial = true;
                let note = format!(
                    "Full-credit spelling check for artist {} was unavailable; no alias equivalence was assumed.",
                    credit.artist_id
                );
                result.notes.push(match response {
                    Err(error) => error.annotate(&note),
                    Ok(_) => note,
                });
                return result;
            }
        }
    }
    result.preserve = full_credit_matches(authored, credits, &artists);
    if result.preserve {
        result.notes.push(format!("Preserved authored artist credit {authored:?}: every member and join phrase agrees with catalog artist names or aliases (IDs: {}). Catalog credit {catalog:?} remains identity evidence.", artists.keys().cloned().collect::<Vec<_>>().join(", ")));
    }
    result
}

fn full_credit_matches(
    authored: &str,
    credits: &[ArtistCredit],
    artists: &BTreeMap<String, Artist>,
) -> bool {
    let authored = authored.trim().to_lowercase();
    let mut suffixes = BTreeSet::from([authored.as_str()]);
    for credit in credits {
        let Some(artist) = artists.get(&credit.artist_id) else {
            return false;
        };
        let spellings = std::iter::once(&credit.name)
            .chain(std::iter::once(&artist.name))
            .chain(artist.sort_name.iter())
            .chain(artist.credit_aliases.iter().take(100))
            .filter(|name| name.chars().any(char::is_alphabetic) && name.len() <= 512)
            // Preserve a corroborated script choice, not an abbreviation or spelling error.
            // Decomposed Latin accents normalize to ASCII; other writing systems remain.
            .filter(|name| {
                name.to_lowercase() == credit.name.to_lowercase()
                    || music_domain::cleanup_loose_key(name).is_ascii()
                        != music_domain::cleanup_loose_key(&credit.name).is_ascii()
            })
            .map(|name| format!("{}{}", name.trim(), credit.join_phrase).to_lowercase())
            .collect::<BTreeSet<_>>();
        suffixes = suffixes
            .iter()
            .flat_map(|rest| {
                spellings
                    .iter()
                    .filter_map(|name| rest.strip_prefix(name.as_str()))
            })
            .collect();
        if suffixes.is_empty() {
            return false;
        }
    }
    suffixes.contains("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_credits_keep_all_members_order_and_join_phrases() {
        let artists = BTreeMap::from([
            (
                "one".into(),
                Artist {
                    id: "one".into(),
                    name: "作曲家".into(),
                    credit_aliases: vec!["Composer".into()],
                    ..Artist::default()
                },
            ),
            (
                "two".into(),
                Artist {
                    id: "two".into(),
                    name: "客人".into(),
                    credit_aliases: vec!["Guest Alias".into()],
                    ..Artist::default()
                },
            ),
        ]);
        let mut credits = vec![ArtistCredit {
            artist_id: "one".into(),
            name: "作曲家".into(),
            join_phrase: String::new(),
        }];
        assert!(full_credit_matches("Composer", &credits, &artists));
        assert!(!full_credit_matches("Album name", &credits, &artists));
        assert!(!full_credit_matches(
            "Composer feat. Guest",
            &credits,
            &artists
        ));
        credits[0].join_phrase = " feat. ".into();
        credits.push(ArtistCredit {
            artist_id: "two".into(),
            name: "客人".into(),
            join_phrase: String::new(),
        });
        assert!(full_credit_matches(
            "Composer feat. Guest Alias",
            &credits,
            &artists
        ));
        for bad in [
            "Composer",
            "Guest Alias feat. Composer",
            "Composer & Guest",
            "Composer feat. Stranger",
        ] {
            assert!(!full_credit_matches(bad, &credits, &artists));
        }
    }

    #[test]
    fn search_hints_and_same_script_abbreviations_do_not_preserve_errors() {
        let credits = vec![ArtistCredit {
            artist_id: "one".into(),
            name: "Borislav Slavov".into(),
            join_phrase: String::new(),
        }];
        let artists = BTreeMap::from([(
            "one".into(),
            Artist {
                id: "one".into(),
                name: "Borislav Slavov".into(),
                credit_aliases: vec!["Boris Slavov".into()],
                aliases: vec!["作曲家".into()],
                ..Artist::default()
            },
        )]);
        assert!(!full_credit_matches("Boris Slavov", &credits, &artists));
        assert!(!full_credit_matches("作曲家", &credits, &artists));
    }
}
