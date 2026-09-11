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

#[tokio::test]
async fn disc_siblings_recover_selected_tracks_and_expire_on_neighbor_edits_or_moves() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    configure(&connector).await?;
    sqlx::query("UPDATE tracks SET path = 'Album/Disc 1/song.mp3' WHERE id = 1")
        .execute(&storage.pool)
        .await?;
    for (id, path, title, artist) in [
        (2, "Album/CD_02/opening.mp3", "Opening", "Composer A"),
        (3, "Album/disk-03/finale.mp3", "Finale", "Composer B"),
    ] {
        add_sibling(&storage, &connector, id, path, title, artist, &[RELEASE]).await?;
        sqlx::query("UPDATE tracks SET disc_no = ? WHERE id = ?")
            .bind(id)
            .bind(id)
            .execute(&storage.pool)
            .await?;
    }
    connector.multiple_editions.store(true, Ordering::SeqCst);
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    let recovered = selected(&coordinator.service, false).await?;
    assert_eq!(recovered["identified"], 1);
    assert_eq!(recovered["plans"].as_array().ok_or("plans")?.len(), 1);
    assert_eq!(recovered["plans"][0]["track_id"], 1);
    assert!(recovered["plans"][0]["identity"]["release_mbid"].is_null());
    assert_eq!(
        recovered["plans"][0]["release_choices"]
            .as_array()
            .ok_or("choices")?
            .len(),
        2
    );
    assert_eq!(recovered["plans"][0]["ops"], json!([]));
    assert_eq!(connector.sibling_queries.lock().await.as_slice(), &[2, 3]);
    assert_eq!(selected(&coordinator.service, false).await?["cached"], 1);
    sqlx::query("UPDATE tracks SET artist = '' WHERE id = 3")
        .execute(&storage.pool)
        .await?;
    let changed = selected(&coordinator.service, false).await?;
    assert_eq!(changed["cached"], 0);
    assert_eq!(changed["unmatched"], 1);
    sqlx::query("UPDATE tracks SET artist = 'Composer B' WHERE id = 3")
        .execute(&storage.pool)
        .await?;
    assert_eq!(
        selected(&coordinator.service, false).await?["identified"],
        1
    );
    sqlx::query("UPDATE tracks SET path = 'Other/Disc 3/finale.mp3' WHERE id = 3")
        .execute(&storage.pool)
        .await?;
    let moved = selected(&coordinator.service, false).await?;
    assert_eq!(moved["cached"], 0);
    assert_eq!(moved["unmatched"], 1);
    sqlx::query("UPDATE tracks SET path = 'Album/Disc 3/finale.mp3' WHERE id = 3")
        .execute(&storage.pool)
        .await?;
    assert_eq!(
        selected(&coordinator.service, false).await?["identified"],
        1
    );
    let untouched: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracks WHERE album_artist = ''")
        .fetch_one(&storage.pool)
        .await?;
    assert_eq!(untouched, 3);
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}

#[tokio::test]
async fn disc_discovery_withholds_unclear_layouts_and_conflicting_evidence() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    configure(&connector).await?;
    for (id, title) in [(2, "Opening"), (3, "Finale")] {
        add_sibling(
            &storage,
            &connector,
            id,
            &format!("Album/CD2/{id}.mp3"),
            title,
            "Artist",
            &[RELEASE],
        )
        .await?;
    }
    sqlx::query("UPDATE tracks SET disc_no = NULL")
        .execute(&storage.pool)
        .await?;
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    for (target, neighbors) in [
        ("Disc 1", "Disc 2"),      // no album parent
        ("Album", "Album/Disc 2"), // not a disc folder
        ("Album/01", "Album/02"),
        ("Album/Part 1", "Album/Part 2"),
        ("Album/Disc 0", "Album/Disc 2"),
        ("Album/Disc 1", "Album/Disc 2 Bonus"),
        ("Album/Disc 1", "Album/Disc 2/extras"),
        ("Album/Disc 1", "Album/Other Edition/Disc 2"),
        ("Album/Disc 1", "Elsewhere/Disc 2"),
    ] {
        for (id, folder) in [(1, target), (2, neighbors), (3, neighbors)] {
            sqlx::query("UPDATE tracks SET path = ? WHERE id = ?")
                .bind(format!("{folder}/{id}.mp3"))
                .bind(id)
                .execute(&storage.pool)
                .await?;
        }
        let result = selected(&coordinator.service, true).await?;
        assert_eq!(result["unmatched"], 1, "{target} / {neighbors}");
        assert!(connector.sibling_queries.lock().await.is_empty());
    }
    sqlx::query("UPDATE tracks SET path = CASE id WHEN 1 THEN 'Album/Disc 1/1.mp3' ELSE 'Album/Disc 2/' || id || '.mp3' END").execute(&storage.pool).await?;
    assert_eq!(
        selected(&coordinator.service, false).await?["identified"],
        1
    );
    for (mutation, restore, reason) in [
        (
            "UPDATE tracks SET album = 'Different Album' WHERE id = 3",
            "UPDATE tracks SET album = 'Album' WHERE id = 3",
            "album",
        ),
        (
            "UPDATE tracks SET disc_no = 1 WHERE id = 3",
            "UPDATE tracks SET disc_no = NULL WHERE id = 3",
            "disc tag",
        ),
        (
            "UPDATE tracks SET path = 'Album/CD1/3.mp3' WHERE id = 3",
            "UPDATE tracks SET path = 'Album/Disc 2/3.mp3' WHERE id = 3",
            "same disc number",
        ),
    ] {
        connector.sibling_queries.lock().await.clear();
        sqlx::query(mutation).execute(&storage.pool).await?;
        let blocked = selected(&coordinator.service, false).await?;
        assert_eq!(blocked["cached"], 0);
        assert_eq!(blocked["unmatched"], 1);
        assert!(connector.sibling_queries.lock().await.is_empty());
        assert!(
            blocked["plans"][0]["notes"]
                .as_array()
                .ok_or("notes")?
                .iter()
                .any(|note| note.as_str().is_some_and(|s| s.contains(reason)))
        );
        sqlx::query(restore).execute(&storage.pool).await?;
        assert_eq!(
            selected(&coordinator.service, false).await?["identified"],
            1
        );
    }
    connector.sibling_queries.lock().await.clear();
    // Authored tags must not hide conflicting imported observations.
    sqlx::query("UPDATE tracks SET disc_no = 1 WHERE id = 1")
        .execute(&storage.pool)
        .await?;
    for fields in [json!({"disc_no":"3"}), json!({"album":"Different Album"})] {
        let imported = run_parameters(&coordinator.service, json!({"scope":{"type":"tracks", "track_ids":[1]}, "imports":[{"track_id":1,"fields":fields}]})).await?;
        assert_eq!(result(&imported)?["unmatched"], 1);
        assert!(connector.sibling_queries.lock().await.is_empty());
    }
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}

#[tokio::test]
async fn disc_anchors_sample_distinct_folders_within_the_existing_request_bound() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    configure(&connector).await?;
    sqlx::query("UPDATE tracks SET path = 'Album/Disc 2/song.mp3', disc_no = 2 WHERE id = 1")
        .execute(&storage.pool)
        .await?;
    for (id, disc, title) in [
        (2, 2, "Opening"),
        (3, 1, "Opening"),
        (4, 1, "Finale"),
        (5, 1, "Credits"),
        (6, 3, "Scene"),
    ] {
        add_sibling(
            &storage,
            &connector,
            id,
            &format!("Album/Disc {disc}/{id}.mp3"),
            title,
            "Artist",
            &[RELEASE],
        )
        .await?;
        sqlx::query("UPDATE tracks SET disc_no = ? WHERE id = ?")
            .bind(disc)
            .bind(id)
            .execute(&storage.pool)
            .await?;
    }
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    assert_eq!(
        selected(&coordinator.service, false).await?["identified"],
        1
    );
    assert_eq!(
        connector.sibling_queries.lock().await.as_slice(),
        &[2, 4, 6]
    );
    connector
        .sibling_candidates
        .lock()
        .await
        .insert(6, vec![candidate(6, "Scene", "Artist", &[OTHER_RELEASE])]);
    assert_eq!(selected(&coordinator.service, true).await?["unmatched"], 1);
    // Conflicting cross-disc tags still allow corroborated same-folder recovery.
    sqlx::query("UPDATE tracks SET album = 'Other Album' WHERE id = 6")
        .execute(&storage.pool)
        .await?;
    add_sibling(
        &storage,
        &connector,
        7,
        "Album/Disc 2/7.mp3",
        "Ending",
        "Artist",
        &[RELEASE],
    )
    .await?;
    sqlx::query("UPDATE tracks SET disc_no = 2 WHERE id = 7")
        .execute(&storage.pool)
        .await?;
    connector.sibling_queries.lock().await.clear();
    assert_eq!(
        selected(&coordinator.service, false).await?["identified"],
        1
    );
    assert_eq!(connector.sibling_queries.lock().await.as_slice(), &[2, 7]);
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}

#[tokio::test]
async fn disc_groups_above_the_limit_are_withheld_instead_of_silently_truncated() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    configure(&connector).await?;
    sqlx::query("UPDATE tracks SET path = 'Album/Disc 1/song.mp3' WHERE id = 1")
        .execute(&storage.pool)
        .await?;
    for id in 2..=21 {
        add_sibling(
            &storage,
            &connector,
            id,
            &format!("Album/Disc {id}/{id}.mp3"),
            &format!("Chapter {id}"),
            "Artist",
            &[RELEASE],
        )
        .await?;
    }
    sqlx::query("UPDATE tracks SET disc_no = id")
        .execute(&storage.pool)
        .await?;
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    let oversized = selected(&coordinator.service, false).await?;
    assert_eq!(oversized["unmatched"], 1);
    assert!(connector.sibling_queries.lock().await.is_empty());
    sqlx::query("UPDATE tracks SET path = 'Other/Disc 21/21.mp3' WHERE id = 21")
        .execute(&storage.pool)
        .await?;
    let bounded = selected(&coordinator.service, false).await?;
    assert_eq!(bounded["cached"], 0);
    assert_eq!(bounded["identified"], 1);
    assert_eq!(connector.sibling_queries.lock().await.len(), 3);
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}
