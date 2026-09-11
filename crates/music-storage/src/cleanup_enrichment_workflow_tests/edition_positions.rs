use super::*;

fn slot(id: &str, track_no: Option<u32>, disc_no: Option<u32>) -> ReleaseSlot {
    ReleaseSlot {
        id: id.into(),
        recording_id: RECORDING.into(),
        title: "Song".into(),
        artist: "Artist".into(),
        length_ms: Some(120_000),
        track_no,
        disc_no,
    }
}

#[tokio::test]
async fn deluxe_editions_are_review_alternatives_until_explicitly_targeted() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    connector.deluxe_edition.store(true, Ordering::SeqCst);
    let coordinator = start_job_coordinator(storage, vec![Arc::new(handler)]).await?;
    for multiple in [false, true] {
        connector
            .multiple_editions
            .store(multiple, Ordering::SeqCst);
        let review = result(&run(&coordinator.service, true).await?)?;
        let plan = &review["plans"][0];
        assert_eq!(plan["identity"]["release_mbid"], Value::Null);
        let choices = plan["release_choices"].as_array().ok_or("choices")?;
        assert_eq!(choices.len(), if multiple { 2 } else { 1 });
        assert!(
            choices
                .iter()
                .any(|choice| choice["title"] == "Album (Deluxe Reissue)")
        );
    }
    let pinned = result(&run_parameters(&coordinator.service, json!({"scope":{"type":"all"},"force":true,"imports":[{"track_id":1,"fields":{"release_mbid":RELEASE}}]})).await?)?;
    assert_eq!(pinned["plans"][0]["identity"]["release_mbid"], RELEASE);
    assert!(
        pinned["plans"][0]["ops"]
            .as_array()
            .ok_or("ops")?
            .iter()
            .any(|op| op["field"] == "album" && op["new"] == "Album (Deluxe Reissue)")
    );
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}

#[tokio::test]
async fn edition_notes_distinguish_unchanged_missing_and_unassigned_positions() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Arc::new(
        SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
    );
    let (connector, handler) = setup(storage.clone()).await?;
    let coordinator = start_job_coordinator(storage.clone(), vec![Arc::new(handler)]).await?;
    for (slots, expected) in [
        (
            vec![slot("slot-one", Some(1), Some(1))],
            "matched release track slot-one has catalog disc 1, track 1.",
        ),
        (
            vec![slot("slot-unknown", None, None)],
            "matched release track slot-unknown has catalog disc unknown, track unknown.",
        ),
        (
            vec![
                slot("slot-a", Some(2), Some(1)),
                slot("slot-b", Some(3), Some(1)),
            ],
            "no usable release-track assignment; track and disc positions remain unresolved.",
        ),
    ] {
        *connector.release_slots.lock().await = Some(slots);
        let run = result(&run(&coordinator.service, true).await?)?;
        let plan = &run["plans"][0];
        assert_eq!(plan["identity"]["release_mbid"], RELEASE);
        assert!(plan["notes"].as_array().ok_or("notes")?.iter().any(|note| {
            note.as_str()
                .is_some_and(|note| note == format!("Edition {RELEASE}: {expected}"))
        }));
        for operations in [&plan["ops"], &plan["release_choices"][0]["ops"]] {
            assert!(
                operations
                    .as_array()
                    .ok_or("ops")?
                    .iter()
                    .all(|op| op["field"] != "track_no" && op["field"] != "disc_no")
            );
        }
    }
    let positions: (i64, i64) = sqlx::query_as("SELECT track_no, disc_no FROM tracks WHERE id = 1")
        .fetch_one(&storage.pool)
        .await?;
    assert_eq!(positions, (1, 1));
    coordinator.service.shutdown();
    coordinator.local_task.await??;
    coordinator.provider_task.await??;
    Ok(())
}
