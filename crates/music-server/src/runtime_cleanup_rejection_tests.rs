use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt;
type TestError = Box<dyn std::error::Error + Send + Sync>;

async fn request(
    router: &Router,
    cookie: &str,
    method: &str,
    path: &str,
    body: Value,
) -> Result<(StatusCode, Value), TestError> {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))?,
        )
        .await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
    Ok((
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)?
        },
    ))
}

pub(super) async fn exercise(
    router: &Router,
    cookie: &str,
    track_id: i64,
    root: &std::path::Path,
) -> Result<(), TestError> {
    let proposal = json!({"op_id":"test-rename", "track_id":track_id, "path":"Album/01 - First.mp3", "kind":"rename", "field":null, "old":"01 - First", "new":"First", "rules":["strip_track_numbers"], "confidence":"high", "verified":false, "evidence":null, "evidence_context":null});
    for (method, path, body) in [
        ("GET", "/api/library/cleanup/rejections", Value::Null),
        ("POST", "/api/library/cleanup/rejections", proposal.clone()),
        (
            "POST",
            "/api/library/cleanup/rejections/match",
            json!([proposal]),
        ),
        (
            "POST",
            "/api/library/cleanup/rejections/1/restore",
            Value::Null,
        ),
        ("DELETE", "/api/library/cleanup/rejections/1", Value::Null),
    ] {
        assert_eq!(
            request(router, "", method, path, body).await?.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let (status, saved) = request(
        router,
        cookie,
        "POST",
        "/api/library/cleanup/rejections",
        proposal.clone(),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let id = saved["id"].as_i64().ok_or("rejection id")?;
    let (_, pool) = request(
        router,
        cookie,
        "GET",
        "/api/library/cleanup/rejections?search=First",
        Value::Null,
    )
    .await?;
    assert_eq!(pool["items"][0]["proposal"], proposal);
    assert_eq!(pool["items"][0]["current"], true);
    assert_eq!(
        request(
            router,
            cookie,
            "POST",
            "/api/library/cleanup/rejections/match",
            json!([proposal])
        )
        .await?
        .1,
        json!([id])
    );
    let (status, restored) = request(
        router,
        cookie,
        "POST",
        &format!("/api/library/cleanup/rejections/{id}/restore"),
        Value::Null,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(restored, proposal);
    assert!(root.join("Album/01 - First.mp3").is_file());
    assert!(!root.join("Album/First.mp3").exists());
    let (_, empty) = request(
        router,
        cookie,
        "GET",
        "/api/library/cleanup/rejections",
        Value::Null,
    )
    .await?;
    assert_eq!(empty["items"], json!([]));
    let (_, applied) = request(router, cookie, "POST", "/api/library/cleanup/apply", json!({"ops":[{"track_id":track_id,"kind":"rename","field":null,"old":restored["old"],"new":restored["new"]}], "scope_label":"Restored rejection", "batch_id":null})).await?;
    assert_eq!(applied["applied"], 1);
    assert!(root.join("Album/First.mp3").is_file());
    let batch = applied["batch_id"].as_i64().ok_or("journal batch")?;
    let (_, reverted) = request(
        router,
        cookie,
        "POST",
        &format!("/api/library/cleanup/batches/{batch}/revert"),
        Value::Null,
    )
    .await?;
    assert_eq!(reverted["reverted"], 1);
    assert!(root.join("Album/01 - First.mp3").is_file());
    let mut invalid = proposal.clone();
    invalid["new"] = json!("../escape");
    assert_eq!(
        request(
            router,
            cookie,
            "POST",
            "/api/library/cleanup/rejections",
            invalid
        )
        .await?
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut stale = proposal;
    stale["old"] = json!("A different file");
    assert_eq!(
        request(
            router,
            cookie,
            "POST",
            "/api/library/cleanup/rejections",
            stale
        )
        .await?
        .0,
        StatusCode::CONFLICT
    );
    Ok(())
}

pub(super) async fn exercise_rich_metadata(
    router: &Router,
    cookie: &str,
    track_id: i64,
    root: &std::path::Path,
) -> Result<(), TestError> {
    let path = format!("/api/library/tracks/{track_id}");
    let (_, before) = request(router, cookie, "GET", &path, Value::Null).await?;
    let rich = json!({"release_date":"2024-02-29", "original_release_date":"1998-07", "composer":"久石 譲"});
    let (status, updated) = request(
        router,
        cookie,
        "PATCH",
        &format!("{path}/metadata"),
        rich.clone(),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["year"], 2024);
    for field in ["release_date", "original_release_date", "composer"] {
        assert_eq!(updated[field], rich[field]);
    }
    let (status, _) = request(
        router,
        cookie,
        "PATCH",
        &format!("{path}/metadata"),
        json!({"release_date":"2025-02-29"}),
    )
    .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (_, unchanged) = request(router, cookie, "GET", &path, Value::Null).await?;
    assert_eq!(unchanged, updated);
    let (_, bulk) = request(
        router,
        cookie,
        "PATCH",
        "/api/library/tracks/bulk-metadata",
        json!({"track_ids":[track_id],"updates":{"year":2025}}),
    )
    .await?;
    assert_eq!(bulk["updated"], json!([]));
    assert!(
        bulk["skipped"][0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("release_date"))
    );
    let (status, conflict) = request(
        router,
        cookie,
        "PATCH",
        &format!("{path}/metadata"),
        json!({"year":2023,"release_date":"2024-02-29"}),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(conflict.to_string().contains("disagree"));
    assert_eq!(bulk["skipped"].as_array().ok_or("skips")?.len(), 1);
    let proposal = json!({"op_id":"rich-date", "track_id":track_id, "path":updated["path"], "kind":"tag", "field":"release_date", "old":"2024-02-29", "new":"2025-10-17", "rules":["imported_metadata"], "confidence":"low", "verified":false, "evidence":null, "evidence_context":null});
    let (status, rejection) = request(
        router,
        cookie,
        "POST",
        "/api/library/cleanup/rejections",
        proposal.clone(),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{rejection}");
    let id = rejection["id"].as_i64().ok_or("rejection id")?;
    let (status, restored) = request(
        router,
        cookie,
        "POST",
        &format!("/api/library/cleanup/rejections/{id}/restore"),
        Value::Null,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(restored, proposal);
    let ops = [("release_date", "2025-10-17"), ("original_release_date", "1998-07-01"), ("composer", "Composer B")].map(|(field, value)| json!({"track_id":track_id,"kind":"tag","field":field,"old":rich[field],"new":value}));
    let (status, applied) = request(
        router,
        cookie,
        "POST",
        "/api/library/cleanup/apply",
        json!({"ops":ops,"scope_label":"Rich metadata","batch_id":null}),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(applied["applied"], 3, "{applied}");
    let (_, current) = request(router, cookie, "GET", &path, Value::Null).await?;
    assert_eq!(current["release_date"], "2025-10-17");
    assert_eq!(current["year"], 2025);
    let batch = applied["batch_id"].as_i64().ok_or("batch")?;
    let (_, reverted) = request(
        router,
        cookie,
        "POST",
        &format!("/api/library/cleanup/batches/{batch}/revert"),
        Value::Null,
    )
    .await?;
    assert_eq!(reverted["reverted"], 3, "{reverted}");
    let (_, reverted) = request(router, cookie, "GET", &path, Value::Null).await?;
    for field in ["release_date", "original_release_date", "composer"] {
        assert_eq!(reverted[field], rich[field]);
    }
    let file =
        music_media::read_audio_metadata(&root.join(updated["path"].as_str().ok_or("path")?))?;
    assert_eq!(file.release_date, "2024-02-29");
    assert_eq!(file.original_release_date, "1998-07");
    assert_eq!(file.composer, "久石 譲");
    let restore = json!({"release_date":before["release_date"],"original_release_date":before["original_release_date"],"composer":before["composer"]});
    let (status, _) = request(
        router,
        cookie,
        "PATCH",
        &format!("{path}/metadata"),
        restore,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    Ok(())
}
