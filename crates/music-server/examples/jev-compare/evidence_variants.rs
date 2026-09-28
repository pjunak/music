//! Pure transformations of disclosed observations; never consult expectations.
use super::*;

pub(crate) fn transform(name: &str, baseline: &Value, track: &Value) -> Result<Value> {
    let mut state = baseline.clone();
    let observations = state["observations"]
        .as_object_mut()
        .ok_or("missing observations")?;
    match name {
        "metadata_only" => observations.retain(|id, _| id.starts_with("metadata.")),
        "album_only" => observations.retain(|id, _| id == "metadata.album"),
        "genre_only" => observations.retain(|id, _| id == "metadata.genre"),
        "audio_only" => observations.retain(|id, _| id.starts_with("audio.")),
        "without_sections" => observations.retain(|id, _| !id.starts_with("audio.sections.")),
        "without_trajectories" => {
            observations.retain(|id, _| !id.starts_with("audio.trajectories."))
        }
        "without_voice" => {
            observations.remove("audio.voice");
        }
        "numeric_only" => {
            for card in observations.values_mut() {
                if let Some(fields) = card.as_object_mut() {
                    fields.remove("physical_bands");
                    fields.remove("band_scale");
                }
            }
        }
        "bands_only" => {
            for card in observations.values_mut() {
                if let Some(bands) = card
                    .get("physical_bands")
                    .and_then(Value::as_object)
                    .cloned()
                    .filter(|v| !v.is_empty())
                {
                    let fields = card["value"]
                        .as_object_mut()
                        .ok_or("banded value must be an object")?;
                    for (key, band) in bands {
                        fields.insert(key, band);
                    }
                    let fields = card.as_object_mut().ok_or("card")?;
                    fields.remove("physical_bands");
                    fields.insert("band_scale".into(), json!("Low, medium and high split the physical 0-1 proxy into thirds; these are descriptive bins, not calibrated emotion scores. Binned magnitudes replace raw values; timing and missingness are retained."));
                }
            }
        }
        "compact_cards" => {
            let scales = observations
                .values()
                .filter_map(|v| v.get("band_scale"))
                .cloned()
                .collect::<Vec<_>>();
            let shared = scales
                .first()
                .filter(|first| scales.iter().all(|v| v == *first))
                .cloned();
            for (id, card) in observations.iter_mut() {
                if let Some(fields) = card.as_object_mut() {
                    if fields.get("id").and_then(Value::as_str) == Some(id) {
                        fields.remove("id");
                    }
                    if shared.is_some() {
                        fields.remove("band_scale");
                    }
                }
            }
            if let Some(scale) = shared {
                state["shared_physical_band_scale"] = scale;
            }
        }
        "repeat_cards_x3" => {
            let copies = Value::Object(observations.clone());
            state["duplicate_context"] = json!({"source_relationship":"Two copies of the same observations above; no new source or independent corroboration.","copies":[copies.clone(),copies]});
        }
        "genre_as_catalog" | "genre_as_community" => {
            let Some(value) = observations
                .get("metadata.genre")
                .and_then(|v| v["value"].as_str())
                .filter(|v| {
                    !v.trim().is_empty()
                        && v.chars().count() <= 128
                        && !v.chars().any(char::is_control)
                })
                .map(str::to_owned)
            else {
                return Ok(state);
            };
            observations.remove("metadata.genre");
            let (id, claim) = if name == "genre_as_catalog" {
                (
                    "catalog.musicbrainz.genres",
                    json!({"id":"catalog.musicbrainz.genres","source":"musicbrainz","scope":"recording","kind":"genres","retrieved_at":1_790_553_600_u64,"value":[value]}),
                )
            } else {
                (
                    "catalog.lastfm.community_tags",
                    json!({"id":"catalog.lastfm.community_tags","source":"lastfm","scope":"recording","kind":"weak_community_labels","retrieved_at":1_790_553_600_u64,"value":[{"name":value,"count":1}]}),
                )
            };
            observations.insert(id.into(), json!({"id":id,"meaning":"Attributed catalog observation with its supplied source, scope and freshness. Community tags are weak claims and release dates do not establish evoked era.","value":claim}));
        }
        "add_identity_context" => {
            for (field, meaning) in [
                (
                    "artist",
                    "Artist identity: weak contextual information. Isolated words in a performer or company name are not musical descriptions.",
                ),
                (
                    "origin",
                    "Provenance: the source game, film or album name. A source name alone does not describe this recording's mood, setting, activity or evoked era.",
                ),
                (
                    "length_s",
                    "Recording duration in seconds; duration alone does not identify mood, setting or era.",
                ),
                (
                    "bpm",
                    "Embedded beats-per-minute claim, unverified by this analysis; tempo alone does not establish mood or use.",
                ),
            ] {
                if let Some(value) = track
                    .get(field)
                    .filter(|v| !v.is_null() && v.as_str().is_none_or(|s| !s.trim().is_empty()))
                {
                    let id = format!("metadata.{field}");
                    observations
                        .entry(id.clone())
                        .or_insert_with(|| json!({"id":id,"meaning":meaning,"value":value}));
                }
            }
        }
        _ => return Err("unknown evidence arm".into()),
    }
    Ok(state)
}
