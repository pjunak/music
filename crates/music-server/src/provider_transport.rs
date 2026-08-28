use std::collections::BTreeSet;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use music_application::assistant::{
    GOOGLE_GEMINI_OPENAI_ADAPTER, GOOGLE_GEMINI_OPENAI_JSON_SCHEMA_ADAPTER,
    OPENAI_COMPATIBLE_ADAPTER, OPENAI_COMPATIBLE_JSON_SCHEMA_ADAPTER, OPENAI_RESPONSES_ADAPTER,
    ProviderConnectionPolicy, ProviderPolicyError, ProviderVerificationResult,
    ProviderVerificationTarget, provider_adapter,
};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use reqwest::{StatusCode, Url};
use serde_json::Value;

const GOOGLE_GEMINI_OPENAI_BASE_URL: &str =
    "https://generativelanguage.googleapis.com/v1beta/openai";
const OPENAI_API_BASE_URL: &str = "https://api.openai.com/v1";
const MAX_VERIFICATION_BYTES: usize = 1_024 * 1_024;
const MAX_VERIFIED_MODELS: usize = 200;
const VERIFICATION_TIMEOUT: Duration = Duration::from_secs(10);
const VERIFIER_USER_AGENT: &str = "music-assistant-provider-verifier/1";
const GEMINI_HEADERS: &[(&str, &str)] = &[("x-goog-api-client", "music-assistant-oai/1.0")];

type ResolveFuture<'a> = Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'a>>;

trait ProviderDnsResolver: fmt::Debug + Send + Sync {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> ResolveFuture<'a>;
}

#[derive(Debug, Default)]
struct SystemProviderDnsResolver;

impl ProviderDnsResolver for SystemProviderDnsResolver {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> ResolveFuture<'a> {
        Box::pin(async move {
            tokio::net::lookup_host((host, port))
                .await
                .map(Iterator::collect)
        })
    }
}

#[derive(Debug)]
pub(crate) struct ProviderNetworkBoundary {
    resolver: Arc<dyn ProviderDnsResolver>,
}

impl Default for ProviderNetworkBoundary {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderNetworkBoundary {
    pub(crate) fn new() -> Self {
        Self {
            resolver: Arc::new(SystemProviderDnsResolver),
        }
    }

    pub(crate) async fn verify_provider_connection(
        &self,
        target: &ProviderVerificationTarget,
    ) -> ProviderVerificationResult {
        let Some(handler) = provider_handler(&target.adapter_id) else {
            return failed_verification("unsupported_adapter");
        };
        let url = format!(
            "{}{}",
            target.base_url.trim_end_matches('/'),
            handler.models_path
        );
        let response = self
            .get_json(
                &url,
                target.api_key.expose_secret(),
                target.allow_private_network,
                VERIFICATION_TIMEOUT,
                MAX_VERIFICATION_BYTES,
                VERIFIER_USER_AGENT,
                handler.additional_headers,
            )
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => return failed_verification(error.code()),
        };
        if !response.status.is_success() {
            return failed_verification(safe_http_error_code(
                response.status,
                "models_endpoint_not_found",
                &response.payload,
            ));
        }
        let Some(entries) = response.payload.get("data").and_then(Value::as_array) else {
            return failed_verification("invalid_response");
        };
        let mut seen = BTreeSet::new();
        let models = entries
            .iter()
            .filter_map(|item| item.get("id").and_then(Value::as_str))
            .map(|model_id| handler.normalize_model_id(model_id))
            .filter(|model_id| !model_id.is_empty() && model_id.len() <= 256)
            .filter(|model_id| seen.insert((*model_id).to_owned()))
            .take(MAX_VERIFIED_MODELS)
            .map(str::to_owned)
            .collect();
        let capability_ids =
            provider_adapter(&target.adapter_id).map_or_else(Vec::new, |adapter| {
                adapter
                    .capability_ids
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect()
            });
        ProviderVerificationResult {
            verified: true,
            error_code: None,
            models,
            capability_ids,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn get_json(
        &self,
        raw_url: &str,
        api_key: &str,
        allow_private_network: bool,
        timeout: Duration,
        max_response_bytes: usize,
        user_agent: &str,
        additional_headers: &[(&str, &str)],
    ) -> Result<JsonHttpResponse, ProviderTransportError> {
        let url =
            Url::parse(raw_url).map_err(|_| ProviderTransportError::new("invalid_request"))?;
        let host = url
            .host_str()
            .ok_or_else(|| ProviderTransportError::new("invalid_request"))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| ProviderTransportError::new("invalid_request"))?;
        let addresses = self.destination_addresses(host, port).await?;
        if addresses.is_empty()
            || (!allow_private_network && addresses.iter().any(|address| !is_global(address.ip())))
        {
            return Err(ProviderTransportError::new("destination_blocked"));
        }
        let headers = request_headers(api_key, user_agent, additional_headers)?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .referer(false)
            .timeout(timeout)
            .connect_timeout(timeout)
            .read_timeout(timeout)
            .pool_max_idle_per_host(0)
            .no_gzip()
            .no_brotli()
            .no_zstd()
            .no_deflate()
            .resolve_to_addrs(host, &addresses)
            .build()
            .map_err(|_| ProviderTransportError::new("invalid_request"))?;
        let response = client
            .get(url)
            .headers(headers)
            .send()
            .await
            .map_err(map_reqwest_error)?;
        bounded_json_response(response, max_response_bytes).await
    }

    async fn destination_addresses(
        &self,
        host: &str,
        port: u16,
    ) -> Result<Vec<SocketAddr>, ProviderTransportError> {
        let resolved = if let Ok(address) = IpAddr::from_str(host) {
            vec![SocketAddr::new(address, port)]
        } else {
            self.resolver
                .resolve(host, port)
                .await
                .map_err(|_| ProviderTransportError::new("destination_blocked"))?
        };
        Ok(resolved
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }

    #[cfg(test)]
    fn with_resolver(resolver: Arc<dyn ProviderDnsResolver>) -> Self {
        Self { resolver }
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

#[derive(Debug, Clone, Copy)]
struct ProviderHandler {
    models_path: &'static str,
    model_resource_prefix: Option<&'static str>,
    additional_headers: &'static [(&'static str, &'static str)],
}

impl ProviderHandler {
    fn normalize_model_id<'a>(&self, model_id: &'a str) -> &'a str {
        self.model_resource_prefix
            .and_then(|prefix| model_id.strip_prefix(prefix))
            .filter(|value| !value.is_empty())
            .unwrap_or(model_id)
    }
}

fn provider_handler(adapter_id: &str) -> Option<ProviderHandler> {
    let common = ProviderHandler {
        models_path: "/models",
        model_resource_prefix: None,
        additional_headers: &[],
    };
    match adapter_id {
        OPENAI_RESPONSES_ADAPTER
        | OPENAI_COMPATIBLE_ADAPTER
        | OPENAI_COMPATIBLE_JSON_SCHEMA_ADAPTER => Some(common),
        GOOGLE_GEMINI_OPENAI_ADAPTER | GOOGLE_GEMINI_OPENAI_JSON_SCHEMA_ADAPTER => {
            Some(ProviderHandler {
                model_resource_prefix: Some("models/"),
                additional_headers: GEMINI_HEADERS,
                ..common
            })
        }
        _ => None,
    }
}

#[derive(Debug)]
struct JsonHttpResponse {
    status: StatusCode,
    payload: Value,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct ProviderTransportError {
    code: &'static str,
}

impl ProviderTransportError {
    const fn new(code: &'static str) -> Self {
        Self { code }
    }

    const fn code(self) -> &'static str {
        self.code
    }
}

impl Display for ProviderTransportError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)
    }
}

impl std::error::Error for ProviderTransportError {}

fn request_headers(
    api_key: &str,
    user_agent: &str,
    additional_headers: &[(&str, &str)],
) -> Result<HeaderMap, ProviderTransportError> {
    if api_key.is_empty()
        || api_key.len() > 4_096
        || api_key.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(ProviderTransportError::new("invalid_request_headers"));
    }
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {api_key}"))
            .map_err(|_| ProviderTransportError::new("invalid_request_headers"))?,
    );
    headers.insert(
        USER_AGENT,
        HeaderValue::from_str(user_agent)
            .map_err(|_| ProviderTransportError::new("invalid_request_headers"))?,
    );
    for (name, value) in additional_headers {
        if !name.eq_ignore_ascii_case("x-goog-api-client")
            || value.is_empty()
            || value.len() > 256
            || value.bytes().any(|byte| byte.is_ascii_control())
        {
            return Err(ProviderTransportError::new("invalid_request_headers"));
        }
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| ProviderTransportError::new("invalid_request_headers"))?;
        let value = HeaderValue::from_str(value)
            .map_err(|_| ProviderTransportError::new("invalid_request_headers"))?;
        headers.insert(name, value);
    }
    Ok(headers)
}

async fn bounded_json_response(
    response: reqwest::Response,
    maximum: usize,
) -> Result<JsonHttpResponse, ProviderTransportError> {
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(ProviderTransportError::new("response_too_large"));
    }
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(map_reqwest_error)?;
        if body.len().saturating_add(chunk.len()) > maximum {
            return Err(ProviderTransportError::new("response_too_large"));
        }
        body.extend_from_slice(&chunk);
    }
    let payload = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(_) if !status.is_success() => Value::Null,
        Err(_) => return Err(ProviderTransportError::new("invalid_response")),
    };
    Ok(JsonHttpResponse { status, payload })
}

fn map_reqwest_error(error: reqwest::Error) -> ProviderTransportError {
    if error.is_timeout() {
        ProviderTransportError::new("timeout")
    } else if error.is_builder() {
        ProviderTransportError::new("invalid_request")
    } else {
        ProviderTransportError::new("network_error")
    }
}

fn safe_http_error_code(
    status: StatusCode,
    not_found_code: &'static str,
    payload: &Value,
) -> &'static str {
    if status.is_redirection() {
        return "redirect_blocked";
    }
    if status == StatusCode::UNAUTHORIZED {
        return "unauthorized";
    }
    if status == StatusCode::FORBIDDEN {
        return "forbidden";
    }
    if let Some(code) = safe_provider_error_code(payload) {
        return code;
    }
    if status == StatusCode::NOT_FOUND {
        return not_found_code;
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return "rate_limited";
    }
    if matches!(
        status,
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
    ) {
        return "invalid_request";
    }
    "upstream_error"
}

fn safe_provider_error_code(payload: &Value) -> Option<&'static str> {
    let root = payload.as_object()?;
    let details = root.get("error").and_then(Value::as_object).unwrap_or(root);
    for key in ["code", "type", "status"] {
        let Some(value) = details.get(key).and_then(Value::as_str) else {
            continue;
        };
        if value.len() > 128 {
            continue;
        }
        let normalized = value.trim().to_ascii_lowercase();
        let mapped = match normalized.as_str() {
            "api_error" | "server_error" => "upstream_error",
            "authentication_error" | "invalid_api_key" => "unauthorized",
            "deadline_exceeded" => "provider_timeout",
            "failed_precondition" => "failed_precondition",
            "insufficient_quota" | "quota_exceeded" => "quota_exceeded",
            "invalid_argument" | "invalid_request" | "invalid_request_error" | "invalid_value" => {
                "invalid_request"
            }
            "model_not_found" => "model_not_found",
            "parameter_unknown" | "unsupported_parameter" => "parameter_unknown",
            "permission_denied" => "forbidden",
            "rate_limit_exceeded" | "resource_exhausted" => "rate_limited",
            "service_unavailable" | "unavailable" => "service_unavailable",
            "unimplemented" => "unsupported_provider_feature",
            _ => continue,
        };
        return Some(mapped);
    }
    None
}

fn failed_verification(error_code: &str) -> ProviderVerificationResult {
    ProviderVerificationResult {
        verified: false,
        error_code: Some(error_code.to_owned()),
        models: Vec::new(),
        capability_ids: Vec::new(),
    }
}

fn is_global(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_global_v4(address),
        IpAddr::V6(address) => is_global_v6(address),
    }
}

fn is_global_v4(address: Ipv4Addr) -> bool {
    let [first, second, third, fourth] = address.octets();
    !(address.is_unspecified()
        || address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_broadcast()
        || address.is_documentation()
        || address.is_multicast()
        || first == 0
        || (first == 100 && (64..=127).contains(&second))
        || (first == 192 && second == 0 && third == 0)
        || (first == 198 && matches!(second, 18 | 19))
        || first >= 240
        || [first, second, third, fourth] == [255, 255, 255, 255])
}

fn is_global_v6(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_global_v4(mapped);
    }
    let segments = address.segments();
    !address.is_unspecified()
        && !address.is_loopback()
        && !address.is_multicast()
        && segments[0] & 0xe000 == 0x2000
        && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
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
    use std::error::Error;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::routing::get;
    use music_application::assistant::{
        GOOGLE_GEMINI_OPENAI_ADAPTER, OPENAI_COMPATIBLE_ADAPTER, OPENAI_RESPONSES_ADAPTER,
        ProviderConnectionPolicy, ProviderSecret, ProviderVerificationTarget,
    };
    use tokio::net::TcpListener;

    use super::*;

    #[derive(Debug)]
    struct FixedResolver {
        addresses: Vec<SocketAddr>,
    }

    impl ProviderDnsResolver for FixedResolver {
        fn resolve<'a>(&'a self, _host: &'a str, _port: u16) -> ResolveFuture<'a> {
            let addresses = self.addresses.clone();
            Box::pin(async move { Ok(addresses) })
        }
    }

    async fn test_server(
        app: Router,
    ) -> Result<(SocketAddr, tokio::task::JoinHandle<()>), Box<dyn Error + Send + Sync>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let task = tokio::spawn(async move {
            let _result = axum::serve(listener, app).await;
        });
        Ok((address, task))
    }

    fn target(base_url: String, allow_private_network: bool) -> ProviderVerificationTarget {
        ProviderVerificationTarget {
            connection_id: "aabbccddeeff00112233445566778899".to_owned(),
            adapter_id: OPENAI_COMPATIBLE_ADAPTER.to_owned(),
            base_url,
            api_key: ProviderSecret::new("secret-value"),
            allow_private_network,
            fingerprint: "f".repeat(64),
        }
    }

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

    #[test]
    fn public_destination_filter_is_conservative_for_special_ranges()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "169.254.1.1",
            "192.0.2.1",
            "198.18.0.1",
            "224.0.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!is_global(address.parse()?), "{address}");
        }
        for address in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(is_global(address.parse()?), "{address}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn verification_uses_pinned_dns_and_normalizes_models()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let app = Router::new().route(
            "/v1/models",
            get(|headers: HeaderMap| async move {
                assert_eq!(
                    headers
                        .get(AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer secret-value")
                );
                axum::Json(serde_json::json!({
                    "data": [
                        {"id": "model-b"},
                        {"id": "model-a"},
                        {"id": "model-b"},
                        {"id": ""},
                        {"other": "ignored"}
                    ]
                }))
            }),
        );
        let (address, server) = test_server(app).await?;
        let boundary = ProviderNetworkBoundary::with_resolver(Arc::new(FixedResolver {
            addresses: vec![address],
        }));
        let result = boundary
            .verify_provider_connection(&target(
                format!("http://provider.test:{}/v1", address.port()),
                true,
            ))
            .await;
        server.abort();
        assert!(result.verified);
        assert_eq!(result.models, ["model-b", "model-a"]);
        assert_eq!(result.capability_ids, ["structured-text/v1"]);
        Ok(())
    }

    #[tokio::test]
    async fn public_verification_rejects_any_non_global_dns_answer()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let boundary = ProviderNetworkBoundary::with_resolver(Arc::new(FixedResolver {
            addresses: vec!["127.0.0.1:443".parse()?],
        }));
        let result = boundary
            .verify_provider_connection(&target("https://provider.test/v1".to_owned(), false))
            .await;
        assert_eq!(result.error_code.as_deref(), Some("destination_blocked"));
        Ok(())
    }

    #[tokio::test]
    async fn verification_never_follows_redirects() -> Result<(), Box<dyn Error + Send + Sync>> {
        let hits = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/v1/models",
                get(|| async { axum::response::Redirect::temporary("/trap") }),
            )
            .route(
                "/trap",
                get({
                    let hits = Arc::clone(&hits);
                    move || {
                        let hits = Arc::clone(&hits);
                        async move {
                            hits.fetch_add(1, Ordering::SeqCst);
                            axum::Json(serde_json::json!({"data": []}))
                        }
                    }
                }),
            );
        let (address, server) = test_server(app).await?;
        let boundary = ProviderNetworkBoundary::with_resolver(Arc::new(FixedResolver {
            addresses: vec![address],
        }));
        let result = boundary
            .verify_provider_connection(&target(
                format!("http://provider.test:{}/v1", address.port()),
                true,
            ))
            .await;
        server.abort();
        assert_eq!(result.error_code.as_deref(), Some("redirect_blocked"));
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn bounded_reader_rejects_declared_oversized_responses()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let app = Router::new().route("/declared", get(|| async { "x".repeat(1_025) }));
        let (address, server) = test_server(app).await?;
        let boundary = ProviderNetworkBoundary::new();
        let error = boundary
            .get_json(
                &format!("http://{address}/declared"),
                "key",
                true,
                VERIFICATION_TIMEOUT,
                1_024,
                "test/1",
                &[],
            )
            .await
            .err()
            .ok_or("oversized response unexpectedly succeeded")?;
        server.abort();
        assert_eq!(error.code(), "response_too_large");
        Ok(())
    }

    #[test]
    fn provider_error_mapping_never_returns_provider_messages() {
        let payload = serde_json::json!({
            "error": {
                "type": "insufficient_quota",
                "message": "secret provider detail"
            }
        });
        let code = safe_http_error_code(StatusCode::BAD_REQUEST, "missing", &payload);
        assert_eq!(code, "quota_exceeded");
        assert!(!code.contains("secret provider detail"));
    }

    #[test]
    fn request_header_builder_rejects_secret_control_characters_and_unknown_headers()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        assert_eq!(
            request_headers("secret\nvalue", "test/1", &[])
                .err()
                .ok_or("control characters unexpectedly succeeded")?
                .code(),
            "invalid_request_headers"
        );
        assert_eq!(
            request_headers("secret", "test/1", &[("x-unsafe", "value")])
                .err()
                .ok_or("unknown headers unexpectedly succeeded")?
                .code(),
            "invalid_request_headers"
        );
        Ok(())
    }
}
