use super::*;

const OTHER_RELEASE: &str = "00000000-0000-0000-0000-000000000097";

fn candidate(id: i64, title: &str, artist: &str, releases: &[&str]) -> Candidate {
    Candidate {
        id: format!("00000000-0000-0000-0000-{id:012}"),
        title: title.into(),
        artist: artist.into(),
        length_ms: Some(120_000),
        releases: releases
            .iter()
            .map(|id| ReleaseSummary {
                id: (*id).into(),
                ..release_summary()
            })
            .collect(),
        provider_score: 1.0,
    }
}

async fn add_sibling(
    storage: &SqliteStorage,
    connector: &FixtureCatalog,
    id: i64,
    path: &str,
    title: &str,
    artist: &str,
    releases: &[&str],
) -> TestResult {
    sqlx::query("INSERT INTO tracks (id, path, title, artist, album_artist, album, track_no, disc_no, year, genre, length_s, bpm, size_bytes, mtime, added_at, display_title, origin) VALUES (?, ?, ?, ?, '', 'Album', ?, 1, 2026, '', 120.0, NULL, 10, 20, CURRENT_TIMESTAMP, '', '')")
        .bind(id).bind(path).bind(title).bind(artist).bind(id).execute(&storage.pool).await?;
    connector
        .sibling_candidates
        .lock()
        .await
        .insert(id, vec![candidate(id, title, artist, releases)]);
    Ok(())
}

async fn configure(connector: &FixtureCatalog) -> TestResult {
    connector.sources.update("acoustid", false).await?;
    connector.sources.update("lastfm", false).await?;
    connector.metadata_match.store(false, Ordering::SeqCst);
    connector.album_match.store(true, Ordering::SeqCst);
    connector
        .album_requires_release
        .store(true, Ordering::SeqCst);
    Ok(())
}

async fn selected(service: &JobService, force: bool) -> TestResult<Value> {
    result(
        &run_parameters(
            service,
            json!({"scope":{"type":"tracks", "track_ids":[1]}, "force":force}),
        )
        .await?,
    )
}

#[tokio::test]
async fn unselected_siblings_recover_candidates_and_raw_tag_changes_expire_cache() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    configure(&connector).await?;
    add_sibling(
        &storage,
        &connector,
        2,
        "album/02.mp3",
        "Opening",
        "Artist",
        &[RELEASE],
    )
    .await?;
    add_sibling(
        &storage,
        &connector,
        3,
        "album/03.mp3",
        "Finale",
        "Artist",
        &[RELEASE],
    )
    .await?;
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    let recovered = selected(&coordinator.service, false).await?;
    assert_eq!(recovered["identified"], 1);
    assert_eq!(recovered["plans"].as_array().ok_or("plans")?.len(), 1);
    assert_eq!(recovered["plans"][0]["track_id"], 1);
    assert_eq!(
        recovered["plans"][0]["identity"]["recording_mbid"],
        RECORDING
    );
    assert_eq!(
        connector.last_release_scope.lock().await.as_deref(),
        Some(RELEASE)
    );
    assert_eq!(connector.sibling_queries.lock().await.as_slice(), &[2, 3]);
    assert!(
        recovered["plans"][0]["notes"]
            .as_array()
            .ok_or("notes")?
            .iter()
            .any(|note| note
                .as_str()
                .is_some_and(|text| text.contains("independently titled")))
    );
    assert!(
        recovered["plans"][0]["ops"]
            .as_array()
            .ok_or("ops")?
            .iter()
            .all(|op| op["confidence"] == "low")
    );
    let untouched: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracks WHERE album_artist = ''")
        .fetch_one(&storage.pool)
        .await?;
    assert_eq!(untouched, 3);
    assert_eq!(selected(&coordinator.service, false).await?["cached"], 1);
    // The local hypothesis can still infer Artist from unanimous neighbors.
    // Discovery must use the changed raw tags, not its own inferred evidence.
    sqlx::query("UPDATE tracks SET artist = '' WHERE id = 3")
        .execute(&storage.pool)
        .await?;
    let changed = selected(&coordinator.service, false).await?;
    assert_eq!(changed["cached"], 0);
    assert_eq!(changed["unmatched"], 1);
    assert_eq!(connector.sibling_queries.lock().await.as_slice(), &[2, 3]);
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}

#[tokio::test]
async fn siblings_require_distinct_songs_and_coherent_folders_with_bounded_queries() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    configure(&connector).await?;
    add_sibling(
        &storage,
        &connector,
        2,
        "album/02.mp3",
        "Opening",
        "Composer A",
        &[RELEASE],
    )
    .await?;
    add_sibling(
        &storage,
        &connector,
        3,
        "album/03.mp3",
        "Opening",
        "Composer B",
        &[RELEASE],
    )
    .await?;
    add_sibling(
        &storage,
        &connector,
        4,
        "other/04.mp3",
        "Finale",
        "Composer C",
        &[RELEASE],
    )
    .await?;
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    assert_eq!(selected(&coordinator.service, true).await?["unmatched"], 1);
    assert!(connector.sibling_queries.lock().await.is_empty());
    sqlx::query("UPDATE tracks SET path = 'album/04.mp3' WHERE id = 4")
        .execute(&storage.pool)
        .await?;
    assert_eq!(
        selected(&coordinator.service, false).await?["identified"],
        1
    );
    assert_eq!(connector.sibling_queries.lock().await.as_slice(), &[2, 4]);
    sqlx::query("UPDATE tracks SET artist = '' WHERE id = 1")
        .execute(&storage.pool)
        .await?;
    let missing_artist = selected(&coordinator.service, true).await?;
    assert_eq!(missing_artist["unmatched"], 1);
    assert_eq!(
        missing_artist["plans"][0]["candidates"]
            .as_array()
            .ok_or("candidates")?
            .len(),
        1
    );
    assert_eq!(missing_artist["plans"][0]["ops"], json!([]));
    sqlx::query("UPDATE tracks SET artist = 'Artist' WHERE id = 1")
        .execute(&storage.pool)
        .await?;
    // Compilation artists are allowed; conflicting nonempty album tags are not.
    sqlx::query("UPDATE tracks SET album = 'Different Soundtrack' WHERE id = 3")
        .execute(&storage.pool)
        .await?;
    connector.sibling_queries.lock().await.clear();
    assert_eq!(selected(&coordinator.service, false).await?["unmatched"], 1);
    assert!(connector.sibling_queries.lock().await.is_empty());
    sqlx::query("UPDATE tracks SET album = 'Album' WHERE id = 3")
        .execute(&storage.pool)
        .await?;
    for id in 5..=7 {
        add_sibling(
            &storage,
            &connector,
            id,
            &format!("album/{id:02}.mp3"),
            &format!("Chapter {id}"),
            "Composer D",
            &[RELEASE],
        )
        .await?;
    }
    assert_eq!(
        selected(&coordinator.service, false).await?["identified"],
        1
    );
    assert_eq!(
        connector.sibling_queries.lock().await.as_slice(),
        &[2, 4, 5]
    );
    // Both copies resolving to the same recording do not form independent anchors.
    connector
        .sibling_candidates
        .lock()
        .await
        .insert(4, vec![candidate(2, "Finale", "Composer C", &[RELEASE])]);
    connector
        .sibling_candidates
        .lock()
        .await
        .insert(5, Vec::new());
    assert_eq!(selected(&coordinator.service, true).await?["unmatched"], 1);
    sqlx::query("UPDATE tracks SET path = REPLACE(path, 'album/', '')")
        .execute(&storage.pool)
        .await?;
    connector.sibling_queries.lock().await.clear();
    assert_eq!(selected(&coordinator.service, false).await?["unmatched"], 1);
    assert!(connector.sibling_queries.lock().await.is_empty());
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}

#[tokio::test]
async fn sibling_release_discovery_preserves_competitors_and_abstains_on_incomplete_evidence()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    configure(&connector).await?;
    for (id, title) in [(2, "Opening"), (3, "Finale"), (4, "Credits")] {
        add_sibling(
            &storage,
            &connector,
            id,
            &format!("album/{id:02}.mp3"),
            title,
            "Artist",
            &[RELEASE, OTHER_RELEASE],
        )
        .await?;
    }
    connector.scoped_candidates.lock().await.insert(
        OTHER_RELEASE.into(),
        vec![candidate(8, "Song", "Artist", &[OTHER_RELEASE])],
    );
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    let ambiguous = selected(&coordinator.service, false).await?;
    assert_eq!(ambiguous["unmatched"], 1);
    assert_eq!(
        ambiguous["plans"][0]["candidates"]
            .as_array()
            .ok_or("candidates")?
            .len(),
        2
    );
    // An unavailable edition must not make its competitor win by omission.
    connector
        .scoped_failures
        .lock()
        .await
        .insert(OTHER_RELEASE.into());
    let failed = selected(&coordinator.service, true).await?;
    assert_eq!(failed["unmatched"], 1);
    assert_eq!(failed["plans"][0]["partial"], true);
    assert_eq!(selected(&coordinator.service, false).await?["cached"], 1); // earlier complete ambiguity
    connector.scoped_failures.lock().await.clear();
    connector.scoped_candidates.lock().await.clear();
    connector.sibling_failures.lock().await.insert(4);
    let failed_anchor = selected(&coordinator.service, true).await?;
    assert_eq!(failed_anchor["unmatched"], 1);
    assert_eq!(failed_anchor["plans"][0]["partial"], true);
    assert_eq!(connector.last_release_scope.lock().await.as_deref(), None);
    connector.sibling_failures.lock().await.clear();
    // A third successful anchor contradicting the first two vetoes agreement.
    connector.sibling_candidates.lock().await.insert(
        4,
        vec![candidate(
            4,
            "Credits",
            "Artist",
            &["00000000-0000-0000-0000-000000000096"],
        )],
    );
    assert_eq!(selected(&coordinator.service, true).await?["unmatched"], 1);
    assert_eq!(connector.last_release_scope.lock().await.as_deref(), None);
    // A large intersection is withheld rather than choosing arbitrary editions.
    for (id, title) in [(2, "Opening"), (3, "Finale"), (4, "Credits")] {
        connector.sibling_candidates.lock().await.insert(
            id,
            vec![candidate(
                id,
                title,
                "Artist",
                &[
                    RELEASE,
                    OTHER_RELEASE,
                    "00000000-0000-0000-0000-000000000096",
                ],
            )],
        );
    }
    let too_many = selected(&coordinator.service, true).await?;
    assert_eq!(too_many["unmatched"], 1);
    assert_eq!(connector.last_release_scope.lock().await.as_deref(), None);
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}

#[tokio::test]
async fn explicit_release_evidence_bypasses_siblings_and_discovery_never_pins_an_edition()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    configure(&connector).await?;
    add_sibling(
        &storage,
        &connector,
        2,
        "album/02.mp3",
        "Opening",
        "Artist",
        &[RELEASE],
    )
    .await?;
    add_sibling(
        &storage,
        &connector,
        3,
        "album/03.mp3",
        "Finale",
        "Artist",
        &[RELEASE],
    )
    .await?;
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    let parameters = json!({"scope":{"type":"tracks", "track_ids":[1]}, "imports":[{"track_id":1,"fields":{"release_mbid":RELEASE}}]});
    assert_eq!(
        result(&run_parameters(&coordinator.service, parameters).await?)?["identified"],
        1
    );
    assert!(connector.sibling_queries.lock().await.is_empty());
    // A shared release hint cannot settle the target recording's edition ambiguity.
    connector.multiple_editions.store(true, Ordering::SeqCst);
    let discovered = selected(&coordinator.service, true).await?;
    assert_eq!(discovered["identified"], 1);
    assert!(discovered["plans"][0]["identity"]["release_mbid"].is_null());
    assert_eq!(
        discovered["plans"][0]["release_choices"]
            .as_array()
            .ok_or("editions")?
            .len(),
        2
    );
    assert_eq!(discovered["plans"][0]["ops"], json!([]));
    // Final recording details still veto a live/version mismatch after discovery.
    connector.recording_conflict.store(true, Ordering::SeqCst);
    let contradicted = selected(&coordinator.service, true).await?;
    assert_eq!(contradicted["unmatched"], 1);
    assert_eq!(contradicted["plans"][0]["candidates"], json!([]));
    assert_eq!(contradicted["plans"][0]["ops"], json!([]));
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}
