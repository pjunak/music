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
    ProviderConnectionPolicy, ProviderExecutionTarget, ProviderPolicyError,
    ProviderVerificationResult, ProviderVerificationTarget, StructuredModelRequest,
    StructuredModelResult, ThinkingMode, provider_adapter,
};
use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, USER_AGENT,
};
use reqwest::{StatusCode, Url};
use serde_json::Value;
use tokio::sync::Semaphore;

const GOOGLE_GEMINI_OPENAI_BASE_URL: &str =
    "https://generativelanguage.googleapis.com/v1beta/openai";
const OPENAI_API_BASE_URL: &str = "https://api.openai.com/v1";
const MAX_VERIFICATION_BYTES: usize = 1_024 * 1_024;
const MAX_VERIFIED_MODELS: usize = 200;
const MAX_REQUEST_BYTES: usize = 256 * 1_024;
const MAX_EXECUTION_RESPONSE_BYTES: usize = 2 * 1_024 * 1_024;
const VERIFICATION_TIMEOUT: Duration = Duration::from_secs(10);
const VERIFIER_USER_AGENT: &str = "music-assistant-provider-verifier/1";
const EXECUTOR_USER_AGENT: &str = "music-assistant-model-executor/1";
const GEMINI_HEADERS: &[(&str, &str)] = &[("x-goog-api-client", "music-assistant-oai/1.0")];
const PROVIDER_REQUEST_CONCURRENCY: usize = 4;

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
    request_slots: Arc<Semaphore>,
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
            request_slots: Arc::new(Semaphore::new(PROVIDER_REQUEST_CONCURRENCY)),
        }
    }

    pub(crate) async fn verify_provider_connection(
        &self,
        target: &ProviderVerificationTarget,
    ) -> ProviderVerificationResult {
        let Ok(_permit) = self.request_slots.try_acquire() else {
            return failed_verification("provider_busy");
        };
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

    pub(crate) async fn execute_structured_model_request(
        &self,
        target: &ProviderExecutionTarget,
        request: &StructuredModelRequest,
    ) -> StructuredModelResult {
        let Ok(_permit) = self.request_slots.try_acquire() else {
            return failed_structured_model("provider_busy");
        };
        let Some(handler) = provider_handler(&target.adapter_id) else {
            return failed_structured_model("unsupported_adapter");
        };
        let provider_schema = request
            .output_schema
            .as_ref()
            .map(|schema| handler.prepare_output_schema(schema));
        let schema_name = request.output_schema_name.as_deref();
        let response_format = match handler.structured_output_mode {
            StructuredOutputMode::JsonObject => serde_json::json!({"type": "json_object"}),
            StructuredOutputMode::JsonSchema => {
                let (Some(name), Some(schema)) = (schema_name, provider_schema.as_ref()) else {
                    return failed_structured_model("output_schema_required");
                };
                serde_json::json!({
                    "type": "json_schema",
                    "json_schema": {
                        "name": name,
                        "strict": true,
                        "schema": schema,
                    },
                })
            }
        };
        let maximum_output_tokens = request.max_output_tokens.min(target.max_output_tokens);
        let model_id = handler.normalize_model_id(&target.model_id);
        let mut payload = match handler.execution_api_style {
            ExecutionApiStyle::Responses => {
                let (Some(name), Some(schema)) = (schema_name, provider_schema.as_ref()) else {
                    return failed_structured_model("output_schema_required");
                };
                serde_json::json!({
                    "model": model_id,
                    "instructions": request.system_prompt,
                    "input": request.user_prompt,
                    "max_output_tokens": maximum_output_tokens,
                    "text": {
                        "format": {
                            "type": "json_schema",
                            "name": name,
                            "strict": true,
                            "schema": schema,
                        },
                    },
                    "store": false,
                })
            }
            ExecutionApiStyle::ChatCompletions => serde_json::json!({
                "model": model_id,
                "messages": [
                    {"role": "system", "content": request.system_prompt},
                    {"role": "user", "content": request.user_prompt},
                ],
                "max_tokens": maximum_output_tokens,
                "response_format": response_format,
            }),
        };
        handler.apply_thinking_mode(&mut payload, target.thinking_mode);
        let url = format!(
            "{}{}",
            target.base_url.trim_end_matches('/'),
            handler.completion_path
        );
        let response = self
            .post_json(
                &url,
                target.api_key.expose_secret(),
                target.allow_private_network,
                Duration::from_secs(u64::from(target.timeout_seconds)),
                MAX_EXECUTION_RESPONSE_BYTES,
                EXECUTOR_USER_AGENT,
                handler.additional_headers,
                &payload,
            )
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => return failed_structured_model(error.code()),
        };
        if !response.status.is_success() {
            return failed_structured_model(safe_http_error_code(
                response.status,
                "completion_endpoint_not_found",
                &response.payload,
            ));
        }
        match handler.execution_api_style {
            ExecutionApiStyle::Responses => parse_responses_result(&response.payload),
            ExecutionApiStyle::ChatCompletions => parse_chat_completions_result(&response.payload),
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
        self.request_json(
            raw_url,
            api_key,
            allow_private_network,
            timeout,
            max_response_bytes,
            user_agent,
            additional_headers,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn post_json(
        &self,
        raw_url: &str,
        api_key: &str,
        allow_private_network: bool,
        timeout: Duration,
        max_response_bytes: usize,
        user_agent: &str,
        additional_headers: &[(&str, &str)],
        payload: &Value,
    ) -> Result<JsonHttpResponse, ProviderTransportError> {
        self.request_json(
            raw_url,
            api_key,
            allow_private_network,
            timeout,
            max_response_bytes,
            user_agent,
            additional_headers,
            Some(payload),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn request_json(
        &self,
        raw_url: &str,
        api_key: &str,
        allow_private_network: bool,
        timeout: Duration,
        max_response_bytes: usize,
        user_agent: &str,
        additional_headers: &[(&str, &str)],
        payload: Option<&Value>,
    ) -> Result<JsonHttpResponse, ProviderTransportError> {
        let body = payload
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|_| ProviderTransportError::new("invalid_request"))?;
        if body
            .as_ref()
            .is_some_and(|body| body.len() > MAX_REQUEST_BYTES)
        {
            return Err(ProviderTransportError::new("request_too_large"));
        }
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
        let request = if let Some(body) = body {
            client
                .post(url)
                .header(CONTENT_TYPE, "application/json")
                .body(body)
        } else {
            client.get(url)
        };
        let response = request
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
        Self {
            resolver,
            request_slots: Arc::new(Semaphore::new(PROVIDER_REQUEST_CONCURRENCY)),
        }
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

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum StructuredOutputMode {
    JsonObject,
    JsonSchema,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ThinkingParameterStyle {
    ThinkingObject,
    ReasoningEffort,
    ReasoningObject,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ExecutionApiStyle {
    ChatCompletions,
    Responses,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum OutputSchemaDialect {
    Full,
    GeminiSubset,
}

#[derive(Debug, Clone, Copy)]
struct ProviderHandler {
    models_path: &'static str,
    completion_path: &'static str,
    model_resource_prefix: Option<&'static str>,
    additional_headers: &'static [(&'static str, &'static str)],
    structured_output_mode: StructuredOutputMode,
    thinking_parameter_style: ThinkingParameterStyle,
    execution_api_style: ExecutionApiStyle,
    output_schema_dialect: OutputSchemaDialect,
}

impl ProviderHandler {
    fn normalize_model_id<'a>(&self, model_id: &'a str) -> &'a str {
        self.model_resource_prefix
            .and_then(|prefix| model_id.strip_prefix(prefix))
            .filter(|value| !value.is_empty())
            .unwrap_or(model_id)
    }

    fn prepare_output_schema(&self, schema: &Value) -> Value {
        match self.output_schema_dialect {
            OutputSchemaDialect::Full => schema.clone(),
            OutputSchemaDialect::GeminiSubset => gemini_compatible_output_schema(schema),
        }
    }

    fn apply_thinking_mode(&self, payload: &mut Value, mode: ThinkingMode) {
        if mode == ThinkingMode::ProviderDefault {
            return;
        }
        let value = match self.thinking_parameter_style {
            ThinkingParameterStyle::ReasoningEffort => {
                ("reasoning_effort", json_string(thinking_effort(mode)))
            }
            ThinkingParameterStyle::ReasoningObject => (
                "reasoning",
                serde_json::json!({"effort": thinking_effort(mode)}),
            ),
            ThinkingParameterStyle::ThinkingObject => {
                ("thinking", serde_json::json!({"type": mode.as_str()}))
            }
        };
        if let Some(object) = payload.as_object_mut() {
            object.insert(value.0.to_owned(), value.1);
        }
    }
}

fn provider_handler(adapter_id: &str) -> Option<ProviderHandler> {
    let compatible = ProviderHandler {
        models_path: "/models",
        completion_path: "/chat/completions",
        model_resource_prefix: None,
        additional_headers: &[],
        structured_output_mode: StructuredOutputMode::JsonObject,
        thinking_parameter_style: ThinkingParameterStyle::ThinkingObject,
        execution_api_style: ExecutionApiStyle::ChatCompletions,
        output_schema_dialect: OutputSchemaDialect::Full,
    };
    match adapter_id {
        OPENAI_RESPONSES_ADAPTER => Some(ProviderHandler {
            completion_path: "/responses",
            structured_output_mode: StructuredOutputMode::JsonSchema,
            thinking_parameter_style: ThinkingParameterStyle::ReasoningObject,
            execution_api_style: ExecutionApiStyle::Responses,
            ..compatible
        }),
        OPENAI_COMPATIBLE_ADAPTER => Some(compatible),
        OPENAI_COMPATIBLE_JSON_SCHEMA_ADAPTER => Some(ProviderHandler {
            structured_output_mode: StructuredOutputMode::JsonSchema,
            ..compatible
        }),
        GOOGLE_GEMINI_OPENAI_ADAPTER | GOOGLE_GEMINI_OPENAI_JSON_SCHEMA_ADAPTER => {
            Some(ProviderHandler {
                model_resource_prefix: Some("models/"),
                additional_headers: GEMINI_HEADERS,
                structured_output_mode: StructuredOutputMode::JsonSchema,
                thinking_parameter_style: ThinkingParameterStyle::ReasoningEffort,
                output_schema_dialect: OutputSchemaDialect::GeminiSubset,
                ..compatible
            })
        }
        _ => None,
    }
}

fn thinking_effort(mode: ThinkingMode) -> &'static str {
    match mode {
        ThinkingMode::Enabled => "high",
        ThinkingMode::Disabled => "none",
        ThinkingMode::ProviderDefault => "",
    }
}

fn json_string(value: &str) -> Value {
    Value::String(value.to_owned())
}

fn gemini_compatible_output_schema(value: &Value) -> Value {
    match value {
        Value::Array(values) => {
            Value::Array(values.iter().map(gemini_compatible_output_schema).collect())
        }
        Value::Object(values) => {
            let mut compatible = serde_json::Map::new();
            for (key, value) in values {
                if matches!(
                    key.as_str(),
                    "exclusiveMinimum"
                        | "exclusiveMaximum"
                        | "maxLength"
                        | "minLength"
                        | "multipleOf"
                        | "pattern"
                        | "uniqueItems"
                ) {
                    continue;
                }
                if key == "const" {
                    if matches!(value, Value::String(_) | Value::Number(_)) {
                        compatible.insert("enum".to_owned(), Value::Array(vec![value.clone()]));
                    }
                    continue;
                }
                compatible.insert(key.clone(), gemini_compatible_output_schema(value));
            }
            Value::Object(compatible)
        }
        _ => value.clone(),
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

fn parse_chat_completions_result(payload: &Value) -> StructuredModelResult {
    let provider_model_id = optional_bounded_text(payload.get("model"), 256);
    let usage = payload.get("usage").and_then(Value::as_object);
    let input_tokens = usage
        .and_then(|usage| usage.get("prompt_tokens"))
        .and_then(Value::as_u64);
    let output_tokens = usage
        .and_then(|usage| usage.get("completion_tokens"))
        .and_then(Value::as_u64);
    let Some(choice) = payload
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(Value::as_object)
    else {
        return structured_model_result(
            false,
            Some("invalid_response"),
            None,
            provider_model_id,
            None,
            input_tokens,
            output_tokens,
        );
    };
    let finish_reason = optional_bounded_text(choice.get("finish_reason"), 64);
    let Some(content) = choice
        .get("message")
        .and_then(Value::as_object)
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
    else {
        return structured_model_result(
            false,
            Some("invalid_response"),
            None,
            provider_model_id,
            finish_reason,
            input_tokens,
            output_tokens,
        );
    };
    parse_structured_text(
        content,
        provider_model_id,
        finish_reason,
        input_tokens,
        output_tokens,
    )
}

fn parse_responses_result(payload: &Value) -> StructuredModelResult {
    let provider_model_id = optional_bounded_text(payload.get("model"), 256);
    let usage = payload.get("usage").and_then(Value::as_object);
    let input_tokens = usage
        .and_then(|usage| usage.get("input_tokens"))
        .and_then(Value::as_u64);
    let output_tokens = usage
        .and_then(|usage| usage.get("output_tokens"))
        .and_then(Value::as_u64);
    match payload.get("status").and_then(Value::as_str) {
        Some("failed") => {
            return structured_model_result(
                false,
                Some(safe_provider_error_code(payload).unwrap_or("upstream_error")),
                None,
                provider_model_id,
                None,
                input_tokens,
                output_tokens,
            );
        }
        Some("incomplete") => {
            let finish_reason = payload
                .get("incomplete_details")
                .and_then(Value::as_object)
                .and_then(|details| optional_bounded_text(details.get("reason"), 64));
            return structured_model_result(
                false,
                Some("incomplete_structured_output"),
                None,
                provider_model_id,
                finish_reason,
                input_tokens,
                output_tokens,
            );
        }
        Some("completed") => {}
        _ => {
            return structured_model_result(
                false,
                Some("invalid_response"),
                None,
                provider_model_id,
                None,
                input_tokens,
                output_tokens,
            );
        }
    }
    let Some(output) = payload.get("output").and_then(Value::as_array) else {
        return structured_model_result(
            false,
            Some("invalid_response"),
            None,
            provider_model_id,
            None,
            input_tokens,
            output_tokens,
        );
    };
    let mut text = String::new();
    let mut refused = false;
    for item in output {
        if item.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(parts) = item.get("content").and_then(Value::as_array) else {
            continue;
        };
        for part in parts {
            match part.get("type").and_then(Value::as_str) {
                Some("output_text") => {
                    if let Some(value) = part.get("text").and_then(Value::as_str) {
                        text.push_str(value);
                    }
                }
                Some("refusal") => refused = true,
                _ => {}
            }
        }
    }
    if text.is_empty() {
        return structured_model_result(
            false,
            Some(if refused {
                "model_refusal"
            } else {
                "empty_structured_output"
            }),
            None,
            provider_model_id,
            Some("stop".to_owned()),
            input_tokens,
            output_tokens,
        );
    }
    parse_structured_text(
        &text,
        provider_model_id,
        Some("stop".to_owned()),
        input_tokens,
        output_tokens,
    )
}

fn parse_structured_text(
    content: &str,
    provider_model_id: Option<String>,
    finish_reason: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
) -> StructuredModelResult {
    if content.trim().is_empty() {
        return structured_model_result(
            false,
            Some("empty_structured_output"),
            None,
            provider_model_id,
            finish_reason,
            input_tokens,
            output_tokens,
        );
    }
    let parsed = serde_json::from_str::<Value>(content);
    let payload = match parsed {
        Ok(Value::Object(values)) => Some(Value::Object(values)),
        Ok(_) => None,
        Err(_) => {
            let code = if matches!(finish_reason.as_deref(), Some("length" | "max_tokens")) {
                "incomplete_structured_output"
            } else {
                "invalid_structured_output"
            };
            return structured_model_result(
                false,
                Some(code),
                None,
                provider_model_id,
                finish_reason,
                input_tokens,
                output_tokens,
            );
        }
    };
    if payload.is_none() {
        return structured_model_result(
            false,
            Some("invalid_structured_output"),
            None,
            provider_model_id,
            finish_reason,
            input_tokens,
            output_tokens,
        );
    }
    structured_model_result(
        true,
        None,
        payload,
        provider_model_id,
        finish_reason,
        input_tokens,
        output_tokens,
    )
}

#[allow(clippy::too_many_arguments)]
fn structured_model_result(
    succeeded: bool,
    error_code: Option<&str>,
    payload: Option<Value>,
    provider_model_id: Option<String>,
    finish_reason: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
) -> StructuredModelResult {
    StructuredModelResult {
        succeeded,
        error_code: error_code.map(str::to_owned),
        payload,
        provider_model_id,
        finish_reason,
        input_tokens,
        output_tokens,
    }
}

fn failed_structured_model(error_code: &str) -> StructuredModelResult {
    structured_model_result(false, Some(error_code), None, None, None, None, None)
}

fn optional_bounded_text(value: Option<&Value>, maximum: usize) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= maximum)
        .map(str::to_owned)
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
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Json;
    use axum::Router;
    use axum::routing::{get, post};
    use music_application::assistant::{
        GOOGLE_GEMINI_OPENAI_ADAPTER, OPENAI_COMPATIBLE_ADAPTER, OPENAI_RESPONSES_ADAPTER,
        ProviderConnectionPolicy, ProviderExecutionTarget, ProviderSecret,
        ProviderVerificationTarget, StructuredModelRequest, ThinkingMode,
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

    fn execution_target(
        adapter_id: &str,
        base_url: String,
        thinking_mode: ThinkingMode,
    ) -> ProviderExecutionTarget {
        ProviderExecutionTarget {
            adapter_id: adapter_id.to_owned(),
            base_url,
            api_key: ProviderSecret::new("secret-value"),
            allow_private_network: true,
            model_id: "models/fixture-model".to_owned(),
            timeout_seconds: 10,
            max_output_tokens: 1_024,
            thinking_mode,
        }
    }

    fn structured_request() -> StructuredModelRequest {
        StructuredModelRequest {
            system_prompt: "Fixed system prompt".to_owned(),
            user_prompt: r#"{"fixture":true}"#.to_owned(),
            max_output_tokens: 256,
            output_schema_name: Some("fixture-response".to_owned()),
            output_schema: Some(serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["accepted"],
                "properties": {
                    "accepted": {"type": "boolean", "const": true},
                    "label": {"type": "string", "minLength": 1, "pattern": "^ok$"},
                },
            })),
        }
    }

    #[tokio::test]
    async fn saturated_provider_boundary_load_sheds_without_starting_network_work()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let boundary = ProviderNetworkBoundary::new();
        let _occupied = boundary
            .request_slots
            .acquire_many(PROVIDER_REQUEST_CONCURRENCY as u32)
            .await?;

        let verification = boundary
            .verify_provider_connection(&target(
                "https://provider.example.test/v1".to_owned(),
                false,
            ))
            .await;
        assert!(!verification.verified);
        assert_eq!(verification.error_code.as_deref(), Some("provider_busy"));

        let execution = boundary
            .execute_structured_model_request(
                &execution_target(
                    OPENAI_COMPATIBLE_ADAPTER,
                    "https://provider.example.test/v1".to_owned(),
                    ThinkingMode::ProviderDefault,
                ),
                &structured_request(),
            )
            .await;
        assert!(!execution.succeeded);
        assert_eq!(execution.error_code.as_deref(), Some("provider_busy"));
        Ok(())
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
    async fn generic_executor_shapes_chat_completions_and_parses_bounded_metadata()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let observed = Arc::new(Mutex::new(None));
        let app = Router::new().route(
            "/v1/chat/completions",
            post({
                let observed = Arc::clone(&observed);
                move |Json(payload): Json<Value>| {
                    let observed = Arc::clone(&observed);
                    async move {
                        let Ok(mut slot) = observed.lock() else {
                            return Err(StatusCode::INTERNAL_SERVER_ERROR);
                        };
                        *slot = Some(payload);
                        Ok(Json(serde_json::json!({
                            "model": "fixture-model",
                            "choices": [{
                                "finish_reason": "stop",
                                "message": {"content": "{\"accepted\":true}"}
                            }],
                            "usage": {"prompt_tokens": 17, "completion_tokens": 6}
                        })))
                    }
                }
            }),
        );
        let (address, server) = test_server(app).await?;
        let boundary = ProviderNetworkBoundary::new();
        let result = boundary
            .execute_structured_model_request(
                &execution_target(
                    OPENAI_COMPATIBLE_ADAPTER,
                    format!("http://{address}/v1"),
                    ThinkingMode::Disabled,
                ),
                &structured_request(),
            )
            .await;
        server.abort();
        assert!(result.succeeded);
        assert_eq!(result.payload, Some(serde_json::json!({"accepted": true})));
        assert_eq!(result.provider_model_id.as_deref(), Some("fixture-model"));
        assert_eq!(
            (result.input_tokens, result.output_tokens),
            (Some(17), Some(6))
        );
        let payload = observed
            .lock()
            .map_err(|_| "observed request lock was poisoned")?
            .clone()
            .ok_or("provider request was not observed")?;
        assert_eq!(payload["model"], "models/fixture-model");
        assert_eq!(payload["max_tokens"], 256);
        assert_eq!(payload["response_format"]["type"], "json_object");
        assert_eq!(payload["thinking"]["type"], "disabled");
        assert!(payload.get("instructions").is_none());
        Ok(())
    }

    #[tokio::test]
    async fn responses_executor_uses_native_schema_reasoning_and_storage_controls()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let observed = Arc::new(Mutex::new(None));
        let app = Router::new().route(
            "/v1/responses",
            post({
                let observed = Arc::clone(&observed);
                move |Json(payload): Json<Value>| {
                    let observed = Arc::clone(&observed);
                    async move {
                        let Ok(mut slot) = observed.lock() else {
                            return Err(StatusCode::INTERNAL_SERVER_ERROR);
                        };
                        *slot = Some(payload);
                        Ok(Json(serde_json::json!({
                            "status": "completed",
                            "model": "gpt-fixture",
                            "output": [{
                                "type": "message",
                                "content": [{
                                    "type": "output_text",
                                    "text": "{\"accepted\":true}"
                                }]
                            }],
                            "usage": {"input_tokens": 21, "output_tokens": 7}
                        })))
                    }
                }
            }),
        );
        let (address, server) = test_server(app).await?;
        let boundary = ProviderNetworkBoundary::new();
        let result = boundary
            .execute_structured_model_request(
                &execution_target(
                    OPENAI_RESPONSES_ADAPTER,
                    format!("http://{address}/v1"),
                    ThinkingMode::Enabled,
                ),
                &structured_request(),
            )
            .await;
        server.abort();
        assert!(result.succeeded);
        assert_eq!(result.finish_reason.as_deref(), Some("stop"));
        let payload = observed
            .lock()
            .map_err(|_| "observed request lock was poisoned")?
            .clone()
            .ok_or("provider request was not observed")?;
        assert_eq!(payload["instructions"], "Fixed system prompt");
        assert_eq!(payload["input"], r#"{"fixture":true}"#);
        assert_eq!(payload["max_output_tokens"], 256);
        assert_eq!(payload["text"]["format"]["type"], "json_schema");
        assert_eq!(payload["reasoning"]["effort"], "high");
        assert_eq!(payload["store"], false);
        assert!(payload.get("messages").is_none());
        Ok(())
    }

    #[test]
    fn gemini_profile_normalizes_models_and_projects_only_unsupported_schema_keywords()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let handler = provider_handler(GOOGLE_GEMINI_OPENAI_ADAPTER)
            .ok_or("missing Gemini provider handler")?;
        assert_eq!(
            handler.normalize_model_id("models/gemini-fixture"),
            "gemini-fixture"
        );
        let request = structured_request();
        let projected = handler.prepare_output_schema(
            request
                .output_schema
                .as_ref()
                .ok_or("missing fixture schema")?,
        );
        assert!(projected.pointer("/properties/label/minLength").is_none());
        assert!(projected.pointer("/properties/label/pattern").is_none());
        assert!(projected.pointer("/properties/accepted/const").is_none());
        assert!(projected.pointer("/properties/accepted/enum").is_none());
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
