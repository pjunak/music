use super::*;
use music_application::assistant::{
    BatchTransportFuture, MAX_MODEL_BATCH_BYTES, MAX_MODEL_BATCH_REQUESTS, ModelBatchTransport,
    ModelTaskError, ProviderBatchResult, ProviderBatchStatus,
};
use serde_json::json;

fn identifier(value: &str) -> Result<&str, ModelTaskError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(ModelTaskError::new("batch_invalid_identifier"));
    }
    Ok(value)
}

fn json_body(bytes: &[u8]) -> Result<Value, ModelTaskError> {
    serde_json::from_slice(bytes).map_err(|_| ModelTaskError::new("batch_invalid_response"))
}

fn field(value: &Value, key: &str) -> Result<String, ModelTaskError> {
    identifier(
        value[key]
            .as_str()
            .ok_or_else(|| ModelTaskError::new("batch_invalid_response"))?,
    )
    .map(str::to_owned)
}

pub(super) fn batch_jsonl(
    target: &ProviderExecutionTarget,
    requests: &[StructuredModelRequest],
) -> Result<Vec<u8>, ModelTaskError> {
    if target.adapter_id != OPENAI_RESPONSES_ADAPTER
        || target.base_url != OPENAI_API_BASE_URL
        || requests.is_empty()
        || requests.len() > MAX_MODEL_BATCH_REQUESTS
    {
        return Err(ModelTaskError::new("batch_unsupported_or_too_large"));
    }
    let handler = provider_handler(OPENAI_RESPONSES_ADAPTER)
        .ok_or_else(|| ModelTaskError::new("unsupported_adapter"))?;
    let mut bytes = Vec::new();
    for (index, request) in requests.iter().enumerate() {
        let prepared = handler
            .prepare_structured_request(
                &target.model_id,
                target.max_output_tokens,
                target.thinking_mode,
                request,
            )
            .map_err(|error| ModelTaskError::new(error.code()))?;
        let line = serde_json::to_vec(&json!({"custom_id":format!("request-{index}"), "method":"POST", "url":"/v1/responses", "body":prepared.payload}))
            .map_err(|_| ModelTaskError::new("invalid_request"))?;
        if bytes.len() + line.len() + 1 > MAX_MODEL_BATCH_BYTES {
            return Err(ModelTaskError::new("batch_too_large"));
        }
        bytes.extend(line);
        bytes.push(b'\n');
    }
    Ok(bytes)
}

impl ProviderNetworkBoundary {
    async fn batch_http(
        &self,
        target: &ProviderExecutionTarget,
        method: reqwest::Method,
        path: &str,
        body: Vec<u8>,
        content_type: &str,
    ) -> Result<Vec<u8>, ModelTaskError> {
        if target.adapter_id != OPENAI_RESPONSES_ADAPTER
            || target.base_url.trim_end_matches('/') != OPENAI_API_BASE_URL
        {
            return Err(ModelTaskError::new("batch_unsupported_adapter"));
        }
        if body.len() > MAX_MODEL_BATCH_BYTES + 2048 {
            return Err(ModelTaskError::new("batch_too_large"));
        }
        let _permit = self
            .request_slots
            .try_acquire()
            .map_err(|_| ModelTaskError::new("provider_busy"))?;
        let timeout = Duration::from_secs(u64::from(target.timeout_seconds));
        tokio::time::timeout(timeout, async {
            let addresses = self
                .destination_addresses("api.openai.com", 443)
                .await
                .map_err(|error| ModelTaskError::new(error.code()))?;
            if addresses.is_empty() || addresses.iter().any(|address| !is_global(address.ip())) {
                return Err(ModelTaskError::new("destination_blocked"));
            }
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
                .resolve_to_addrs("api.openai.com", &addresses)
                .build()
                .map_err(|_| ModelTaskError::new("invalid_request"))?;
            let headers = request_headers(target.api_key.expose_secret(), EXECUTOR_USER_AGENT, &[])
                .map_err(|error| ModelTaskError::new(error.code()))?;
            let response = client
                .request(method, format!("{OPENAI_API_BASE_URL}{path}"))
                .headers(headers)
                .header(CONTENT_TYPE, content_type)
                .body(body)
                .send()
                .await
                .map_err(|_| ModelTaskError::new("batch_network_uncertain"))?;
            let status = response.status();
            if response
                .content_length()
                .is_some_and(|length| length > MAX_MODEL_BATCH_BYTES as u64)
            {
                return Err(ModelTaskError::new("response_too_large"));
            }
            let mut bytes = Vec::new();
            let mut chunks = response.bytes_stream();
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk.map_err(|_| ModelTaskError::new("batch_network_uncertain"))?;
                if bytes.len() + chunk.len() > MAX_MODEL_BATCH_BYTES {
                    return Err(ModelTaskError::new("response_too_large"));
                }
                bytes.extend(chunk);
            }
            if !status.is_success() {
                return Err(ModelTaskError::new(safe_http_error_code(
                    status,
                    "batch_not_found",
                    &json_body(&bytes).unwrap_or(Value::Null),
                )));
            }
            Ok(bytes)
        })
        .await
        .map_err(|_| ModelTaskError::new("batch_network_uncertain"))?
    }
}

impl ModelBatchTransport for ProviderNetworkBoundary {
    fn validate(
        &self,
        target: &ProviderExecutionTarget,
        requests: &[StructuredModelRequest],
    ) -> Result<(), ModelTaskError> {
        batch_jsonl(target, requests).map(|_| ())
    }
    fn upload<'a>(
        &'a self,
        target: &'a ProviderExecutionTarget,
        requests: &'a [StructuredModelRequest],
    ) -> BatchTransportFuture<'a, String> {
        Box::pin(async move {
            let jsonl = batch_jsonl(target, requests)?;
            let boundary = format!("music-{}", uuid::Uuid::new_v4().simple());
            let mut body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"purpose\"\r\n\r\nbatch\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"expires_after[anchor]\"\r\n\r\ncreated_at\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"expires_after[seconds]\"\r\n\r\n604800\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"mood-requests.jsonl\"\r\nContent-Type: application/jsonl\r\n\r\n").into_bytes();
            body.extend(jsonl);
            body.extend(format!("\r\n--{boundary}--\r\n").bytes());
            let bytes = self
                .batch_http(
                    target,
                    reqwest::Method::POST,
                    "/files",
                    body,
                    &format!("multipart/form-data; boundary={boundary}"),
                )
                .await?;
            field(&json_body(&bytes)?, "id")
        })
    }
    fn submit<'a>(
        &'a self,
        target: &'a ProviderExecutionTarget,
        file_id: &'a str,
        run_id: &'a str,
    ) -> BatchTransportFuture<'a, String> {
        Box::pin(async move {
            identifier(file_id)?;
            let body = json!({"input_file_id":file_id,"endpoint":"/v1/responses","completion_window":"24h","metadata":{"music_run_id":run_id}}).to_string().into_bytes();
            let bytes = self
                .batch_http(
                    target,
                    reqwest::Method::POST,
                    "/batches",
                    body,
                    "application/json",
                )
                .await?;
            field(&json_body(&bytes)?, "id")
        })
    }
    fn status<'a>(
        &'a self,
        target: &'a ProviderExecutionTarget,
        batch_id: &'a str,
    ) -> BatchTransportFuture<'a, ProviderBatchStatus> {
        Box::pin(async move {
            let bytes = self
                .batch_http(
                    target,
                    reqwest::Method::GET,
                    &format!("/batches/{}", identifier(batch_id)?),
                    Vec::new(),
                    "application/json",
                )
                .await?;
            let value = json_body(&bytes)?;
            if value["id"] != batch_id || value["endpoint"] != "/v1/responses" {
                return Err(ModelTaskError::new("batch_identity_mismatch"));
            }
            let state = field(&value, "status")?;
            if !matches!(
                state.as_str(),
                "validating"
                    | "in_progress"
                    | "finalizing"
                    | "completed"
                    | "failed"
                    | "expired"
                    | "cancelling"
                    | "cancelled"
            ) {
                return Err(ModelTaskError::new("batch_invalid_response"));
            }
            Ok(ProviderBatchStatus {
                run_id: field(&value["metadata"], "music_run_id")?,
                state,
                input_file_id: field(&value, "input_file_id")?,
                output_file_id: (!value["output_file_id"].is_null())
                    .then(|| field(&value, "output_file_id"))
                    .transpose()?,
                error_file_id: (!value["error_file_id"].is_null())
                    .then(|| field(&value, "error_file_id"))
                    .transpose()?,
            })
        })
    }
    fn results<'a>(
        &'a self,
        target: &'a ProviderExecutionTarget,
        file_id: &'a str,
    ) -> BatchTransportFuture<'a, Vec<ProviderBatchResult>> {
        Box::pin(async move {
            let bytes = self
                .batch_http(
                    target,
                    reqwest::Method::GET,
                    &format!("/files/{}/content", identifier(file_id)?),
                    Vec::new(),
                    "application/json",
                )
                .await?;
            parse_batch_results(&bytes)
        })
    }
    fn cancel<'a>(
        &'a self,
        target: &'a ProviderExecutionTarget,
        batch_id: &'a str,
    ) -> BatchTransportFuture<'a, ()> {
        Box::pin(async move {
            self.batch_http(
                target,
                reqwest::Method::POST,
                &format!("/batches/{}/cancel", identifier(batch_id)?),
                Vec::new(),
                "application/json",
            )
            .await?;
            Ok(())
        })
    }
    fn delete_file<'a>(
        &'a self,
        target: &'a ProviderExecutionTarget,
        file_id: &'a str,
    ) -> BatchTransportFuture<'a, ()> {
        Box::pin(async move {
            match self
                .batch_http(
                    target,
                    reqwest::Method::DELETE,
                    &format!("/files/{}", identifier(file_id)?),
                    Vec::new(),
                    "application/json",
                )
                .await
            {
                Ok(_) => Ok(()),
                Err(error) if error.code == "batch_not_found" => Ok(()),
                Err(error) => Err(error),
            }
        })
    }
}

fn parse_batch_results(bytes: &[u8]) -> Result<Vec<ProviderBatchResult>, ModelTaskError> {
    let handler = provider_handler(OPENAI_RESPONSES_ADAPTER)
        .ok_or_else(|| ModelTaskError::new("unsupported_adapter"))?;
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        if results.len() >= MAX_MODEL_BATCH_REQUESTS || line.len() > MAX_EXECUTION_RESPONSE_BYTES {
            return Err(ModelTaskError::new("response_too_large"));
        }
        let value = json_body(line)?;
        let custom_id = field(&value, "custom_id")?;
        if !seen.insert(custom_id.clone()) {
            return Err(ModelTaskError::new("batch_duplicate_result"));
        }
        let result = if value["response"]["status_code"] == 200 && value["error"].is_null() {
            handler.parse_structured_response(&value["response"]["body"])
        } else {
            failed_structured_model(
                safe_provider_error_code(&value["response"]["body"])
                    .unwrap_or("batch_request_failed"),
                ProviderAttemptOutcome::ResponseReceived,
            )
        };
        results.push(ProviderBatchResult { custom_id, result });
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_results_keep_identity_usage_and_failures_without_trusting_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let success = json!({"custom_id":"request-2","response":{"status_code":200,"body":{
            "model":"fixture","status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"{\"ok\":true}"}]}],
            "usage":{"input_tokens":100,"output_tokens":20,"input_tokens_details":{"cached_tokens":80,"cache_write_tokens":0},"output_tokens_details":{"reasoning_tokens":5}}
        }},"error":null});
        let failure = json!({"custom_id":"request-0","response":null,"error":{"code":"batch_expired","message":"untrusted diagnostic"}});
        let bytes = format!("{success}\n{failure}\n").into_bytes();
        let results = parse_batch_results(&bytes)?;
        assert_eq!(results[0].custom_id, "request-2");
        assert_eq!(results[0].result.input_tokens, Some(100));
        assert_eq!(
            results[0].result.token_details.cached_input_tokens,
            Some(80)
        );
        assert_eq!(
            results[0].result.token_details.reasoning_output_tokens,
            Some(5)
        );
        assert!(!results[1].result.succeeded);
        assert_eq!(
            results[1].result.error_code.as_deref(),
            Some("batch_request_failed")
        );
        assert!(parse_batch_results(format!("{success}\n{success}\n").as_bytes()).is_err());
        assert!(identifier("../other-file").is_err());
        assert!(identifier("file?redirect=other").is_err());
        Ok(())
    }

    #[test]
    fn batch_envelopes_are_bounded_and_use_the_same_structured_requests()
    -> Result<(), Box<dyn std::error::Error>> {
        use music_application::assistant::{
            ModelTaggerBatch, ThinkingMode, default_vocabulary_snapshot,
        };
        let task = ModelTaggerBatch::new(
            vec![
                json!({"track_id":9876,"artist":"Artist","album":"Album","origin":"","genre":"folk","length_s":120.0}),
            ],
            default_vocabulary_snapshot()?,
        )?;
        let target = ProviderExecutionTarget {
            adapter_id: OPENAI_RESPONSES_ADAPTER.to_owned(),
            base_url: OPENAI_API_BASE_URL.to_owned(),
            api_key: music_application::assistant::ProviderSecret::new("fixture"),
            allow_private_network: false,
            model_id: "gpt-5.6-luna".to_owned(),
            timeout_seconds: 30,
            max_output_tokens: 5000,
            thinking_mode: ThinkingMode::Disabled,
        };
        let bytes = batch_jsonl(&target, &[task.request(false)])?;
        let line: Value = serde_json::from_slice(&bytes)?;
        assert_eq!(line["custom_id"], "request-0");
        assert_eq!(line["url"], "/v1/responses");
        assert_eq!(line["body"]["store"], false);
        assert_eq!(line["body"]["input"][0]["role"], "user");
        assert_eq!(
            line["body"]["input"][0]["content"][0]["prompt_cache_breakpoint"]["mode"],
            "explicit"
        );
        assert!(batch_jsonl(&target, &[]).is_err());
        assert!(
            batch_jsonl(
                &target,
                &vec![task.request(false); MAX_MODEL_BATCH_REQUESTS + 1]
            )
            .is_err()
        );
        Ok(())
    }
}
