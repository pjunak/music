use axum::http::HeaderMap;
use axum::http::header::{HOST, ORIGIN};

use crate::config::AppConfig;

#[derive(Debug, Clone)]
pub(crate) struct WebsocketOriginPolicy {
    public_scheme: &'static str,
    allowed_origins: Vec<String>,
}

impl WebsocketOriginPolicy {
    pub(crate) fn from_config(config: &AppConfig) -> Self {
        Self {
            // The documented deployment contract uses secure cookies for HTTPS
            // (including a proxy's external HTTPS hop), false only for plain HTTP.
            public_scheme: if config.session_cookie_secure {
                "https"
            } else {
                "http"
            },
            allowed_origins: config
                .allowed_origins
                .iter()
                .filter_map(|value| canonical_origin(value))
                .collect(),
        }
    }

    pub(crate) fn allows(&self, headers: &HeaderMap) -> bool {
        let mut origins = headers.get_all(ORIGIN).iter();
        let Some(origin) = origins.next() else {
            // Native clients omit Origin. Their existing session and mutation
            // checks still apply; this policy only adds the browser boundary.
            return true;
        };
        if origins.next().is_some() {
            return false;
        }
        let Some(origin) = origin.to_str().ok().and_then(canonical_origin) else {
            return false;
        };
        if self.allowed_origins.contains(&origin) {
            return true;
        }
        let mut hosts = headers.get_all(HOST).iter();
        let Some(host) = hosts.next().and_then(|host| host.to_str().ok()) else {
            return false;
        };
        if hosts.next().is_some() {
            return false;
        }
        // Never trust arbitrary Forwarded/X-Forwarded-* values. Proxies which
        // rewrite Host must explicitly list the public origin in ALLOWED_ORIGINS.
        canonical_origin(&format!("{}://{host}", self.public_scheme)).as_ref() == Some(&origin)
    }
}

pub(crate) fn canonical_origin(value: &str) -> Option<String> {
    let (scheme, authority) = value.split_once("://")?;
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        || authority.is_empty()
        || value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        || authority.contains(['/', '\\', '?', '#', '@', ','])
    {
        return None;
    }
    let url = reqwest::Url::parse(value).ok()?;
    url.host_str()?;
    Some(url.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(
        origin: &str,
        host: &str,
    ) -> Result<HeaderMap, axum::http::header::InvalidHeaderValue> {
        Ok(HeaderMap::from_iter([
            (ORIGIN, HeaderValue::from_str(origin)?),
            (HOST, HeaderValue::from_str(host)?),
        ]))
    }

    #[test]
    fn websocket_origin_accepts_native_same_origin_and_explicit_extra_origins()
    -> Result<(), Box<dyn std::error::Error>> {
        let policy = WebsocketOriginPolicy {
            public_scheme: "https",
            allowed_origins: vec!["https://controller.example".to_owned()],
        };
        assert!(policy.allows(&HeaderMap::new()));
        for (origin, host) in [
            ("https://music.example", "music.example"),
            ("https://MUSIC.example:443", "music.example"),
            ("https://music.example", "music.example:443"),
            ("https://[::1]:8443", "[::1]:8443"),
            ("https://controller.example", "internal-server:8000"),
        ] {
            assert!(policy.allows(&headers(origin, host)?), "{origin} {host}");
        }
        let plain = WebsocketOriginPolicy {
            public_scheme: "http",
            allowed_origins: vec![],
        };
        assert!(plain.allows(&headers("http://localhost:8000", "localhost:8000")?));
        assert!(!plain.allows(&headers("https://localhost:8000", "localhost:8000")?));
        Ok(())
    }

    #[test]
    fn websocket_origin_rejects_sibling_sites_invalid_forms_and_forwarded_spoofing()
    -> Result<(), Box<dyn std::error::Error>> {
        let policy = WebsocketOriginPolicy {
            public_scheme: "https",
            allowed_origins: vec![],
        };
        for origin in [
            "https://sibling.example",
            "https://music.example.evil",
            "http://music.example",
            "https://music.example:8443",
            "null",
            "",
            " https://music.example",
            "https://music.example/",
            "https://music.example/..",
            "https://music.example?x",
            "https://music.example#x",
            "https://user@music.example",
            "https://@music.example",
            "https://music.example https://evil.example",
            "https://music.example,https://evil.example",
            "https://music.example\\evil",
            "file://music.example",
            "https://music.example:bad",
        ] {
            assert!(
                !policy.allows(&headers(origin, "music.example")?),
                "{origin}"
            );
        }
        let mut duplicate = headers("https://music.example", "music.example")?;
        duplicate.append(ORIGIN, HeaderValue::from_static("https://evil.example"));
        assert!(!policy.allows(&duplicate));
        let mut forwarded = headers("https://evil.example", "music.example")?;
        forwarded.insert("x-forwarded-host", HeaderValue::from_static("evil.example"));
        forwarded.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        forwarded.insert(
            "forwarded",
            HeaderValue::from_static("host=evil.example;proto=https"),
        );
        assert!(!policy.allows(&forwarded));
        let mut invalid_bytes = headers("https://music.example", "music.example")?;
        invalid_bytes.insert(ORIGIN, HeaderValue::from_bytes(b"\xff")?);
        assert!(!policy.allows(&invalid_bytes));
        Ok(())
    }
}
