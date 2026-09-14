use music_application::assistant::{
    DEEPSEEK_CHAT_ADAPTER, DEEPSEEK_RESPONSES_ADAPTER, OPENAI_RESPONSES_ADAPTER,
    ProviderConformanceTarget, ProviderExecutionTarget, ProviderSecret, ThinkingMode,
};
use serde_json::{Value, json};

use super::provider_handler;

fn challenge(adapter: &str, model: &str, mode: ThinkingMode) -> ProviderConformanceTarget {
    ProviderConformanceTarget {
        role_id: "music_tagger".to_owned(),
        execution: ProviderExecutionTarget {
            adapter_id: adapter.to_owned(),
            base_url: "https://api.deepseek.com".to_owned(),
            api_key: ProviderSecret::new("fixture".to_owned()),
            allow_private_network: false,
            model_id: model.to_owned(),
            timeout_seconds: 60,
            max_output_tokens: 8000,
            thinking_mode: mode,
        },
        challenge: "fixture-challenge".to_owned(),
        runtime_fingerprint: "fixture".to_owned(),
        role_configuration_fingerprint: "fixture".to_owned(),
        connection_fingerprint: "fixture".to_owned(),
    }
}

#[test]
fn conformance_preserves_operator_budget_and_provider_specific_parameters()
-> Result<(), Box<dyn std::error::Error>> {
    for (adapter, path) in [
        (DEEPSEEK_CHAT_ADAPTER, "/chat/completions"),
        (DEEPSEEK_RESPONSES_ADAPTER, "/responses"),
        (OPENAI_RESPONSES_ADAPTER, "/responses"),
    ] {
        let model = if adapter == OPENAI_RESPONSES_ADAPTER {
            "gpt-6-astra"
        } else {
            "deepseek-flash"
        };
        let target = challenge(adapter, model, ThinkingMode::Low);
        let handler = provider_handler(adapter).ok_or("handler missing")?;
        let prepared = handler.prepare_structured_request(
            model,
            8000,
            ThinkingMode::Low,
            &target.request(),
        )?;
        assert_eq!(prepared.path, path);
        let body = prepared.payload;
        if adapter == DEEPSEEK_CHAT_ADAPTER {
            assert_eq!(body["max_tokens"], 8000);
            assert_eq!(body["thinking"]["type"], "enabled");
            assert_eq!(body["reasoning_effort"], "low");
            assert_eq!(body["response_format"]["type"], "json_object");
        } else {
            assert_eq!(body["max_output_tokens"], 8000);
            assert_eq!(body["reasoning"]["effort"], "low");
            assert_eq!(body["text"]["format"]["type"], "json_schema");
            assert_eq!(body["store"], false);
            if adapter == DEEPSEEK_RESPONSES_ADAPTER {
                assert!(body["text"]["format"].get("strict").is_none());
            } else {
                assert_eq!(body["text"]["format"]["strict"], true);
            }
        }
        assert!(body.get("prompt_cache_options").is_none());
        assert!(body.get("temperature").is_none());
    }
    Ok(())
}

#[test]
fn deepseek_off_and_default_have_distinct_wire_semantics() -> Result<(), Box<dyn std::error::Error>>
{
    let handler = provider_handler(DEEPSEEK_CHAT_ADAPTER).ok_or("handler missing")?;
    for mode in [ThinkingMode::Disabled, ThinkingMode::ProviderDefault] {
        let target = challenge(DEEPSEEK_CHAT_ADAPTER, "deepseek-flash", mode);
        let body = handler
            .prepare_structured_request("deepseek-flash", 8000, mode, &target.request())?
            .payload;
        assert!(body.get("reasoning_effort").is_none());
        if mode == ThinkingMode::Disabled {
            assert_eq!(body["thinking"]["type"], "disabled");
        } else {
            assert!(body.get("thinking").is_none());
        }
        let handler = provider_handler(DEEPSEEK_RESPONSES_ADAPTER).ok_or("handler missing")?;
        let body = handler
            .prepare_structured_request("deepseek-flash", 8000, mode, &target.request())?
            .payload;
        if mode == ThinkingMode::Disabled {
            assert_eq!(body["reasoning"]["effort"], "none");
        } else {
            assert!(body.get("reasoning").is_none());
        }
    }
    let handler = provider_handler(OPENAI_RESPONSES_ADAPTER).ok_or("handler missing")?;
    let target = challenge(
        OPENAI_RESPONSES_ADAPTER,
        "gpt-6-astra",
        ThinkingMode::Disabled,
    );
    assert_eq!(
        handler
            .prepare_structured_request(
                "gpt-6-astra",
                8000,
                ThinkingMode::Disabled,
                &target.request()
            )
            .err()
            .ok_or("unsupported mode accepted")?
            .code(),
        "unsupported_reasoning_mode"
    );
    Ok(())
}

#[test]
fn termination_is_classified_before_null_empty_or_parseable_content()
-> Result<(), Box<dyn std::error::Error>> {
    let handler = provider_handler(DEEPSEEK_CHAT_ADAPTER).ok_or("handler missing")?;
    for (reason, error) in [
        ("length", "incomplete_structured_output"),
        ("content_filter", "model_refusal"),
        ("aborted", "provider_interrupted"),
        ("insufficient_system_resource", "provider_interrupted"),
        ("tool_calls", "unexpected_tool_call"),
        ("unknown", "invalid_response"),
    ] {
        for content in [Value::Null, json!(""), json!("{}"), json!("{\"partial\":")] {
            let result = handler.parse_structured_response(&json!({
                "model":"deepseek-flash", "choices":[{"finish_reason":reason,"message":{"content":content,"reasoning_content":"private reasoning"}}],
                "usage":{"prompt_tokens":100,"completion_tokens":8000,"prompt_cache_hit_tokens":80,"completion_tokens_details":{"reasoning_tokens":7990}}
            }));
            assert!(!result.succeeded);
            assert_eq!(result.error_code.as_deref(), Some(error));
            assert!(result.payload.is_none());
            assert_eq!(result.output_tokens, Some(8000));
            assert_eq!(result.token_details.reasoning_output_tokens, Some(7990));
            assert_eq!(result.token_details.cached_input_tokens, Some(80));
        }
    }
    Ok(())
}

#[test]
fn completed_responses_ignore_reasoning_but_reject_refusals_and_tool_calls()
-> Result<(), Box<dyn std::error::Error>> {
    let chat = provider_handler(DEEPSEEK_CHAT_ADAPTER).ok_or("handler missing")?;
    let result = chat.parse_structured_response(&json!({
        "choices":[{"finish_reason":"stop","message":{"content":"{}","tool_calls":[{"type":"function"}]}}]
    }));
    assert_eq!(result.error_code.as_deref(), Some("unexpected_tool_call"));
    for adapter in [OPENAI_RESPONSES_ADAPTER, DEEPSEEK_RESPONSES_ADAPTER] {
        let handler = provider_handler(adapter).ok_or("handler missing")?;
        let text = json!({"type":"message","content":[{"type":"output_text","text":"{\"accepted\":true}"}]});
        let mut payload = json!({"status":"completed","model":"fixture","output":[{"type":"reasoning","summary":[]},text]});
        assert_eq!(
            handler.parse_structured_response(&payload).payload,
            Some(json!({"accepted":true}))
        );
        payload["output"][0] = json!({"type":"function_call","arguments":"{}"});
        assert_eq!(
            handler
                .parse_structured_response(&payload)
                .error_code
                .as_deref(),
            Some("unexpected_tool_call")
        );
        payload["output"][0] =
            json!({"type":"message","content":[{"type":"refusal","refusal":"private message"}]});
        assert_eq!(
            handler
                .parse_structured_response(&payload)
                .error_code
                .as_deref(),
            Some("model_refusal")
        );
        payload["status"] = json!("incomplete");
        payload["incomplete_details"] = json!({"reason":"content_filter"});
        assert_eq!(
            handler
                .parse_structured_response(&payload)
                .error_code
                .as_deref(),
            Some("model_refusal")
        );
    }
    Ok(())
}

#[test]
fn cleanup_openai_request_requires_nullable_candidate() -> Result<(), Box<dyn std::error::Error>> {
    let handler = provider_handler(OPENAI_RESPONSES_ADAPTER).ok_or("handler missing")?;
    for (_, task, _) in music_application::assistant::library_cleanup_quality_cases()? {
        let body = handler
            .prepare_structured_request(
                "gpt-5.6-terra",
                10000,
                ThinkingMode::Disabled,
                &task.request(),
            )?
            .payload;
        let schema = &body["text"]["format"]["schema"];
        assert_eq!(body["text"]["format"]["strict"], true);
        assert_eq!(schema["additionalProperties"], false);
        let required = schema["required"].as_array().ok_or("missing required")?;
        for field in schema["properties"]
            .as_object()
            .ok_or("missing properties")?
            .keys()
        {
            assert!(
                required.contains(&json!(field)),
                "OpenAI rejects optional property {field}"
            );
        }
        assert!(
            schema["properties"]["candidate_id"]["type"]
                .as_array()
                .ok_or("missing type")?
                .contains(&json!("null"))
        );
        assert!(
            schema["properties"]["candidate_id"]["enum"]
                .as_array()
                .ok_or("missing enum")?
                .contains(&Value::Null)
        );
        assert_eq!(body["reasoning"]["effort"], "none");
    }
    Ok(())
}
