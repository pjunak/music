"""Provider-neutral structured model execution contracts.

Feature code must supply a fixed, reviewed prompt and validate the returned
payload against its own schema. This layer only normalizes provider transport,
JSON-object extraction, bounded usage metadata, and safe error codes.
"""

from __future__ import annotations

import json
from dataclasses import dataclass

from app.assistant.providers.definitions import OPENAI_COMPATIBLE_ADAPTER
from app.assistant.providers.transport import (
    ProviderTransportError,
    request_json,
    safe_http_error_code,
)

CONFORMANCE_CONTRACT = "assistant-provider-conformance/v2"
_MAX_EXECUTION_RESPONSE_BYTES = 2 * 1024 * 1024


@dataclass(frozen=True)
class ProviderExecutionTarget:
    adapter_id: str
    base_url: str
    api_key: str
    allow_private_network: bool
    model_id: str
    timeout_seconds: int
    max_output_tokens: int


@dataclass(frozen=True)
class StructuredModelRequest:
    system_prompt: str
    user_prompt: str
    max_output_tokens: int


@dataclass(frozen=True)
class StructuredModelResult:
    succeeded: bool
    error_code: str | None
    payload: dict[str, object] | None = None
    provider_model_id: str | None = None
    finish_reason: str | None = None
    input_tokens: int | None = None
    output_tokens: int | None = None


@dataclass(frozen=True)
class ProviderConformanceResult:
    passed: bool
    error_code: str | None


def _optional_bounded_text(value: object, *, max_length: int) -> str | None:
    if isinstance(value, str) and 0 < len(value) <= max_length:
        return value
    return None


def _optional_token_count(value: object) -> int | None:
    if isinstance(value, int) and not isinstance(value, bool) and value >= 0:
        return value
    return None


def _parse_openai_compatible_response(payload: object) -> StructuredModelResult:
    if not isinstance(payload, dict):
        return StructuredModelResult(False, "invalid_response")
    choices = payload.get("choices")
    if not isinstance(choices, list) or not choices or not isinstance(choices[0], dict):
        return StructuredModelResult(False, "invalid_response")
    choice = choices[0]
    message = choice.get("message")
    if not isinstance(message, dict) or not isinstance(message.get("content"), str):
        return StructuredModelResult(False, "invalid_response")
    try:
        structured = json.loads(message["content"])
    except json.JSONDecodeError:
        return StructuredModelResult(False, "invalid_structured_output")
    if not isinstance(structured, dict):
        return StructuredModelResult(False, "invalid_structured_output")

    usage = payload.get("usage")
    usage_dict = usage if isinstance(usage, dict) else {}
    return StructuredModelResult(
        True,
        None,
        structured,
        provider_model_id=_optional_bounded_text(payload.get("model"), max_length=256),
        finish_reason=_optional_bounded_text(choice.get("finish_reason"), max_length=64),
        input_tokens=_optional_token_count(usage_dict.get("prompt_tokens")),
        output_tokens=_optional_token_count(usage_dict.get("completion_tokens")),
    )


def _execute_openai_compatible(
    target: ProviderExecutionTarget,
    request: StructuredModelRequest,
) -> StructuredModelResult:
    try:
        response = request_json(
            "POST",
            f"{target.base_url.rstrip('/')}/chat/completions",
            target.api_key,
            allow_private_network=target.allow_private_network,
            timeout_seconds=target.timeout_seconds,
            max_response_bytes=_MAX_EXECUTION_RESPONSE_BYTES,
            user_agent="music-assistant-model-executor/1",
            payload={
                "model": target.model_id,
                "messages": [
                    {"role": "system", "content": request.system_prompt},
                    {"role": "user", "content": request.user_prompt},
                ],
                "max_tokens": min(
                    request.max_output_tokens,
                    target.max_output_tokens,
                ),
                "response_format": {"type": "json_object"},
            },
        )
    except ProviderTransportError as exc:
        return StructuredModelResult(False, exc.code)

    if not 200 <= response.status_code < 300:
        return StructuredModelResult(
            False,
            safe_http_error_code(
                response.status_code,
                not_found_code="completion_endpoint_not_found",
            ),
        )
    return _parse_openai_compatible_response(response.payload)


def execute_structured_model_request(
    target: ProviderExecutionTarget,
    request: StructuredModelRequest,
) -> StructuredModelResult:
    """Execute one request; feature-specific schema checks happen above this layer."""

    if target.adapter_id == OPENAI_COMPATIBLE_ADAPTER:
        return _execute_openai_compatible(target, request)
    return StructuredModelResult(False, "unsupported_adapter")


def run_provider_conformance(
    target: ProviderExecutionTarget,
    challenge: str,
) -> ProviderConformanceResult:
    """Check transport plus strict JSON instruction following with synthetic data."""

    request = StructuredModelRequest(
        system_prompt=(
            "This is a connection conformance test. Return only one JSON object with "
            "exactly these keys: contract, challenge, accepted. Copy the supplied "
            "contract and challenge exactly and set accepted to true. Do not use a "
            "Markdown code fence or add any other text."
        ),
        user_prompt=json.dumps(
            {"contract": CONFORMANCE_CONTRACT, "challenge": challenge},
            separators=(",", ":"),
        ),
        max_output_tokens=256,
    )
    result = execute_structured_model_request(target, request)
    if not result.succeeded:
        return ProviderConformanceResult(False, result.error_code)
    if result.payload != {
        "contract": CONFORMANCE_CONTRACT,
        "challenge": challenge,
        "accepted": True,
    }:
        return ProviderConformanceResult(False, "conformance_mismatch")
    return ProviderConformanceResult(True, None)
