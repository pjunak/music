use std::fmt::Debug;

use music_application::assistant::{
    GOOGLE_GEMINI_OPENAI_ADAPTER, GOOGLE_GEMINI_OPENAI_JSON_SCHEMA_ADAPTER,
    OPENAI_RESPONSES_ADAPTER, ProviderConnectionPolicy, ProviderPolicyError,
};
use reqwest::Url;

const GOOGLE_GEMINI_OPENAI_BASE_URL: &str =
    "https://generativelanguage.googleapis.com/v1beta/openai";
const OPENAI_API_BASE_URL: &str = "https://api.openai.com/v1";

#[derive(Debug, Default)]
pub(crate) struct ProviderNetworkBoundary;

impl ProviderNetworkBoundary {
    pub(crate) const fn new() -> Self {
        Self
    }
}

impl ProviderConnectionPolicy for ProviderNetworkBoundary {
    fn normalize_base_url(
        &self,
        adapter_id: &str,
        raw: &str,
        allow_private_network: bool,
    ) -> Result<String, ProviderPolicyError> {
        normalize_provider_base_url(raw, allow_private_network)
            .and_then(|normalized| validate_adapter_base_url(adapter_id, normalized))
    }
}

fn normalize_provider_base_url(
    raw: &str,
    allow_private_network: bool,
) -> Result<String, ProviderPolicyError> {
    let value = raw.trim();
    if value
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(url_error("Provider URL cannot contain whitespace."));
    }
    let mut parsed = Url::parse(value).map_err(|_| url_error("Provider URL is invalid."))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(url_error("Provider URL must use HTTP or HTTPS."));
    }
    if parsed.scheme() != "https" && !allow_private_network {
        return Err(url_error("Public provider connections must use HTTPS."));
    }
    if parsed.host_str().is_none() {
        return Err(url_error("Provider URL must include a host."));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(url_error("Provider URL cannot contain credentials."));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(url_error(
            "Provider URL cannot contain a query or fragment.",
        ));
    }
    let trimmed_path = parsed.path().trim_end_matches('/').to_owned();
    parsed.set_path(&trimmed_path);
    let mut normalized = parsed.to_string();
    if trimmed_path.is_empty() && normalized.ends_with('/') {
        normalized.pop();
    }
    Ok(normalized)
}

fn validate_adapter_base_url(
    adapter_id: &str,
    normalized: String,
) -> Result<String, ProviderPolicyError> {
    let expected = match adapter_id {
        OPENAI_RESPONSES_ADAPTER => Some(OPENAI_API_BASE_URL),
        GOOGLE_GEMINI_OPENAI_ADAPTER | GOOGLE_GEMINI_OPENAI_JSON_SCHEMA_ADAPTER => {
            Some(GOOGLE_GEMINI_OPENAI_BASE_URL)
        }
        _ => None,
    };
    if expected.is_some_and(|expected| normalized != expected) {
        return Err(ProviderPolicyError {
            code: "invalid_provider_url".to_owned(),
            message: "This provider adapter requires its documented API base URL.".to_owned(),
        });
    }
    Ok(normalized)
}

fn url_error(message: &str) -> ProviderPolicyError {
    ProviderPolicyError {
        code: "invalid_provider_url".to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use music_application::assistant::{
        GOOGLE_GEMINI_OPENAI_ADAPTER, OPENAI_COMPATIBLE_ADAPTER, OPENAI_RESPONSES_ADAPTER,
        ProviderConnectionPolicy,
    };

    use super::{GOOGLE_GEMINI_OPENAI_BASE_URL, OPENAI_API_BASE_URL, ProviderNetworkBoundary};

    #[test]
    fn normalizes_hosts_ports_and_trailing_paths_without_relaxing_public_https() {
        let policy = ProviderNetworkBoundary::new();
        assert_eq!(
            policy
                .normalize_base_url(
                    OPENAI_COMPATIBLE_ADAPTER,
                    " HTTPS://ExAmPlE.COM:8443/v1/// ",
                    false,
                )
                .as_deref(),
            Ok("https://example.com:8443/v1")
        );
        assert!(
            policy
                .normalize_base_url(OPENAI_COMPATIBLE_ADAPTER, "http://example.com/v1", false,)
                .is_err()
        );
        assert_eq!(
            policy
                .normalize_base_url(
                    OPENAI_COMPATIBLE_ADAPTER,
                    "http://127.0.0.1:11434/v1/",
                    true,
                )
                .as_deref(),
            Ok("http://127.0.0.1:11434/v1")
        );
    }

    #[test]
    fn rejects_ambiguous_authority_and_url_components() {
        let policy = ProviderNetworkBoundary::new();
        for value in [
            "https://user:secret@example.com/v1",
            "https://example.com/v1?q=1",
            "https://example.com/v1#fragment",
            "https://example.com/a b",
            "file:///tmp/provider",
        ] {
            assert!(
                policy
                    .normalize_base_url(OPENAI_COMPATIBLE_ADAPTER, value, false)
                    .is_err(),
                "accepted {value}"
            );
        }
    }

    #[test]
    fn fixed_vendor_adapters_require_their_documented_base_url() {
        let policy = ProviderNetworkBoundary::new();
        assert_eq!(
            policy
                .normalize_base_url(OPENAI_RESPONSES_ADAPTER, OPENAI_API_BASE_URL, false)
                .as_deref(),
            Ok(OPENAI_API_BASE_URL)
        );
        assert_eq!(
            policy
                .normalize_base_url(
                    GOOGLE_GEMINI_OPENAI_ADAPTER,
                    GOOGLE_GEMINI_OPENAI_BASE_URL,
                    false,
                )
                .as_deref(),
            Ok(GOOGLE_GEMINI_OPENAI_BASE_URL)
        );
        assert!(
            policy
                .normalize_base_url(
                    OPENAI_RESPONSES_ADAPTER,
                    "https://proxy.example.test/v1",
                    false,
                )
                .is_err()
        );
    }
}
