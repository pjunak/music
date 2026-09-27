use super::*;
use music_application::assistant::{
    ModelTaskError, TYPESAFE_ADAPTER, TypedDecisionRequest, TypedDecisionTransport,
};

pub(crate) fn validate_typesafe_request(
    target: &ProviderExecutionTarget,
    request: &TypedDecisionRequest,
) -> Result<(), ModelTaskError> {
    if target.adapter_id != TYPESAFE_ADAPTER {
        return Err(ModelTaskError::new("unsupported_adapter"));
    }
    music_application::assistant::validate_model_settings(
        &target.adapter_id,
        &target.model_id,
        target.thinking_mode,
        target.max_output_tokens,
    )
    .map_err(ModelTaskError::new)?;
    request.validate()
}

impl TypedDecisionTransport for ProviderNetworkBoundary {
    fn validate_typed_request(
        &self,
        target: &ProviderExecutionTarget,
        request: &TypedDecisionRequest,
    ) -> Result<(), ModelTaskError> {
        validate_typesafe_request(target, request)
    }
    fn execute_typed_request<'a>(
        &'a self,
        target: &'a ProviderExecutionTarget,
        request: &'a TypedDecisionRequest,
    ) -> music_application::assistant::ModelTransportFuture<'a> {
        Box::pin(async move {
            if let Err(error) = validate_typesafe_request(target, request) {
                return failed_structured_model(
                    &error.code,
                    ProviderAttemptOutcome::PreflightRejected,
                );
            }
            let Ok(_permit) = self.request_slots.try_acquire() else {
                return failed_structured_model("provider_busy", ProviderAttemptOutcome::NotSent);
            };
            let response = self.post_json(&format!("{}/systemone",target.base_url.trim_end_matches('/')),
                target.api_key.expose_secret(),target.allow_private_network,Duration::from_secs(u64::from(target.timeout_seconds)),
                MAX_EXECUTION_RESPONSE_BYTES,EXECUTOR_USER_AGENT,&[],
                &serde_json::json!({"model":target.model_id,"state":request.state,"questions":request.questions})).await;
            let response = match response {
                Ok(response) => response,
                Err(error) => return failed_structured_model(error.code(), error.outcome),
            };
            if !response.status.is_success() {
                return failed_structured_model(
                    safe_http_error_code(
                        response.status,
                        "completion_endpoint_not_found",
                        &response.payload,
                    ),
                    ProviderAttemptOutcome::ResponseReceived,
                );
            }
            match music_application::assistant::parse_typesafe_response(
                &target.model_id,
                request,
                response.payload.clone(),
            ) {
                Ok(result) => result,
                Err(error) => {
                    // A malformed answer can still have consumed measured usage.
                    let mut result = failed_structured_model(
                        &error.code,
                        ProviderAttemptOutcome::ResponseReceived,
                    );
                    result.input_tokens = response
                        .payload
                        .pointer("/usage/input_tokens")
                        .and_then(Value::as_u64);
                    result.output_tokens = response
                        .payload
                        .pointer("/usage/output_tokens")
                        .and_then(Value::as_u64);
                    result.provider_model_id = response
                        .payload
                        .get("model")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty() && id.len() <= 256)
                        .map(str::to_owned);
                    result
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        routing::{get, post},
    };
    use music_application::assistant::{
        ProviderSecret, ThinkingMode, typed_conformance_request, validate_typed_conformance,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;
    type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
    fn target(address: std::net::SocketAddr) -> ProviderExecutionTarget {
        ProviderExecutionTarget {
            adapter_id: TYPESAFE_ADAPTER.to_owned(),
            base_url: format!("http://{address}/v1"),
            api_key: ProviderSecret::new("fixture-only"),
            allow_private_network: true,
            model_id: "jev-1.13.0".to_owned(),
            thinking_mode: ThinkingMode::ProviderDefault,
            max_output_tokens: 4000,
            timeout_seconds: 1,
        }
    }
    async fn server(
        app: Router,
    ) -> Result<
        (std::net::SocketAddr, tokio::task::JoinHandle<()>),
        Box<dyn std::error::Error + Send + Sync>,
    > {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        Ok((
            address,
            tokio::spawn(async move {
                let _result = axum::serve(listener, app).await;
            }),
        ))
    }
    #[tokio::test]
    async fn jev_discovers_native_names_and_posts_only_the_documented_primitive_body() -> TestResult
    {
        let captured = Arc::new(Mutex::new(None));
        let capture = captured.clone();
        let app=Router::new()
            .route("/v1/models",get(||async{Json(serde_json::json!({"models":[{"name":"jev-latest"},{"name":"jev-preview"}]}))}))
            .route("/v1/systemone",post(move |headers:HeaderMap,Json(body):Json<Value>| { let capture=capture.clone(); async move {
                *capture.lock().await=Some((headers,body));
                Json(serde_json::json!({"model":"jev-1.13.0","answers":{
                    "yes_nonce":{"type":"noul","noul":0.99},"no_nonce":{"type":"noul","noul":0.01},
                    "choice_nonce":{"type":"choice","choice":"solo_singing","probabilities":{"solo_singing":0.98,"instrumental_music":0.01,"silence":0.01},"confidence":0.9}},
                    "usage":{"input_tokens":222,"output_tokens":25}}))
            }}));
        let (address, server) = server(app).await?;
        let target = target(address);
        let boundary = ProviderNetworkBoundary::new();
        let verification = boundary
            .verify_provider_connection(&ProviderVerificationTarget {
                connection_id: "fixture".to_owned(),
                adapter_id: target.adapter_id.clone(),
                base_url: target.base_url.clone(),
                api_key: ProviderSecret::new("fixture-only"),
                allow_private_network: true,
                fingerprint: "fixture".to_owned(),
            })
            .await;
        assert!(verification.verified);
        assert_eq!(verification.models, vec!["jev-latest", "jev-preview"]);
        let request = typed_conformance_request("nonce");
        let result = boundary.execute_typed_request(&target, &request).await;
        server.abort();
        validate_typed_conformance("nonce", &result)?;
        assert_eq!(result.input_tokens, Some(222));
        let (headers, body) = captured.lock().await.take().ok_or("no request")?;
        assert_eq!(headers[AUTHORIZATION], "Bearer fixture-only");
        assert_eq!(
            body.as_object()
                .ok_or("body")?
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["model", "questions", "state"]
        );
        assert_eq!(body["questions"], serde_json::to_value(&request.questions)?);
        assert!(
            crate::provider_handlers::validate_structured_request(
                &target,
                &request.accounting_request()
            )
            .is_err()
        );
        assert!(
            boundary
                .normalize_base_url(TYPESAFE_ADAPTER, "https://api.typesafe.ai/v1", false)
                .is_ok()
        );
        assert!(
            boundary
                .normalize_base_url(TYPESAFE_ADAPTER, "https://wrong.example/v1", false)
                .is_err()
        );
        Ok(())
    }
    #[tokio::test]
    async fn jev_errors_are_not_retried_and_malformed_paid_answers_keep_usage() -> TestResult {
        for (status, code) in [
            (401, "unauthorized"),
            (422, "invalid_request"),
            (429, "rate_limited"),
            (529, "service_unavailable"),
            (200, "typed_answer_set_mismatch"),
        ] {
            let status_code = StatusCode::from_u16(status)?;
            let calls = Arc::new(AtomicUsize::new(0));
            let count = calls.clone();
            let app=Router::new().route("/v1/systemone",post(move || { let count=count.clone(); async move {
                count.fetch_add(1,Ordering::SeqCst);
                (status_code,Json(serde_json::json!({"model":"jev-1.13.0","answers":{},"usage":{"input_tokens":73,"output_tokens":2}})))
            }}));
            let (address, server) = server(app).await?;
            let result = ProviderNetworkBoundary::new()
                .execute_typed_request(&target(address), &typed_conformance_request("nonce"))
                .await;
            server.abort();
            assert_eq!(result.error_code.as_deref(), Some(code));
            assert_eq!(result.outcome, ProviderAttemptOutcome::ResponseReceived);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            if status == 200 {
                assert_eq!(result.input_tokens, Some(73));
                assert_eq!(result.output_tokens, Some(2));
            }
        }
        Ok(())
    }
    #[tokio::test]
    async fn jev_timeout_after_submission_is_uncertain_and_releases_capacity() -> TestResult {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let app = Router::new().route(
            "/v1/systemone",
            post(move || {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    std::future::pending::<()>().await;
                    Json(Value::Null)
                }
            }),
        );
        let (address, server) = server(app).await?;
        let boundary = ProviderNetworkBoundary::new();
        let result = boundary
            .execute_typed_request(&target(address), &typed_conformance_request("nonce"))
            .await;
        server.abort();
        assert_eq!(result.error_code.as_deref(), Some("timeout"));
        assert_eq!(result.outcome, ProviderAttemptOutcome::Uncertain);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            boundary.request_slots.available_permits(),
            PROVIDER_REQUEST_CONCURRENCY
        );
        let mut target = target(address);
        target.model_id = "jev-latest".to_owned();
        let rejected = boundary
            .execute_typed_request(&target, &typed_conformance_request("nonce"))
            .await;
        assert_eq!(rejected.outcome, ProviderAttemptOutcome::PreflightRejected);
        assert_eq!(
            rejected.error_code.as_deref(),
            Some("pinned_model_required")
        );
        Ok(())
    }
}
