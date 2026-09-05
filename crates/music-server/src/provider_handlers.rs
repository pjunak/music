use std::collections::BTreeSet;

use music_application::assistant::{
    GOOGLE_GEMINI_OPENAI_ADAPTER, GOOGLE_GEMINI_OPENAI_JSON_SCHEMA_ADAPTER,
    OPENAI_COMPATIBLE_ADAPTER, OPENAI_COMPATIBLE_JSON_SCHEMA_ADAPTER, OPENAI_RESPONSES_ADAPTER,
    StructuredModelRequest, StructuredModelResult, ThinkingMode,
};
use serde_json::Value;

const GEMINI_HEADERS: &[(&str, &str)] = &[("x-goog-api-client", "music-assistant-oai/1.0")];
pub(crate) const MAX_REQUEST_BYTES: usize = 256 * 1_024;

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
    OpenAiStructuredOutputsSubset,
    GeminiSubset,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProviderHandler {
    models_path: &'static str,
    completion_path: &'static str,
    model_resource_prefix: Option<&'static str>,
    additional_headers: &'static [(&'static str, &'static str)],
    structured_output_mode: StructuredOutputMode,
    thinking_parameter_style: ThinkingParameterStyle,
    execution_api_style: ExecutionApiStyle,
    output_schema_dialect: OutputSchemaDialect,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ProviderHandlerError {
    code: &'static str,
}

impl ProviderHandlerError {
    #[must_use]
    pub(crate) const fn code(&self) -> &'static str {
        self.code
    }
}

impl std::fmt::Display for ProviderHandlerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code)
    }
}

impl std::error::Error for ProviderHandlerError {}

#[derive(Debug)]
pub(crate) struct PreparedProviderRequest {
    pub(crate) path: &'static str,
    pub(crate) additional_headers: &'static [(&'static str, &'static str)],
    pub(crate) payload: Value,
}

impl ProviderHandler {
    #[must_use]
    pub(crate) const fn models_path(self) -> &'static str {
        self.models_path
    }

    #[must_use]
    pub(crate) const fn additional_headers(self) -> &'static [(&'static str, &'static str)] {
        self.additional_headers
    }

    #[must_use]
    pub(crate) fn normalize_model_id(self, model_id: &str) -> &str {
        self.model_resource_prefix
            .and_then(|prefix| model_id.strip_prefix(prefix))
            .filter(|value| !value.is_empty())
            .unwrap_or(model_id)
    }

    #[must_use]
    pub(crate) fn parse_models(self, payload: &Value, maximum: usize) -> Option<Vec<String>> {
        let entries = payload.get("data").and_then(Value::as_array)?;
        let mut seen = BTreeSet::new();
        Some(
            entries
                .iter()
                .filter_map(|item| item.get("id").and_then(Value::as_str))
                .map(|model_id| self.normalize_model_id(model_id))
                .filter(|model_id| !model_id.is_empty() && model_id.len() <= 256)
                .filter(|model_id| seen.insert((*model_id).to_owned()))
                .take(maximum)
                .map(str::to_owned)
                .collect(),
        )
    }

    pub(crate) fn prepare_structured_request(
        self,
        model_id: &str,
        target_max_output_tokens: u32,
        thinking_mode: ThinkingMode,
        request: &StructuredModelRequest,
    ) -> Result<PreparedProviderRequest, ProviderHandlerError> {
        let provider_schema = request
            .output_schema
            .as_ref()
            .map(|schema| self.prepare_output_schema(schema));
        let schema_name = request.output_schema_name.as_deref();
        let response_format = match self.structured_output_mode {
            StructuredOutputMode::JsonObject => serde_json::json!({"type": "json_object"}),
            StructuredOutputMode::JsonSchema => {
                let (Some(name), Some(schema)) = (schema_name, provider_schema.as_ref()) else {
                    return Err(ProviderHandlerError {
                        code: "output_schema_required",
                    });
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
        let maximum_output_tokens = request.max_output_tokens.min(target_max_output_tokens);
        let normalized_model_id = self.normalize_model_id(model_id);
        let mut payload = match self.execution_api_style {
            ExecutionApiStyle::Responses => {
                let (Some(name), Some(schema)) = (schema_name, provider_schema.as_ref()) else {
                    return Err(ProviderHandlerError {
                        code: "output_schema_required",
                    });
                };
                serde_json::json!({
                    "model": normalized_model_id,
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
                "model": normalized_model_id,
                "messages": [
                    {"role": "system", "content": request.system_prompt},
                    {"role": "user", "content": request.user_prompt},
                ],
                "max_tokens": maximum_output_tokens,
                "response_format": response_format,
            }),
        };
        self.apply_thinking_mode(&mut payload, thinking_mode);
        let bytes = serde_json::to_vec(&payload).map_err(|_| ProviderHandlerError {
            code: "invalid_request",
        })?;
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(ProviderHandlerError {
                code: "request_too_large",
            });
        }
        Ok(PreparedProviderRequest {
            path: self.completion_path,
            additional_headers: self.additional_headers,
            payload,
        })
    }

    #[must_use]
    pub(crate) fn parse_structured_response(self, payload: &Value) -> StructuredModelResult {
        match self.execution_api_style {
            ExecutionApiStyle::Responses => parse_responses_result(payload),
            ExecutionApiStyle::ChatCompletions => parse_chat_completions_result(payload),
        }
    }

    fn prepare_output_schema(self, schema: &Value) -> Value {
        match self.output_schema_dialect {
            OutputSchemaDialect::Full => schema.clone(),
            OutputSchemaDialect::OpenAiStructuredOutputsSubset => {
                openai_compatible_output_schema(schema)
            }
            OutputSchemaDialect::GeminiSubset => gemini_compatible_output_schema(schema),
        }
    }

    fn apply_thinking_mode(self, payload: &mut Value, mode: ThinkingMode) {
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

pub(crate) fn validate_structured_request(
    target: &music_application::assistant::ProviderExecutionTarget,
    request: &StructuredModelRequest,
) -> Result<(), music_application::assistant::ModelTaskError> {
    let handler = provider_handler(&target.adapter_id)
        .ok_or_else(|| music_application::assistant::ModelTaskError::new("unsupported_adapter"))?;
    handler
        .prepare_structured_request(
            &target.model_id,
            target.max_output_tokens,
            target.thinking_mode,
            request,
        )
        .map(|_| ())
        .map_err(|error| music_application::assistant::ModelTaskError::new(error.code()))
}

#[must_use]
pub(crate) fn provider_handler(adapter_id: &str) -> Option<ProviderHandler> {
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
            output_schema_dialect: OutputSchemaDialect::OpenAiStructuredOutputsSubset,
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

fn openai_compatible_output_schema(value: &Value) -> Value {
    project_output_schema(value, &["uniqueItems"], false)
}

fn gemini_compatible_output_schema(value: &Value) -> Value {
    project_output_schema(
        value,
        &[
            "exclusiveMinimum",
            "exclusiveMaximum",
            "maxLength",
            "minLength",
            "multipleOf",
            "pattern",
            "uniqueItems",
        ],
        true,
    )
}

fn project_output_schema(
    value: &Value,
    unsupported_keywords: &[&str],
    scalar_const_as_enum: bool,
) -> Value {
    match value {
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| {
                    project_output_schema(value, unsupported_keywords, scalar_const_as_enum)
                })
                .collect(),
        ),
        Value::Object(values) => {
            let mut compatible = serde_json::Map::new();
            for (key, value) in values {
                if unsupported_keywords.contains(&key.as_str()) {
                    continue;
                }
                if scalar_const_as_enum && key == "const" {
                    if matches!(value, Value::String(_) | Value::Number(_)) {
                        compatible.insert("enum".to_owned(), Value::Array(vec![value.clone()]));
                    }
                    continue;
                }
                compatible.insert(
                    key.clone(),
                    project_output_schema(value, unsupported_keywords, scalar_const_as_enum),
                );
            }
            Value::Object(compatible)
        }
        _ => value.clone(),
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
    let payload = match serde_json::from_str::<Value>(content) {
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
        outcome: music_application::assistant::ProviderAttemptOutcome::ResponseReceived,
        succeeded,
        error_code: error_code.map(str::to_owned),
        payload,
        provider_model_id,
        finish_reason,
        input_tokens,
        output_tokens,
    }
}

fn optional_bounded_text(value: Option<&Value>, maximum: usize) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= maximum)
        .map(str::to_owned)
}

#[must_use]
pub(crate) fn safe_provider_error_code(payload: &Value) -> Option<&'static str> {
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

#[cfg(test)]
mod tests {
    use std::error::Error;

    use music_application::assistant::{
        GOOGLE_GEMINI_OPENAI_ADAPTER, ModelTaggerBatch, OPENAI_COMPATIBLE_JSON_SCHEMA_ADAPTER,
        OPENAI_RESPONSES_ADAPTER, StructuredModelRequest, ThinkingMode,
        default_vocabulary_snapshot,
    };
    use serde_json::{Value, json};

    use super::provider_handler;

    #[test]
    fn every_adapter_preserves_twenty_tracks_and_the_full_two_hundred_tag_vocabulary()
    -> Result<(), Box<dyn Error>> {
        use music_application::assistant::{
            PROVIDER_ADAPTERS, TagQualityVocabulary, plan_model_tagger_batches,
        };
        let vocabulary = TagQualityVocabulary::Maximum.snapshot()?;
        let inputs = (1..=20)
            .map(|id| {
                json!({
                    "track_id": id, "artist": "Équipe 静かな音楽", "album": "Archive cue 196",
                    "origin": "", "genre": "instrumental", "length_s": 180, "bpm": null
                })
            })
            .collect::<Vec<_>>();
        for adapter in PROVIDER_ADAPTERS {
            let handler = provider_handler(adapter.id).ok_or("adapter missing handler")?;
            let planned = plan_model_tagger_batches(&inputs, &vocabulary, |request| {
                handler
                    .prepare_structured_request(
                        "test-model",
                        8000,
                        ThinkingMode::ProviderDefault,
                        request,
                    )
                    .map(|_| ())
                    .map_err(|error| {
                        music_application::assistant::ModelTaskError::new(error.code())
                    })
            })?;
            assert_eq!(planned.len(), 1, "{}", adapter.id);
            assert_eq!(planned[0].input_range, 0..20);
            for correction in [false, true] {
                let request = planned[0].task.request(correction);
                let prepared = handler.prepare_structured_request(
                    "test-model",
                    8000,
                    ThinkingMode::ProviderDefault,
                    &request,
                )?;
                let bytes = serde_json::to_vec(&prepared.payload)?;
                assert!(bytes.len() <= super::MAX_REQUEST_BYTES);
                let payload = String::from_utf8(bytes)?;
                assert!(payload.contains("archive.cue_196"));
                assert!(payload.contains("study.focus"));
                let input: Value = serde_json::from_str(&request.user_prompt)?;
                assert_eq!(
                    input["tracks"].as_array().ok_or("tracks missing")?.len(),
                    20
                );
                assert_eq!(
                    input["vocabulary_groups"]
                        .as_array()
                        .ok_or("groups missing")?
                        .iter()
                        .flat_map(|group| group["tags"].as_array().into_iter().flatten())
                        .count(),
                    200
                );
            }
        }
        Ok(())
    }

    #[test]
    fn a_valid_large_vocabulary_is_rejected_by_every_actual_adapter_before_execution()
    -> Result<(), Box<dyn Error>> {
        use music_application::assistant::{
            TAG_VOCABULARY_SCHEMA, TagVocabularyDocument, TagVocabularyEntry, TagVocabularyGroup,
            TagVocabularySnapshot, plan_model_tagger_batches, vocabulary_fingerprint,
        };
        let document = TagVocabularyDocument {
            schema_version: TAG_VOCABULARY_SCHEMA.to_owned(),
            groups: (0..2)
                .map(|group| TagVocabularyGroup {
                    key: format!("group{group}"),
                    label: format!("Group {group}"),
                    description: "Synthetic group".to_owned(),
                    tags: (0..100)
                        .map(|tag| TagVocabularyEntry {
                            id: format!("g{group}.tag{tag}"),
                            name: format!("group {group} tag {tag}"),
                            description: "Synthetic tag".to_owned(),
                            aliases: vec![],
                            context_cues: (0..32)
                                .map(|cue| format!("{cue:02} {}", "x".repeat(57)))
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
        }
        .normalized()?;
        let vocabulary = TagVocabularySnapshot {
            revision: 1,
            fingerprint: vocabulary_fingerprint(&document)?,
            document,
        };
        let inputs = vec![
            json!({"track_id": 1, "artist": "", "album": "", "origin": "", "genre": "", "length_s": 120.0}),
        ];
        for adapter in [
            super::OPENAI_COMPATIBLE_ADAPTER,
            super::OPENAI_COMPATIBLE_JSON_SCHEMA_ADAPTER,
            super::GOOGLE_GEMINI_OPENAI_ADAPTER,
            super::GOOGLE_GEMINI_OPENAI_JSON_SCHEMA_ADAPTER,
            super::OPENAI_RESPONSES_ADAPTER,
        ] {
            let handler = provider_handler(adapter).ok_or("missing adapter")?;
            let result = plan_model_tagger_batches(&inputs, &vocabulary, |request| {
                handler
                    .prepare_structured_request("test-model", 8000, ThinkingMode::Enabled, request)
                    .map(|_| ())
                    .map_err(|error| {
                        music_application::assistant::ModelTaskError::new(error.code())
                    })
            });
            assert_eq!(
                result.err().ok_or("oversized request accepted")?.code,
                "request_too_large",
                "{adapter}"
            );
        }
        Ok(())
    }

    fn production_tagging_request() -> Result<StructuredModelRequest, Box<dyn Error>> {
        let tracks = (1..=20)
            .map(|track_id| {
                json!({
                    "track_id": track_id,
                    "artist": "",
                    "album": "",
                    "origin": "",
                    "genre": "",
                    "length_s": 120.0,
                })
            })
            .collect();
        Ok(ModelTaggerBatch::new(tracks, default_vocabulary_snapshot()?)?.request(false))
    }

    fn contains_keyword(value: &Value, keyword: &str) -> bool {
        match value {
            Value::Array(values) => values.iter().any(|value| contains_keyword(value, keyword)),
            Value::Object(values) => {
                values.contains_key(keyword)
                    || values
                        .values()
                        .any(|value| contains_keyword(value, keyword))
            }
            _ => false,
        }
    }

    #[test]
    fn openai_handler_projects_the_production_tagger_schema_without_weakening_supported_bounds()
    -> Result<(), Box<dyn Error>> {
        let handler =
            provider_handler(OPENAI_RESPONSES_ADAPTER).ok_or("missing OpenAI provider handler")?;
        let prepared = handler.prepare_structured_request(
            "gpt-fixture",
            8_000,
            ThinkingMode::Disabled,
            &production_tagging_request()?,
        )?;
        let schema = &prepared.payload["text"]["format"]["schema"];
        assert!(!contains_keyword(schema, "uniqueItems"));
        assert_eq!(
            schema.pointer("/properties/tracks/minItems"),
            Some(&json!(20))
        );
        assert_eq!(
            schema.pointer("/properties/tracks/maxItems"),
            Some(&json!(20))
        );
        assert_eq!(
            schema.pointer("/properties/tracks/items/properties/tag_ids/maxItems"),
            Some(&json!(8))
        );
        assert_eq!(
            schema.pointer("/properties/tracks/items/properties/evidence/items/minLength"),
            Some(&json!(1))
        );
        Ok(())
    }

    #[test]
    fn generic_strict_handler_preserves_the_canonical_schema() -> Result<(), Box<dyn Error>> {
        let handler = provider_handler(OPENAI_COMPATIBLE_JSON_SCHEMA_ADAPTER)
            .ok_or("missing strict compatible provider handler")?;
        let prepared = handler.prepare_structured_request(
            "fixture",
            8_000,
            ThinkingMode::ProviderDefault,
            &production_tagging_request()?,
        )?;
        assert!(contains_keyword(
            &prepared.payload["response_format"]["json_schema"]["schema"],
            "uniqueItems"
        ));
        Ok(())
    }

    #[test]
    fn gemini_handler_normalizes_models_and_projects_its_documented_subset()
    -> Result<(), Box<dyn Error>> {
        let handler = provider_handler(GOOGLE_GEMINI_OPENAI_ADAPTER)
            .ok_or("missing Gemini provider handler")?;
        assert_eq!(
            handler.normalize_model_id("models/gemini-fixture"),
            "gemini-fixture"
        );
        let prepared = handler.prepare_structured_request(
            "models/gemini-fixture",
            8_000,
            ThinkingMode::ProviderDefault,
            &production_tagging_request()?,
        )?;
        let schema = &prepared.payload["response_format"]["json_schema"]["schema"];
        assert!(!contains_keyword(schema, "uniqueItems"));
        assert!(!contains_keyword(schema, "minLength"));
        assert!(!contains_keyword(schema, "maxLength"));
        Ok(())
    }
}
