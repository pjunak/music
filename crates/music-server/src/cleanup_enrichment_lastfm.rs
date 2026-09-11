use super::{CatalogError, CatalogFailure, CommunityTag, bounded_json, http_failure};

pub(super) async fn community_tags(
    client: &reqwest::Client,
    endpoint: &str,
    artist: &str,
    title: &str,
    api_key: &str,
    recording_id: Option<&str>,
) -> Result<Vec<CommunityTag>, CatalogError> {
    let mut query = vec![
        ("method", "track.gettoptags"),
        ("api_key", api_key),
        ("autocorrect", "0"),
        ("format", "json"),
    ];
    if let Some(id) = recording_id {
        query.push(("mbid", id));
    } else {
        query.extend([("artist", artist), ("track", title)]);
    }
    // This is a read service. Only typed failures may escape this adapter:
    // the GET URL contains the credential and must never reach retained notes.
    let response = client
        .get(endpoint)
        .query(&query)
        .send()
        .await
        .map_err(|error| CatalogError::LastFmFailure(http_failure(&error)))?;
    parse_response(response).await
}

async fn parse_response(response: reqwest::Response) -> Result<Vec<CommunityTag>, CatalogError> {
    let status = response.status();
    let payload = bounded_json(response).await;
    if !status.is_success() {
        // Last.fm may return its actionable API code inside an HTTP error.
        // Read it under the same size/deadline bounds as success responses,
        // retain only the numeric code, and never accept tags from an HTTP error.
        let code = payload.as_ref().ok().and_then(|body| {
            body.get("error")
                .and_then(serde_json::Value::as_u64)
                .and_then(|code| u32::try_from(code).ok())
        });
        let failure = code.map_or(CatalogFailure::HttpStatus(status.as_u16()), |code| {
            CatalogFailure::HttpProviderCode {
                status: status.as_u16(),
                code,
            }
        });
        return Err(CatalogError::LastFmFailure(failure));
    }
    let payload = payload.map_err(CatalogError::LastFmFailure)?;
    super::parse_community_tags(&payload).map_err(|error| match error {
        CatalogError::LastFmFailure(_) => error,
        _ => CatalogError::LastFmFailure(CatalogFailure::InvalidPayload),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::Query, routing::get};
    use serde_json::json;
    use std::collections::BTreeMap;

    type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

    #[tokio::test]
    async fn read_service_uses_get_and_keeps_recording_lookup_scoped_to_mbid() -> TestResult {
        let router = Router::new().route(
            "/",
            get(|Query(query): Query<BTreeMap<String, String>>| async move {
                assert_eq!(
                    query.get("method").map(String::as_str),
                    Some("track.gettoptags")
                );
                assert_eq!(
                    query.get("api_key").map(String::as_str),
                    Some("fixture-key")
                );
                assert_eq!(query.get("autocorrect").map(String::as_str), Some("0"));
                assert_eq!(query.get("format").map(String::as_str), Some("json"));
                if let Some(mbid) = query.get("mbid") {
                    assert_eq!(mbid, "00000000-0000-0000-0000-000000000001");
                    assert!(!query.contains_key("artist"));
                    assert!(!query.contains_key("track"));
                } else {
                    assert_eq!(
                        query.get("artist").map(String::as_str),
                        Some("Artist & Guest")
                    );
                    assert_eq!(query.get("track").map(String::as_str), Some("Song + Theme"));
                }
                Json(json!({"toptags":{"tag":[{"name":"dark","count":"80"}]}}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}/", listener.local_addr()?);
        let server = tokio::spawn(async move { axum::serve(listener, router).await });
        let client = reqwest::Client::builder()
            .timeout(super::super::HTTP_TIMEOUT)
            .build()?;
        for id in [Some("00000000-0000-0000-0000-000000000001"), None] {
            let tags = community_tags(
                &client,
                &endpoint,
                "Artist & Guest",
                "Song + Theme",
                "fixture-key",
                id,
            )
            .await?;
            assert_eq!(tags.len(), 1);
            assert_eq!(tags[0].name, "dark");
            assert_eq!(tags[0].count, 80);
        }
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn http_errors_retain_numeric_api_codes_without_accepting_tags_or_private_messages()
    -> TestResult {
        for (status, body, detail) in [
            (
                400,
                r#"{"error":6,"message":"private-key; private-title"}"#,
                "HTTP 400; provider error code 6",
            ),
            (
                403,
                r#"{"error":10,"message":"private-key"}"#,
                "HTTP 403; provider error code 10",
            ),
            (
                200,
                r#"{"error":26,"message":"private-key"}"#,
                "provider error code 26",
            ),
            (429, "<html>private-key</html>", "HTTP 429"),
            (400, r#"{"error":"private-key"}"#, "HTTP 400"),
            (
                400,
                r#"{"toptags":{"tag":[{"name":"dark","count":80}]}}"#,
                "HTTP 400",
            ),
            (302, r#"{"toptags":{"tag":[]}}"#, "HTTP 302"),
            (200, "not json; private-key", "invalid JSON response"),
            (200, "{}", "unexpected response structure"),
        ] {
            let response = axum::http::Response::builder().status(status).body(body)?;
            let error = parse_response(reqwest::Response::from(response))
                .await
                .err()
                .ok_or("fixture must fail")?;
            assert_eq!(error.code(), "lastfm_unavailable");
            assert_eq!(
                error.annotate("Tags unavailable."),
                format!("Tags unavailable. Details: Last.fm: {detail}.")
            );
            assert!(!format!("{error:?}").contains("private"));
        }
        Ok(())
    }

    #[tokio::test]
    async fn oversized_error_bodies_stay_bounded_and_empty_success_is_valid() -> TestResult {
        let oversized = axum::http::Response::builder().status(400).body(format!(
            r#"{{"error":6,"message":"{}"}}"#,
            "x".repeat(super::super::MAX_RESPONSE_BYTES)
        ))?;
        let error = parse_response(reqwest::Response::from(oversized))
            .await
            .err()
            .ok_or("fixture must fail")?;
        assert_eq!(
            error.annotate("Tags unavailable."),
            "Tags unavailable. Details: Last.fm: HTTP 400."
        );
        let empty = axum::http::Response::builder().body(r#"{"toptags":{"tag":[]}}"#)?;
        assert!(
            parse_response(reqwest::Response::from(empty))
                .await?
                .is_empty()
        );
        Ok(())
    }
}
