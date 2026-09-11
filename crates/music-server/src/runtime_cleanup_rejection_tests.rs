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
