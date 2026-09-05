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

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
const RECORDING: &str = "00000000-0000-0000-0000-000000000001";

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
    metadata_match: AtomicBool,
    ambiguous_fingerprint: AtomicBool,
    release_failure: AtomicBool,
}

impl CatalogConnector for FixtureCatalog {
    fn runtime_credential(&self, _: CatalogCredentialSource) -> Option<&str> {
        Some("synthetic-fixture")
    }

    fn search_metadata<'a>(&'a self, _: &'a IndexedTrack) -> CatalogFuture<'a, Vec<Candidate>> {
        Box::pin(async move {
            self.searches.fetch_add(1, Ordering::SeqCst);
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
            Ok(Recording {
                title: "Song".to_owned(),
                artist: "Artist".to_owned(),
                first_release_date: Some("2026".to_owned()),
                releases: vec![release_summary()],
            })
        })
    }

    fn release<'a>(&'a self, _: &'a str, _: &'a str) -> CatalogFuture<'a, ReleaseDetail> {
        Box::pin(async move {
            if self.release_failure.load(Ordering::SeqCst) {
                return Err(CatalogError::MusicBrainz);
            }
            Ok(ReleaseDetail {
                id: "release".to_owned(),
                title: "Album".to_owned(),
                artist: "Artist".to_owned(),
                date: Some("2026".to_owned()),
                track_no: Some(1),
                disc_no: Some(1),
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
        id: "release".to_owned(),
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
        metadata_match: AtomicBool::new(true),
        ambiguous_fingerprint: AtomicBool::new(false),
        release_failure: AtomicBool::new(false),
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
    let job = service
        .enqueue(
            CLEANUP_ENRICHMENT_JOB_KIND,
            json!({"scope": {"type": "all"}, "force": force}),
        )
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
