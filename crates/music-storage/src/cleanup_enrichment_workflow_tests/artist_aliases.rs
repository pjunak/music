use super::*;

fn artist(index: u8) -> Artist {
    Artist {
        id: format!("10000000-0000-0000-0000-{index:012}"),
        name: "Artist".into(),
        aliases: vec!["作曲家".into()],
        ..Artist::default()
    }
}

fn recording(id: &str) -> Candidate {
    Candidate {
        id: id.into(),
        title: "Song".into(),
        artist: "Artist".into(),
        length_ms: Some(120_000),
        releases: vec![release_summary()],
        provider_score: 1.0,
    }
}

async fn configure(connector: &FixtureCatalog, artists: &[Artist]) -> TestResult {
    connector.sources.update("acoustid", false).await?;
    connector.sources.update("lastfm", false).await?;
    connector.metadata_match.store(false, Ordering::SeqCst);
    for name in ["作曲家", "Artist"] {
        connector
            .artist_hits
            .lock()
            .await
            .insert(name.into(), artists.to_vec());
    }
    for artist in artists {
        connector
            .artist_details
            .lock()
            .await
            .insert(artist.id.clone(), artist.clone());
        connector
            .artist_recordings
            .lock()
            .await
            .insert(artist.id.clone(), vec![recording(RECORDING)]);
    }
    Ok(())
}

#[tokio::test]
async fn catalog_artist_aliases_expand_review_without_replacing_authored_artist() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    configure(&connector, &[artist(1)]).await?;
    sqlx::query("UPDATE tracks SET artist = '作曲家'")
        .execute(&storage.pool)
        .await?;
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    let reviewed = result(&run(&coordinator.service, false).await?)?;
    assert_eq!(reviewed["unmatched"], 1);
    assert_eq!(reviewed["plans"][0]["candidates"][0]["id"], RECORDING);
    assert_eq!(reviewed["plans"][0]["ops"], json!([]));
    assert_eq!(connector.artist_requests.lock().await.len(), 3);
    assert_eq!(
        result(&run(&coordinator.service, false).await?)?["cached"],
        1
    );
    assert_eq!(connector.artist_requests.lock().await.len(), 3);
    let indexed: String = sqlx::query_scalar("SELECT artist FROM tracks")
        .fetch_one(&storage.pool)
        .await?;
    assert_eq!(indexed, "作曲家");
    // A broad search hit without the exact spelling in fetched details is not evidence.
    connector.artist_details.lock().await.insert(
        artist(1).id,
        Artist {
            aliases: vec![],
            ..artist(1)
        },
    );
    let unsupported = result(&run(&coordinator.service, true).await?)?;
    assert_eq!(unsupported["plans"][0]["candidates"], json!([]));
    // Matching an existing sort spelling is also retrieval-only.
    connector.artist_details.lock().await.insert(
        artist(1).id,
        Artist {
            sort_name: Some("作曲家".into()),
            aliases: vec![],
            ..artist(1)
        },
    );
    assert_eq!(
        result(&run(&coordinator.service, true).await?)?["plans"][0]["candidates"][0]["id"],
        RECORDING
    );
    // If the original full title/artist already matches, the normal gates can accept
    // a candidate recovered through an artist ID without substituting any alias.
    sqlx::query("UPDATE tracks SET artist = 'Artist'")
        .execute(&storage.pool)
        .await?;
    assert_eq!(
        result(&run(&coordinator.service, false).await?)?["identified"],
        1
    );
    connector.recording_conflict.store(true, Ordering::SeqCst);
    let contradicted = result(&run(&coordinator.service, true).await?)?;
    assert_eq!(contradicted["unmatched"], 1);
    assert_eq!(contradicted["plans"][0]["candidates"], json!([]));
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}

#[tokio::test]
async fn catalog_artist_aliases_keep_homonyms_competing_and_bound_ambiguous_expansion() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    configure(&connector, &[artist(2), artist(1), artist(1)]).await?;
    connector.artist_recordings.lock().await.insert(
        artist(2).id,
        vec![recording("00000000-0000-0000-0000-000000000002")],
    );
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    let ambiguous = result(&run(&coordinator.service, false).await?)?;
    assert_eq!(ambiguous["unmatched"], 1);
    assert_eq!(
        ambiguous["plans"][0]["candidates"]
            .as_array()
            .ok_or("candidates")?
            .len(),
        2
    );
    let order = connector.artist_requests.lock().await.clone();
    assert_eq!(order.len(), 5); // one name, two unique details, two recording queries
    connector.artist_requests.lock().await.clear();
    connector
        .artist_hits
        .lock()
        .await
        .insert("Artist".into(), vec![artist(1), artist(2)]);
    let reordered = result(&run(&coordinator.service, true).await?)?;
    assert_eq!(
        reordered["plans"][0]["candidates"],
        ambiguous["plans"][0]["candidates"]
    );
    assert_eq!(*connector.artist_requests.lock().await, order);
    connector
        .artist_hits
        .lock()
        .await
        .insert("Artist".into(), (1..=4).map(artist).collect());
    connector.artist_requests.lock().await.clear();
    let bounded = result(&run(&coordinator.service, true).await?)?;
    assert_eq!(bounded["unmatched"], 1);
    assert_eq!(bounded["plans"][0]["candidates"], json!([]));
    assert_eq!(connector.artist_requests.lock().await.len(), 1);
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}

#[tokio::test]
async fn catalog_artist_alias_failures_do_not_select_by_omission_or_block_fingerprints()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    configure(&connector, &[artist(1), artist(2)]).await?;
    connector
        .artist_failures
        .lock()
        .await
        .insert(format!("recordings:{}", artist(2).id));
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    let partial = result(&run(&coordinator.service, false).await?)?;
    assert_eq!(partial["unmatched"], 1);
    assert_eq!(partial["plans"][0]["partial"], true);
    assert_eq!(
        partial["plans"][0]["candidates"]
            .as_array()
            .ok_or("candidates")?
            .len(),
        1
    );
    assert!(
        storage
            .cleanup_enrichment(TrackId::new(1)?)
            .await?
            .is_none()
    );
    assert_eq!(
        result(&run(&coordinator.service, false).await?)?["cached"],
        0
    );
    connector.sources.update("acoustid", true).await?;
    assert_eq!(
        result(&run(&coordinator.service, false).await?)?["fingerprinted"],
        1
    );
    connector.sources.update("acoustid", false).await?;
    connector.artist_failures.lock().await.clear();
    connector
        .artist_details
        .lock()
        .await
        .insert(artist(2).id, artist(3));
    let wrong_id = result(&run(&coordinator.service, false).await?)?;
    assert_eq!(wrong_id["unmatched"], 1);
    assert_eq!(wrong_id["plans"][0]["partial"], true);
    connector
        .artist_details
        .lock()
        .await
        .insert(artist(2).id, artist(2));
    assert_eq!(
        result(&run(&coordinator.service, false).await?)?["identified"],
        1
    );
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}
