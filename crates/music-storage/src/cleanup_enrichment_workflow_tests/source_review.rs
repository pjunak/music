use super::*;

#[tokio::test]
async fn corroborated_recording_and_album_credits_preserve_authored_names() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    connector.sources.update("lastfm", false).await?;
    let id = "10000000-0000-0000-0000-000000000001";
    *connector.recording_credits.lock().await = vec![ArtistCredit {
        artist_id: id.into(),
        name: "Artist".into(),
        join_phrase: String::new(),
    }];
    connector.artist_details.lock().await.insert(
        id.into(),
        Artist {
            id: id.into(),
            name: "Artist".into(),
            credit_aliases: vec!["作曲家".into()],
            ..Artist::default()
        },
    );
    sqlx::query(
        "UPDATE tracks SET artist = '作曲家', album_artist = '作曲家', title = 'Old title'",
    )
    .execute(&storage.pool)
    .await?;
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    let parameters = json!({"scope":{"type":"all"},"force":true,"imports":[{"track_id":1,"fields":{"recording_mbid":RECORDING}}]});
    let reviewed = result(&run_parameters(&coordinator.service, parameters.clone()).await?)?;
    let plan = &reviewed["plans"][0];
    assert_eq!(plan["identity"]["artist"], "Artist");
    assert_eq!(plan["identity"]["method"], "identifier");
    for ops in [&plan["ops"], &plan["release_choices"][0]["ops"]] {
        assert!(
            ops.as_array()
                .ok_or("ops")?
                .iter()
                .all(|op| op["field"] != "artist" && op["field"] != "album_artist")
        );
    }
    assert!(
        plan["ops"]
            .as_array()
            .ok_or("ops")?
            .iter()
            .any(|op| op["field"] == "title")
    );
    assert!(
        plan["notes"]
            .to_string()
            .contains("Preserved authored artist credit")
    );
    connector
        .artist_details
        .lock()
        .await
        .get_mut(id)
        .ok_or("artist")?
        .credit_aliases
        .clear();
    let unverified = result(&run_parameters(&coordinator.service, parameters.clone()).await?)?;
    assert!(
        unverified["plans"][0]["ops"]
            .as_array()
            .ok_or("ops")?
            .iter()
            .any(|op| op["field"] == "artist")
    );
    connector
        .artist_details
        .lock()
        .await
        .get_mut(id)
        .ok_or("artist")?
        .id = "different-id".into();
    let partial = result(&run_parameters(&coordinator.service, parameters).await?)?;
    assert_eq!(partial["plans"][0]["partial"], true);
    let stored: String = sqlx::query_scalar("SELECT artist FROM tracks WHERE id=1")
        .fetch_one(&storage.pool)
        .await?;
    assert_eq!(stored, "作曲家");
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}

#[tokio::test]
async fn explicit_source_fields_remain_reviewable_when_catalog_identity_is_unmatched() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    connector.sources.update("lastfm", false).await?;
    connector.sources.update("acoustid", false).await?;
    connector.metadata_match.store(false, Ordering::SeqCst);
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    let mut parameters = json!({"scope":{"type":"all"},"imports":[{"track_id":1,"source":"Creator tracklist: confirmed deluxe edition","fields":{"title":"Source title","album":"Deluxe album","date":"2025-10-17","original_date":"2015-03-10","track_no":"4"}}]});
    let evidence_only = result(&run_parameters(&coordinator.service, parameters.clone()).await?)?;
    assert_eq!(evidence_only["plans"][0]["ops"], json!([]));
    parameters["imports"][0]["propose"] = json!(true);
    for _ in 0..2 {
        let review = result(&run_parameters(&coordinator.service, parameters.clone()).await?)?;
        let plan = &review["plans"][0];
        assert_eq!(plan["identity"], Value::Null);
        assert_eq!(review["identified"], 0);
        let ops = plan["ops"].as_array().ok_or("ops")?;
        assert_eq!(ops.len(), 4);
        assert!(ops.iter().all(|op| op["confidence"] == "low"
            && op["verified"] == false
            && op["rules"] == json!(["imported_metadata"])));
        assert!(
            ops.iter()
                .any(|op| op["field"] == "year" && op["new"] == 2025)
        );
        assert!(plan["local_evidence"].to_string().contains("2015-03-10"));
    }
    parameters["imports"][0]["fields"]["date"] = json!("2025-02-30");
    assert_eq!(
        run_parameters(&coordinator.service, parameters.clone())
            .await?
            .status,
        JobStatus::Failed
    );
    parameters["imports"][0]["fields"]["date"] = json!("2025");
    parameters["imports"][0]["source"] = Value::Null;
    assert_eq!(
        run_parameters(&coordinator.service, parameters)
            .await?
            .status,
        JobStatus::Failed
    );
    let indexed: String = sqlx::query_scalar("SELECT title FROM tracks WHERE id=1")
        .fetch_one(&storage.pool)
        .await?;
    assert_eq!(indexed, "Song");
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}
