use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use music_application::assistant::{
    AssistantService, ProviderCredentialError, ProviderCredentialFuture, ProviderCredentialSource,
};
use music_application::cleanup::CleanupService;
use music_application::cleanup_enrichment::catalog::*;
use music_application::cleanup_enrichment::{
    CLEANUP_ENRICHMENT_JOB_KIND, CleanupEnrichmentJobHandler, CleanupEnrichmentRepository,
    CleanupEnrichmentServices,
};
use music_application::cleanup_sources::{
    CleanupSourceError, CleanupSourceRuntime, CleanupSourceService,
};
use music_application::jobs::{JobRecord, JobService, JobStatus, start_job_coordinator};
use music_domain::{IndexedTrack, TrackId};
use serde_json::{Value, json};

use crate::{SqliteStorage, SqliteStorageOptions};

#[path = "cleanup_enrichment_workflow_tests/artist_aliases.rs"]
mod artist_aliases;
#[path = "cleanup_enrichment_workflow_tests/model_review.rs"]
mod model_review;
#[path = "cleanup_enrichment_workflow_tests/sibling_discovery.rs"]
mod sibling_discovery;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
const RECORDING: &str = "00000000-0000-0000-0000-000000000001";
const RELEASE: &str = "00000000-0000-0000-0000-000000000099";

#[derive(Debug)]
struct NoSavedCredentials;
impl ProviderCredentialSource for NoSavedCredentials {
    fn current_cipher(&self) -> ProviderCredentialFuture<'_> {
        Box::pin(async {
            Err(ProviderCredentialError {
                code: "fixture_has_no_saved_credentials".to_owned(),
            })
        })
    }
}

#[derive(Debug)]
struct FixtureCatalog {
    sources: Arc<CleanupSourceService>,
    searches: AtomicUsize,
    fingerprints: AtomicUsize,
    tag_calls: AtomicUsize,
    tag_failure: AtomicBool,
    metadata_match: AtomicBool,
    metadata_failure: AtomicBool,
    ambiguous_fingerprint: AtomicBool,
    release_failure: AtomicBool,
    album_searches: AtomicUsize,
    album_match: AtomicBool,
    album_failure: AtomicBool,
    album_competitor: AtomicBool,
    last_release_scope: tokio::sync::Mutex<Option<String>>,
    lookup_order: tokio::sync::Mutex<Vec<&'static str>>,
    recording_conflict: AtomicBool,
    sibling_candidates: tokio::sync::Mutex<std::collections::BTreeMap<i64, Vec<Candidate>>>,
    sibling_failures: tokio::sync::Mutex<std::collections::BTreeSet<i64>>,
    sibling_queries: tokio::sync::Mutex<Vec<i64>>,
    album_requires_release: AtomicBool,
    scoped_candidates: tokio::sync::Mutex<std::collections::BTreeMap<String, Vec<Candidate>>>,
    scoped_failures: tokio::sync::Mutex<std::collections::BTreeSet<String>>,
    multiple_editions: AtomicBool,
    artist_hits: tokio::sync::Mutex<std::collections::BTreeMap<String, Vec<Artist>>>,
    artist_details: tokio::sync::Mutex<std::collections::BTreeMap<String, Artist>>,
    artist_recordings: tokio::sync::Mutex<std::collections::BTreeMap<String, Vec<Candidate>>>,
    artist_requests: tokio::sync::Mutex<Vec<String>>,
    artist_failures: tokio::sync::Mutex<std::collections::BTreeSet<String>>,
}

impl CatalogConnector for FixtureCatalog {
    fn search_artists<'a>(&'a self, name: &'a str) -> CatalogFuture<'a, Vec<Artist>> {
        Box::pin(async move {
            let key = format!("name:{name}");
            self.artist_requests.lock().await.push(key.clone());
            if self.artist_failures.lock().await.contains(&key) {
                return Err(CatalogError::MusicBrainz);
            }
            Ok(self
                .artist_hits
                .lock()
                .await
                .get(name)
                .cloned()
                .unwrap_or_default())
        })
    }
    fn artist<'a>(&'a self, artist_id: &'a str) -> CatalogFuture<'a, Artist> {
        Box::pin(async move {
            self.artist_requests
                .lock()
                .await
                .push(format!("artist:{artist_id}"));
            self.artist_details
                .lock()
                .await
                .get(artist_id)
                .cloned()
                .ok_or(CatalogError::MusicBrainz)
        })
    }
    fn search_artist_recordings<'a>(
        &'a self,
        track: &'a IndexedTrack,
        artist_id: &'a str,
    ) -> CatalogFuture<'a, Vec<Candidate>> {
        Box::pin(async move {
            let key = format!("recordings:{artist_id}");
            self.artist_requests.lock().await.push(key.clone());
            assert!(!track.metadata.title.is_empty());
            if self.artist_failures.lock().await.contains(&key) {
                return Err(CatalogError::MusicBrainz);
            }
            Ok(self
                .artist_recordings
                .lock()
                .await
                .get(artist_id)
                .cloned()
                .unwrap_or_default())
        })
    }
    fn search_album_metadata<'a>(
        &'a self,
        _: &'a IndexedTrack,
        release_id: Option<&'a str>,
    ) -> CatalogFuture<'a, Vec<Candidate>> {
        Box::pin(async move {
            self.lookup_order.lock().await.push("album");
            self.album_searches.fetch_add(1, Ordering::SeqCst);
            *self.last_release_scope.lock().await = release_id.map(str::to_owned);
            if let Some(id) = release_id {
                if self.scoped_failures.lock().await.contains(id) {
                    return Err(CatalogError::MusicBrainz);
                }
                if let Some(candidates) = self.scoped_candidates.lock().await.get(id) {
                    return Ok(candidates.clone());
                }
            } else if self.album_requires_release.load(Ordering::SeqCst) {
                return Ok(Vec::new());
            }
            if self.album_failure.load(Ordering::SeqCst) {
                return Err(CatalogError::MusicBrainz);
            }
            let mut candidates = Vec::new();
            if self.album_match.load(Ordering::SeqCst) {
                let candidate = Candidate {
                    id: RECORDING.into(),
                    title: "Song".into(),
                    artist: "Artist".into(),
                    length_ms: Some(120_000),
                    releases: vec![release_summary()],
                    provider_score: 1.0,
                };
                candidates.push(candidate.clone());
                if self.album_competitor.load(Ordering::SeqCst) {
                    candidates.push(Candidate {
                        id: "00000000-0000-0000-0000-000000000002".into(),
                        ..candidate
                    });
                }
            }
            Ok(candidates)
        })
    }
    fn runtime_credential(&self, _: CatalogCredentialSource) -> Option<&str> {
        Some("synthetic-fixture")
    }

    fn search_metadata<'a>(&'a self, track: &'a IndexedTrack) -> CatalogFuture<'a, Vec<Candidate>> {
        Box::pin(async move {
            self.lookup_order.lock().await.push("artist");
            self.searches.fetch_add(1, Ordering::SeqCst);
            if track.id.get() != 1 {
                self.sibling_queries.lock().await.push(track.id.get());
                if self.sibling_failures.lock().await.contains(&track.id.get()) {
                    return Err(CatalogError::MusicBrainz);
                }
                return Ok(self
                    .sibling_candidates
                    .lock()
                    .await
                    .get(&track.id.get())
                    .cloned()
                    .unwrap_or_default());
            }
            if self.metadata_failure.load(Ordering::SeqCst) {
                return Err(CatalogError::MusicBrainzFailure(
                    CatalogFailure::HttpStatus(429),
                ));
            }
            assert_eq!(
                self.sources.update("musicbrainz", false).await,
                Err(CleanupSourceError::Busy)
            );
            Ok(if self.metadata_match.load(Ordering::SeqCst) {
                vec![Candidate {
                    id: RECORDING.to_owned(),
                    title: "Song".to_owned(),
                    artist: "Artist".to_owned(),
                    length_ms: Some(120_000),
                    releases: vec![release_summary()],
                    provider_score: 1.0,
                }]
            } else {
                vec![]
            })
        })
    }

    fn recording<'a>(&'a self, id: &'a str) -> CatalogFuture<'a, Recording> {
        Box::pin(async move {
            assert_eq!(id, RECORDING);
            let mut releases = vec![release_summary()];
            if self.multiple_editions.load(Ordering::SeqCst) {
                releases.push(ReleaseSummary {
                    id: "00000000-0000-0000-0000-000000000097".into(),
                    ..release_summary()
                });
            }
            Ok(Recording {
                title: if self.recording_conflict.load(Ordering::SeqCst) {
                    "Song (live)"
                } else {
                    "Song"
                }
                .to_owned(),
                artist: "Artist".to_owned(),
                first_release_date: Some("2026".to_owned()),
                releases,
                releases_complete: true,
                ..Recording::default()
            })
        })
    }

    fn release<'a>(&'a self, release_id: &'a str, _: &'a str) -> CatalogFuture<'a, ReleaseDetail> {
        Box::pin(async move {
            if self.release_failure.load(Ordering::SeqCst) {
                return Err(CatalogError::MusicBrainzFailure(
                    CatalogFailure::HttpStatus(503),
                ));
            }
            Ok(ReleaseDetail {
                id: release_id.to_owned(),
                title: "Album".to_owned(),
                artist: "Artist".to_owned(),
                date: Some("2026".to_owned()),
                track_no: Some(1),
                disc_no: Some(1),
                slots: vec![
                    music_application::cleanup_enrichment::catalog::ReleaseSlot {
                        id: "00000000-0000-0000-0000-000000000098".into(),
                        recording_id: "00000000-0000-0000-0000-000000000001".into(),
                        title: "Song".into(),
                        artist: "Artist".into(),
                        length_ms: Some(120_000),
                        track_no: Some(1),
                        disc_no: Some(1),
                    },
                ],
                ..ReleaseDetail::default()
            })
        })
    }

    fn fingerprint_candidates<'a>(
        &'a self,
        _: &'a IndexedTrack,
        _: &'a str,
    ) -> CatalogFuture<'a, Vec<AcousticCandidate>> {
        Box::pin(async move {
            self.fingerprints.fetch_add(1, Ordering::SeqCst);
            let mut recording_ids = vec![RECORDING.to_owned()];
            if self.ambiguous_fingerprint.load(Ordering::SeqCst) {
                recording_ids.push("other-recording".to_owned());
            }
            Ok(vec![AcousticCandidate {
                recording_ids,
                score: 0.99,
            }])
        })
    }

    fn community_tags<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a str,
    ) -> CatalogFuture<'a, Vec<CommunityTag>> {
        Box::pin(async move {
            self.tag_calls.fetch_add(1, Ordering::SeqCst);
            if self.tag_failure.load(Ordering::SeqCst) {
                return Err(CatalogError::LastFmFailure(CatalogFailure::ProviderCode(
                    26,
                )));
            }
            Ok(vec![
                CommunityTag {
                    name: "dark".to_owned(),
                    count: 80,
                },
                CommunityTag {
                    name: "invented concept".to_owned(),
                    count: 999,
                },
            ])
        })
    }
}

fn release_summary() -> ReleaseSummary {
    ReleaseSummary {
        id: RELEASE.to_owned(),
        title: "Album".to_owned(),
        status: Some("Official".to_owned()),
    }
}

async fn setup(
    storage: Arc<SqliteStorage>,
) -> TestResult<(Arc<FixtureCatalog>, CleanupEnrichmentJobHandler)> {
    sqlx::query("INSERT INTO tracks (path, title, artist, album_artist, album, track_no, disc_no, year, genre, length_s, bpm, size_bytes, mtime, added_at, display_title, origin) VALUES ('album/song.mp3', 'Song', 'Artist', '', 'Album', 1, 1, 2026, '', 120.0, NULL, 10, 20, CURRENT_TIMESTAMP, '', '')")
        .execute(&storage.pool).await?;
    let sources = Arc::new(CleanupSourceService::new(
        storage.clone(),
        Arc::new(NoSavedCredentials),
        CleanupSourceRuntime {
            acoustid_configured: true,
            fpcalc_available: true,
            lastfm_configured: true,
        },
    ));
    sources.update("lastfm", true).await?;
    sources.update("acoustid", true).await?;
    let connector = Arc::new(FixtureCatalog {
        sources: sources.clone(),
        searches: AtomicUsize::new(0),
        fingerprints: AtomicUsize::new(0),
        tag_calls: AtomicUsize::new(0),
        tag_failure: AtomicBool::new(false),
        metadata_match: AtomicBool::new(true),
        metadata_failure: AtomicBool::new(false),
        ambiguous_fingerprint: AtomicBool::new(false),
        release_failure: AtomicBool::new(false),
        album_searches: AtomicUsize::new(0),
        album_match: AtomicBool::new(false),
        album_failure: AtomicBool::new(false),
        album_competitor: AtomicBool::new(false),
        last_release_scope: tokio::sync::Mutex::new(None),
        lookup_order: tokio::sync::Mutex::new(Vec::new()),
        recording_conflict: AtomicBool::new(false),
        sibling_candidates: tokio::sync::Mutex::default(),
        sibling_failures: tokio::sync::Mutex::default(),
        sibling_queries: tokio::sync::Mutex::default(),
        album_requires_release: AtomicBool::new(false),
        scoped_candidates: tokio::sync::Mutex::default(),
        scoped_failures: tokio::sync::Mutex::default(),
        multiple_editions: AtomicBool::new(false),
        artist_hits: tokio::sync::Mutex::default(),
        artist_details: tokio::sync::Mutex::default(),
        artist_recordings: tokio::sync::Mutex::default(),
        artist_requests: tokio::sync::Mutex::default(),
        artist_failures: tokio::sync::Mutex::default(),
    });
    let handler = CleanupEnrichmentJobHandler::new(
        CleanupEnrichmentServices {
            cleanup: Arc::new(CleanupService::new(storage.clone())),
            cache: storage.clone(),
            analyses: storage.clone(),
            assistant: Arc::new(AssistantService::new(storage)),
            sources,
        },
        connector.clone(),
    );
    Ok((connector, handler))
}

async fn run(service: &JobService, force: bool) -> TestResult<JobRecord> {
    run_parameters(service, json!({"scope": {"type": "all"}, "force": force})).await
}

async fn run_parameters(service: &JobService, parameters: Value) -> TestResult<JobRecord> {
    let job = service
        .enqueue(CLEANUP_ENRICHMENT_JOB_KIND, parameters)
        .await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let current = service.get(&job.id).await?.ok_or("job disappeared")?;
            if matches!(
                current.status,
                JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled
            ) {
                return Ok(current);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?
}

fn result(job: &JobRecord) -> TestResult<Value> {
    assert_eq!(job.status, JobStatus::Succeeded, "{:?}", job.error);
    Ok(Value::Object(job.result.clone().ok_or("missing result")?))
}

#[tokio::test]
async fn catalog_workflow_reuses_complete_cache_but_preserves_review_and_source_locks() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    let first = result(&run(&coordinator.service, false).await?)?;
    assert_eq!(first["identified"], 1);
    assert_eq!(first["plans"][0]["tag_suggestions"][0]["tag"], "dark");
    assert_eq!(connector.fingerprints.load(Ordering::SeqCst), 0);
    assert_eq!(connector.album_searches.load(Ordering::SeqCst), 0);
    let second = result(&run(&coordinator.service, false).await?)?;
    assert_eq!(second["cached"], 1);
    assert_eq!(connector.searches.load(Ordering::SeqCst), 1);
    assert_eq!(connector.tag_calls.load(Ordering::SeqCst), 1);
    let manual: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM track_user_tags")
        .fetch_one(&storage.pool)
        .await?;
    let proposals: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM track_analyses WHERE analyzer_id = 'catalog-tags/v1'",
    )
    .fetch_one(&storage.pool)
    .await?;
    assert_eq!(manual, 0);
    assert_eq!(proposals, 1);
    assert_eq!(
        result(&run(&coordinator.service, true).await?)?["cached"],
        0
    );
    assert_eq!(connector.searches.load(Ordering::SeqCst), 2);
    connector.sources.update("musicbrainz", false).await?;
    assert!(
        storage
            .cleanup_enrichment(TrackId::new(1)?)
            .await?
            .is_none()
    );
    assert_eq!(
        run(&coordinator.service, false).await?.status,
        JobStatus::Failed
    );
    assert_eq!(connector.searches.load(Ordering::SeqCst), 2);
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}

#[tokio::test]
async fn catalog_workflow_bounds_fallback_and_retries_partial_results_on_explicit_runs()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    connector.metadata_match.store(false, Ordering::SeqCst);
    connector
        .ambiguous_fingerprint
        .store(true, Ordering::SeqCst);
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    let unmatched = result(&run(&coordinator.service, false).await?)?;
    assert_eq!(unmatched["unmatched"], 1);
    assert_eq!(connector.fingerprints.load(Ordering::SeqCst), 1);
    assert_eq!(connector.tag_calls.load(Ordering::SeqCst), 0);
    connector
        .ambiguous_fingerprint
        .store(false, Ordering::SeqCst);
    connector.release_failure.store(true, Ordering::SeqCst);
    connector.sources.update("lastfm", false).await?;
    let partial = result(&run(&coordinator.service, false).await?)?;
    assert_eq!(partial["fingerprinted"], 1);
    assert_eq!(partial["plans"][0]["partial"], true);
    assert!(
        storage
            .cleanup_enrichment(TrackId::new(1)?)
            .await?
            .is_none()
    );
    let again = result(&run(&coordinator.service, false).await?)?;
    assert_eq!(again["cached"], 0);
    assert_eq!(connector.fingerprints.load(Ordering::SeqCst), 3);
    assert_eq!(connector.tag_calls.load(Ordering::SeqCst), 0);
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}

#[tokio::test]
async fn catalog_failures_retain_safe_diagnostics_without_losing_identity_or_caching_partial_results()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    connector.metadata_failure.store(true, Ordering::SeqCst);
    connector.release_failure.store(true, Ordering::SeqCst);
    connector.tag_failure.store(true, Ordering::SeqCst);
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    let result = result(&run(&coordinator.service, false).await?)?;
    assert_eq!(result["fingerprinted"], 1);
    assert_eq!(result["plans"][0]["partial"], true);
    assert_eq!(result["plans"][0]["identity"]["recording_mbid"], RECORDING);
    let notes = result["plans"][0]["notes"]
        .as_array()
        .ok_or("missing notes")?;
    for detail in [
        "MusicBrainz: HTTP 429",
        "MusicBrainz: HTTP 503",
        "Last.fm: provider error code 26",
    ] {
        assert!(
            notes
                .iter()
                .any(|note| note.as_str().is_some_and(|note| note.contains(detail)))
        );
    }
    assert_eq!(connector.tag_calls.load(Ordering::SeqCst), 1);
    assert_eq!(connector.fingerprints.load(Ordering::SeqCst), 1);
    assert!(
        storage
            .cleanup_enrichment(TrackId::new(1)?)
            .await?
            .is_none()
    );
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}

#[tokio::test]
async fn imports_prefer_typed_ids_invalidate_cache_and_reject_out_of_scope_before_requests()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    assert_eq!(
        result(&run(&coordinator.service, false).await?)?["identified"],
        1
    );
    let imports = json!({"scope":{"type":"all"}, "imports":[{"track_id":1,"fields":{"recording_mbid":RECORDING, "date":"2026-09-10"}}]});
    let imported = result(&run_parameters(&coordinator.service, imports.clone()).await?)?;
    assert_eq!(imported["cached"], 0);
    assert_eq!(imported["plans"][0]["identity"]["method"], "identifier");
    assert_eq!(connector.searches.load(Ordering::SeqCst), 1);
    assert_eq!(
        result(&run_parameters(&coordinator.service, imports).await?)?["cached"],
        1
    );
    let invalid = json!({"scope":{"type":"all"}, "imports":[{"track_id":9,"fields":{"recording_mbid":RECORDING}}]});
    assert_eq!(
        run_parameters(&coordinator.service, invalid).await?.status,
        JobStatus::Failed
    );
    assert_eq!(connector.searches.load(Ordering::SeqCst), 1);
    connector.metadata_failure.store(true, Ordering::SeqCst);
    connector
        .ambiguous_fingerprint
        .store(false, Ordering::SeqCst);
    let fallback = result(&run(&coordinator.service, true).await?)?;
    assert_eq!(fallback["fingerprinted"], 1);
    assert_eq!(fallback["plans"][0]["partial"], true);
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}

#[tokio::test]
async fn album_retrieval_preserves_unknown_artists_competitors_and_review_only_writes() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    connector.metadata_match.store(false, Ordering::SeqCst);
    connector.album_match.store(true, Ordering::SeqCst);
    connector.sources.update("acoustid", false).await?;
    connector.sources.update("lastfm", false).await?;
    sqlx::query("UPDATE tracks SET artist = ''")
        .execute(&storage.pool)
        .await?;
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    let unknown = result(&run(&coordinator.service, false).await?)?;
    assert_eq!(unknown["unmatched"], 1);
    assert_eq!(unknown["plans"][0]["candidates"][0]["id"], RECORDING);
    assert_eq!(unknown["plans"][0]["ops"], json!([]));
    assert_eq!(connector.album_searches.load(Ordering::SeqCst), 1);
    assert!(
        unknown["plans"][0]["notes"]
            .as_array()
            .ok_or("missing notes")?
            .iter()
            .any(|n| n.as_str().is_some_and(|n| n.contains("album title")))
    );
    assert_eq!(
        result(&run(&coordinator.service, false).await?)?["cached"],
        1
    );
    assert_eq!(connector.album_searches.load(Ordering::SeqCst), 1);
    let indexed_artist: String = sqlx::query_scalar("SELECT artist FROM tracks")
        .fetch_one(&storage.pool)
        .await?;
    assert!(indexed_artist.is_empty());

    // Album lookup can recover a match outside ordinary search's shortlist,
    // but the artist/title/duration criteria remain exactly the same.
    sqlx::query("UPDATE tracks SET artist = 'Artist'")
        .execute(&storage.pool)
        .await?;
    let identified = result(&run(&coordinator.service, false).await?)?;
    assert_eq!(identified["identified"], 1);
    assert_eq!(
        identified["plans"][0]["identity"]["recording_mbid"],
        RECORDING
    );
    assert_eq!(identified["plans"][0]["ops"][0]["confidence"], "low");
    let album_artist: String = sqlx::query_scalar("SELECT album_artist FROM tracks")
        .fetch_one(&storage.pool)
        .await?;
    assert!(album_artist.is_empty());
    connector.album_competitor.store(true, Ordering::SeqCst);
    let ambiguous = result(&run(&coordinator.service, true).await?)?;
    assert_eq!(ambiguous["unmatched"], 1);
    assert_eq!(
        ambiguous["plans"][0]["candidates"]
            .as_array()
            .ok_or("missing candidates")?
            .len(),
        2
    );
    connector.album_competitor.store(false, Ordering::SeqCst);
    connector.recording_conflict.store(true, Ordering::SeqCst);
    let contradicted = result(&run(&coordinator.service, true).await?)?;
    assert_eq!(contradicted["unmatched"], 1);
    assert_eq!(contradicted["plans"][0]["candidates"], json!([]));
    assert_eq!(contradicted["plans"][0]["ops"], json!([]));
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}

#[tokio::test]
async fn explicit_release_retrieval_precedes_text_and_partial_failures_do_not_poison_cache()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    connector.metadata_match.store(false, Ordering::SeqCst);
    connector.album_match.store(true, Ordering::SeqCst);
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    let release = RELEASE;
    let parameters = json!({"scope":{"type":"all"}, "imports":[{"track_id":1,"fields":{"release_mbid":release}}]});
    let matched = result(&run_parameters(&coordinator.service, parameters).await?)?;
    assert_eq!(matched["identified"], 1);
    assert_eq!(
        connector.last_release_scope.lock().await.as_deref(),
        Some(release)
    );
    assert_eq!(connector.album_searches.load(Ordering::SeqCst), 1);
    assert_eq!(connector.fingerprints.load(Ordering::SeqCst), 0);
    assert_eq!(
        connector.lookup_order.lock().await.as_slice(),
        &["album", "artist"]
    );
    let complete_cache = storage.cleanup_enrichment(TrackId::new(1)?).await?;
    connector.album_failure.store(true, Ordering::SeqCst);
    let partial = result(&run(&coordinator.service, true).await?)?;
    assert_eq!(partial["fingerprinted"], 1);
    assert_eq!(partial["plans"][0]["partial"], true);
    assert_eq!(
        storage.cleanup_enrichment(TrackId::new(1)?).await?,
        complete_cache
    );
    assert_eq!(
        result(&run(&coordinator.service, false).await?)?["cached"],
        0
    );
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}
