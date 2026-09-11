//! One retry per transient read, with a shared ten-retry job budget and admission clock.
use super::{MusicBrainzLookupError, MusicBrainzNameLookup};
use std::time::{Duration, SystemTime};
use tokio::time::Instant;

#[derive(Debug)]
pub(super) struct RequestAdmission {
    next_request_at: Option<Instant>,
    cooldown_until: Option<Instant>,
    cooldown_overflow: bool,
    retries_remaining: u8,
}

impl Default for RequestAdmission {
    fn default() -> Self {
        Self {
            next_request_at: None,
            cooldown_until: None,
            cooldown_overflow: false,
            retries_remaining: 10,
        }
    }
}

impl MusicBrainzNameLookup {
    pub(crate) async fn begin_catalog_lookup(&self) {
        // Refresh never bypasses an outstanding provider cooldown.
        self.admission.lock().await.retries_remaining = 10;
    }

    pub(super) async fn fetch_response(
        &self,
        resource: &str,
        query: &[(&str, String)],
    ) -> Result<reqwest::Response, MusicBrainzLookupError> {
        let endpoint = format!("{}/{resource}", self.base_url.trim_end_matches('/'));
        for attempt in 0..2 {
            let mut admission = self.admission.lock().await;
            if admission.cooldown_overflow {
                return Err(MusicBrainzLookupError::CoolingDown);
            }
            let now = Instant::now();
            let next = admission
                .next_request_at
                .into_iter()
                .chain(admission.cooldown_until)
                .max();
            if let Some(next) = next {
                // Long Retry-After periods produce partial results without tying up a job.
                if next.saturating_duration_since(now) > Duration::from_secs(30) {
                    return Err(MusicBrainzLookupError::CoolingDown);
                }
                if next > now {
                    tokio::time::sleep_until(next).await;
                }
            }
            // Hold admission through response headers, including failures.
            let response = self.client.get(&endpoint).query(query).send().await;
            let now = Instant::now();
            admission.next_request_at = Some(now + self.minimum_interval);
            let transient = match &response {
                Ok(response) => matches!(response.status().as_u16(), 429 | 502 | 503 | 504),
                Err(error) => error.is_timeout() || error.is_connect(),
            };
            let delay = response
                .as_ref()
                .ok()
                .and_then(|r| retry_after(r.headers(), SystemTime::now()))
                .unwrap_or(self.minimum_interval.saturating_mul(2));
            if transient {
                // checked_add avoids panics for a maliciously large numeric Retry-After.
                admission.cooldown_until = now.checked_add(delay);
                admission.cooldown_overflow = admission.cooldown_until.is_none();
            }
            let retry = transient
                && attempt == 0
                && admission.retries_remaining > 0
                && delay <= Duration::from_secs(30);
            if retry {
                admission.retries_remaining -= 1;
            }
            drop(admission);
            if retry {
                continue;
            }
            return response
                .map_err(MusicBrainzLookupError::Http)?
                .error_for_status()
                .map_err(MusicBrainzLookupError::Http);
        }
        Err(MusicBrainzLookupError::CoolingDown)
    }
}

fn retry_after(headers: &reqwest::header::HeaderMap, now: SystemTime) -> Option<Duration> {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    httpdate::parse_http_date(value)
        .ok()
        .map(|date| date.duration_since(now).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, extract::State, http::StatusCode, routing::get};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn retry_after_accepts_seconds_and_http_dates() -> Result<(), Box<dyn std::error::Error>> {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        for (value, expected) in [
            ("17".to_owned(), Some(17)),
            (
                httpdate::fmt_http_date(now + Duration::from_secs(24)),
                Some(24),
            ),
            ("invalid".to_owned(), None),
        ] {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(reqwest::header::RETRY_AFTER, value.parse()?);
            assert_eq!(retry_after(&headers, now).map(|d| d.as_secs()), expected);
        }
        Ok(())
    }

    #[tokio::test]
    async fn transient_failure_recovers_after_one_paced_retry()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let arrivals = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let router = Router::new()
            .route(
                "/",
                get(
                    |State(arrivals): State<Arc<tokio::sync::Mutex<Vec<Instant>>>>| async move {
                        let mut arrivals = arrivals.lock().await;
                        arrivals.push(Instant::now());
                        if arrivals.len() == 1 {
                            (StatusCode::BAD_GATEWAY, axum::Json(serde_json::json!({})))
                        } else {
                            (StatusCode::OK, axum::Json(serde_json::json!({"ok":true})))
                        }
                    },
                ),
            )
            .with_state(arrivals.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let mut client =
            MusicBrainzNameLookup::fixture_endpoint(&format!("http://{}", listener.local_addr()?))?;
        client.minimum_interval = Duration::from_millis(30);
        let server = tokio::spawn(async { axum::serve(listener, router).await });
        let (first, second) = tokio::join!(client.fetch_json("", &[]), client.fetch_json("", &[]));
        assert_eq!(first?["ok"], true);
        assert_eq!(second?["ok"], true);
        let arrivals = arrivals.lock().await;
        assert_eq!(arrivals.len(), 3);
        assert!(arrivals[1].duration_since(arrivals[0]) >= Duration::from_millis(60));
        assert!(arrivals[2].duration_since(arrivals[1]) >= Duration::from_millis(30));
        assert_eq!(client.admission.lock().await.retries_remaining, 9);
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn retries_transient_reads_once_and_shares_the_run_budget()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let count = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .route(
                "/transient",
                get(|State(count): State<Arc<AtomicUsize>>| async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    StatusCode::SERVICE_UNAVAILABLE
                }),
            )
            .route("/permanent", get(|| async { StatusCode::BAD_REQUEST }))
            .route(
                "/cooldown",
                get(|| async { (StatusCode::SERVICE_UNAVAILABLE, [("retry-after", "120")]) }),
            )
            .route(
                "/ok",
                get(|| async { axum::Json(serde_json::json!({"ok": true})) }),
            )
            .with_state(count.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let client =
            MusicBrainzNameLookup::fixture_endpoint(&format!("http://{}", listener.local_addr()?))?;
        let server = tokio::spawn(async { axum::serve(listener, router).await });
        for _ in 0..12 {
            assert!(client.fetch_json("transient", &[]).await.is_err());
        }
        assert_eq!(count.load(Ordering::SeqCst), 22); // Twelve reads plus ten retries.
        client.begin_catalog_lookup().await;
        assert!(client.fetch_json("permanent", &[]).await.is_err());
        assert_eq!(client.admission.lock().await.retries_remaining, 10);
        assert_eq!(client.fetch_json("ok", &[]).await?["ok"], true);
        assert!(client.fetch_json("cooldown", &[]).await.is_err());
        client.begin_catalog_lookup().await;
        assert!(matches!(
            client.fetch_json("ok", &[]).await,
            Err(MusicBrainzLookupError::CoolingDown)
        ));
        server.abort();
        Ok(())
    }
}
