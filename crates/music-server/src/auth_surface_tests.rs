//! Pins the authentication surface: every documented route rejects a request
//! without a session, except the declared guest routes that let browsers,
//! output appliances and Baton read and play without signing in.

use std::error::Error;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tempfile::tempdir;
use tower::ServiceExt;

use crate::http::openapi_document;
use crate::runtime::AppRuntime;
use crate::test_support::test_config;

const GUEST_ROUTES: &[&str] = &[
    "GET /api/health",
    "GET /api/readiness",
    "POST /api/auth/login",
    "GET /api/sync/state",
    "GET /api/library/tracks",
    "GET /api/library/tracks/{track_id}",
    "GET /api/library/tracks/{track_id}/cover",
    "GET /api/library/tracks/{track_id}/stream",
    "GET /api/modes/{mode_id}/presets",
    "GET /api/sfx/file",
];

#[tokio::test]
async fn every_route_requires_a_session_unless_it_is_a_declared_guest_route()
-> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let runtime = AppRuntime::start(test_config(directory.path(), &[])?).await?;
    let router = runtime.router()?;
    let document = serde_json::to_value(openapi_document())?;
    let paths = document["paths"]
        .as_object()
        .ok_or("paths is not an object")?;

    let mut open = Vec::new();
    for (path, operations) in paths {
        let uri = sample_uri(path);
        for method in operations
            .as_object()
            .ok_or("operations is not an object")?
            .keys()
        {
            let method = method.to_ascii_uppercase();
            let request = Request::builder()
                .method(method.as_str())
                .uri(&uri)
                .header("content-type", "application/json")
                .body(Body::from("{}"))?;
            let status = router.clone().oneshot(request).await?.status();
            if status != StatusCode::UNAUTHORIZED {
                open.push(format!("{method} {path}"));
            }
        }
    }
    open.sort();
    let mut expected = GUEST_ROUTES
        .iter()
        .map(|route| (*route).to_owned())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(open, expected, "routes reachable without a session changed");
    Ok(())
}

/// Fill path parameters with a value every parameter type accepts.
fn sample_uri(path: &str) -> String {
    let mut uri = String::with_capacity(path.len());
    let mut inside = false;
    for character in path.chars() {
        match character {
            '{' => {
                inside = true;
                uri.push('1');
            }
            '}' => inside = false,
            _ if inside => {}
            _ => uri.push(character),
        }
    }
    uri
}
