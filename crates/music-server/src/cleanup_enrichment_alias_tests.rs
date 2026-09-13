use super::*;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use music_domain::{LibraryPath, TrackId, TrackMetadata};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const ARTIST: &str = "10000000-0000-0000-0000-000000000001";
const OTHER: &str = "10000000-0000-0000-0000-000000000002";

fn artist_payload() -> Value {
    json!({"id":ARTIST,"name":"Artist","sort-name":"Artist","aliases":[{"name":"作曲家","type":"Artist name"}]})
}

fn recordings() -> Value {
    json!({"recordings":[{"id":"00000000-0000-0000-0000-000000000001","title":"Song","length":120000,"score":100,"artist-credit":[{"name":"Artist","joinphrase":" feat. ","artist":{"id":ARTIST,"name":"Artist"}},{"name":"Guest","artist":{"id":OTHER,"name":"Guest"}}]}]})
}

#[test]
fn artist_queries_keep_aliases_in_the_artist_index_and_quote_literal_text() -> TestResult {
    assert_eq!(
        artist_name_query("作曲家"),
        Some("artist:\"作曲家\" OR alias:\"作曲家\" OR sortname:\"作曲家\"".into())
    );
    assert_eq!(
        artist_name_query("X\" OR *:*"),
        Some(
            "artist:\"X\\\" OR *:*\" OR alias:\"X\\\" OR *:*\" OR sortname:\"X\\\" OR *:*\"".into()
        )
    );
    assert!(artist_name_query(" ").is_none());
    assert!(artist_name_query("Artist\nInjected").is_none());
    assert!(artist_name_query(&"x".repeat(513)).is_none());
    assert_eq!(
        artist_recording_query("Song", ARTIST, Duration::from_secs(120))?,
        Some(format!(
            "recording:\"Song\" AND arid:\"{ARTIST}\" AND dur:[110000 TO 130000]"
        ))
    );
    assert!(
        !artist_recording_query("Song", ARTIST, Duration::ZERO)?
            .ok_or("query")?
            .contains("dur:")
    );
    assert!(artist_recording_query("Song", "../bad-id", Duration::ZERO).is_err());
    assert!(artist_recording_query("", ARTIST, Duration::ZERO)?.is_none());
    Ok(())
}

#[test]
fn artist_observations_validate_ids_and_treat_bounded_aliases_as_a_set() -> TestResult {
    let artist = parse_artist(&artist_payload(), Some(ARTIST))?;
    assert_eq!(artist.aliases, ["作曲家"]);
    assert_eq!(artist.credit_aliases, ["作曲家"]);
    let mut hints = artist_payload();
    hints["aliases"][0]["type"] = json!("Search hint");
    assert!(
        parse_artist(&hints, Some(ARTIST))?
            .credit_aliases
            .is_empty()
    );
    assert!(parse_artist(&artist_payload(), Some(OTHER)).is_err());
    assert!(parse_artist(&json!({"id":ARTIST,"name":"Artist"}), Some(ARTIST)).is_err());
    assert!(parse_artist(&json!({"id":ARTIST,"name":"Artist","aliases":{}}), None).is_err());
    assert!(parse_artist_candidates(&json!({"artists":{}})).is_err());
    assert!(parse_artist_candidates(&json!({"artists":[]}))?.is_empty());
    let mut payload = artist_payload();
    let mut aliases = (0..110)
        .map(|i| json!({"name":format!("Alias {i:03}")}))
        .collect::<Vec<_>>();
    aliases.push(json!({"name":"Alias 000"}));
    aliases.push(json!({"name":"x".repeat(513)}));
    payload["aliases"] = json!(aliases);
    let forward = parse_artist(&payload, Some(ARTIST))?;
    aliases.reverse();
    payload["aliases"] = json!(aliases);
    assert_eq!(
        parse_artist(&payload, Some(ARTIST))?.aliases,
        forward.aliases
    );
    assert_eq!(forward.aliases.len(), 100);
    let hits = json!({"artists": vec![artist_payload(); 11]});
    assert_eq!(parse_artist_candidates(&hits)?.len(), 10);
    Ok(())
}

#[test]
fn artist_scoped_recordings_preserve_full_credits_and_drop_unrelated_artist_ids() -> TestResult {
    let candidates = parse_artist_recordings(&recordings(), ARTIST)?;
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].artist, "Artist feat. Guest");
    assert_eq!(parse_artist_recordings(&recordings(), OTHER)?.len(), 1);
    assert!(
        parse_artist_recordings(&recordings(), "10000000-0000-0000-0000-000000000003")?.is_empty()
    );
    assert!(parse_artist_recordings(&json!({"recordings":null}), ARTIST).is_err());
    Ok(())
}

#[derive(Default)]
struct Fixture {
    requests: Mutex<Vec<(String, BTreeMap<String, String>)>>,
    invalid_recording: AtomicBool,
    invalid_artist: AtomicBool,
    invalid_browse: AtomicBool,
    invalid_detail: AtomicBool,
}

async fn catalog_fixture(
    State(state): State<Arc<Fixture>>,
    Path(path): Path<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> Json<Value> {
    state.requests.lock().await.push((path.clone(), query));
    Json(match path.as_str() {
        "recording" if state.invalid_recording.swap(false, Ordering::SeqCst) => {
            json!({"recordings":{}})
        }
        "recording" => recordings(),
        "artist" => json!({"artists":[{"id":ARTIST,"name":"Artist"}]}),
        "release" if state.invalid_browse.swap(false, Ordering::SeqCst) => {
            json!({"releases":[],"release-count":"invalid"})
        }
        "release" => json!({"releases":[],"release-count":0}),
        _ if path.starts_with("recording/") => {
            let mut payload = recordings()["recordings"][0].clone();
            if state.invalid_detail.swap(false, Ordering::SeqCst) {
                payload["id"] = json!(OTHER);
            }
            payload
        }
        _ if path == format!("artist/{ARTIST}")
            && state.invalid_artist.swap(false, Ordering::SeqCst) =>
        {
            json!({"id":OTHER,"name":"Wrong artist","aliases":[]})
        }
        _ if path == format!("artist/{ARTIST}") => artist_payload(),
        _ => json!({}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalid_entity_and_browse_responses_retry_without_poisoning_cache() -> TestResult {
        let state = Arc::new(Fixture::default());
        state.invalid_detail.store(true, Ordering::SeqCst);
        state.invalid_browse.store(true, Ordering::SeqCst);
        let router = Router::new()
            .route("/{*path}", get(catalog_fixture))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move { axum::serve(listener, router).await });
        let directory = tempfile::tempdir()?;
        let connector = HttpCatalogConnector {
            musicbrainz: Arc::new(MusicBrainzNameLookup::fixture_endpoint(&endpoint)?),
            library_root: LibraryRoot::open(directory.path())?,
            config: CleanupConnectorConfig::new(None, None, "unused-fixture-fpcalc".into()),
            http: Client::new(),
            entities: Mutex::default(),
            fingerprints: Mutex::default(),
        };
        let recording_id = "00000000-0000-0000-0000-000000000001";
        assert!(connector.recording(recording_id).await.is_err());
        let incomplete = connector.recording(recording_id).await?;
        assert!(!incomplete.releases_complete);
        assert!(
            incomplete
                .lookup_notes
                .iter()
                .any(|note| note.contains("MusicBrainz: unexpected response structure"))
        );
        let repaired = connector.recording(recording_id).await?;
        assert!(repaired.releases_complete);
        assert!(repaired.lookup_notes.is_empty());
        connector.recording(recording_id).await?;
        assert_eq!(state.requests.lock().await.len(), 4);
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn cached_alias_text_and_isrc_queries_retry_bad_responses_and_respect_refresh()
    -> TestResult {
        let state = Arc::new(Fixture::default());
        state.invalid_recording.store(true, Ordering::SeqCst);
        state.invalid_artist.store(true, Ordering::SeqCst);
        let router = Router::new()
            .route("/{*path}", get(catalog_fixture))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move { axum::serve(listener, router).await });
        let directory = tempfile::tempdir()?;
        let connector = HttpCatalogConnector {
            musicbrainz: Arc::new(MusicBrainzNameLookup::fixture_endpoint(&endpoint)?),
            library_root: LibraryRoot::open(directory.path())?,
            config: CleanupConnectorConfig::new(None, None, "unused-fixture-fpcalc".into()),
            http: Client::new(),
            entities: Mutex::default(),
            fingerprints: Mutex::default(),
        };
        let track = IndexedTrack {
            id: TrackId::new(1)?,
            path: LibraryPath::parse("album/song.mp3")?,
            metadata: TrackMetadata {
                release_date: String::new(),
                original_release_date: String::new(),
                composer: String::new(),
                title: "Song".into(),
                artist: "Artist feat. Guest".into(),
                album_artist: String::new(),
                album: String::new(),
                track_no: None,
                disc_no: None,
                year: None,
                genre: String::new(),
                bpm: None,
            },
            duration: Duration::from_secs(120),
            display_title: String::new(),
            origin: String::new(),
            size_bytes: 0,
            mtime_unix_seconds: 0,
            added_at_unix_seconds: 0,
        };
        assert!(connector.search_metadata(&track).await.is_err());
        assert_eq!(connector.search_metadata(&track).await?.len(), 1);
        assert_eq!(connector.search_metadata(&track).await?.len(), 1);
        for index in 0..2 {
            assert_eq!(connector.search_isrc("USABC2612345").await?.len(), 1);
            assert_eq!(connector.search_artists("作曲家").await?.len(), 1);
            if index == 0 {
                assert!(connector.artist(ARTIST).await.is_err());
            }
            assert_eq!(connector.artist(ARTIST).await?.aliases, ["作曲家"]);
            assert_eq!(
                connector
                    .search_artist_recordings(&track, ARTIST)
                    .await?
                    .len(),
                1
            );
        }
        assert_eq!(state.requests.lock().await.len(), 7);
        connector.begin_lookup(false).await?;
        connector.search_metadata(&track).await?;
        assert_eq!(state.requests.lock().await.len(), 7);
        connector.begin_lookup(true).await?;
        connector.search_metadata(&track).await?;
        let requests = state.requests.lock().await;
        assert_eq!(requests.len(), 8);
        assert_eq!(requests[3].1.get("limit").map(String::as_str), Some("10"));
        assert_eq!(
            requests[4].1.get("inc").map(String::as_str),
            Some("aliases")
        );
        assert!(requests[6].1["query"].contains(&format!("arid:\"{ARTIST}\"")));
        assert_eq!(requests[6].1.get("limit").map(String::as_str), Some("25"));
        server.abort();
        assert!(server.await.is_err_and(|error| error.is_cancelled()));
        Ok(())
    }
}
