"""Provider-neutral structured model execution contracts.

Feature code must supply a fixed, reviewed prompt and validate the returned
payload against its own schema. This layer only normalizes provider transport,
JSON-object extraction, bounded usage metadata, and safe error codes.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Literal

from app.assistant.providers.definitions import (
    OPENAI_COMPATIBLE_ADAPTER,
    OPENAI_COMPATIBLE_JSON_SCHEMA_ADAPTER,
)
from app.assistant.providers.transport import (
    ProviderTransportError,
    request_json,
    safe_http_error_code,
)

CONFORMANCE_CONTRACT: Literal["assistant-provider-conformance/v3"] = (
    "assistant-provider-conformance/v3"
)
_MAX_EXECUTION_RESPONSE_BYTES = 2 * 1024 * 1024
ThinkingMode = Literal["provider_default", "enabled", "disabled"]


@dataclass(frozen=True)
class ProviderExecutionTarget:
    adapter_id: str
    base_url: str
    api_key: str
    allow_private_network: bool
    model_id: str
    timeout_seconds: int
    max_output_tokens: int
    thinking_mode: ThinkingMode = "provider_default"


@dataclass(frozen=True)
class StructuredModelRequest:
    system_prompt: str
    user_prompt: str
    max_output_tokens: int
    output_schema_name: str | None = None
    output_schema: dict[str, object] | None = None


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
    provider_model_id: str | None = None
    finish_reason: str | None = None
    input_tokens: int | None = None
    output_tokens: int | None = None


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
    provider_model_id = _optional_bounded_text(payload.get("model"), max_length=256)
    usage = payload.get("usage")
    usage_dict = usage if isinstance(usage, dict) else {}
    input_tokens = _optional_token_count(usage_dict.get("prompt_tokens"))
    output_tokens = _optional_token_count(usage_dict.get("completion_tokens"))
    choices = payload.get("choices")
    if not isinstance(choices, list) or not choices or not isinstance(choices[0], dict):
        return StructuredModelResult(
            False,
            "invalid_response",
            provider_model_id=provider_model_id,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
        )
    choice = choices[0]
    finish_reason = _optional_bounded_text(choice.get("finish_reason"), max_length=64)
    message = choice.get("message")
    if not isinstance(message, dict) or not isinstance(message.get("content"), str):
        return StructuredModelResult(
            False,
            "invalid_response",
            provider_model_id=provider_model_id,
            finish_reason=finish_reason,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
        )
    try:
        structured = json.loads(message["content"])
    except json.JSONDecodeError:
        return StructuredModelResult(
            False,
            "invalid_structured_output",
            provider_model_id=provider_model_id,
            finish_reason=finish_reason,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
        )
    if not isinstance(structured, dict):
        return StructuredModelResult(
            False,
            "invalid_structured_output",
            provider_model_id=provider_model_id,
            finish_reason=finish_reason,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
        )
    return StructuredModelResult(
        True,
        None,
        structured,
        provider_model_id=provider_model_id,
        finish_reason=finish_reason,
        input_tokens=input_tokens,
        output_tokens=output_tokens,
    )


def _execute_openai_compatible(
    target: ProviderExecutionTarget,
    request: StructuredModelRequest,
) -> StructuredModelResult:
    response_format: dict[str, object] = {"type": "json_object"}
    if target.adapter_id == OPENAI_COMPATIBLE_JSON_SCHEMA_ADAPTER:
        if request.output_schema_name is None or request.output_schema is None:
            return StructuredModelResult(False, "output_schema_required")
        response_format = {
            "type": "json_schema",
            "json_schema": {
                "name": request.output_schema_name,
                "strict": True,
                "schema": request.output_schema,
            },
        }
    payload: dict[str, object] = {
        "model": target.model_id,
        "messages": [
            {"role": "system", "content": request.system_prompt},
            {"role": "user", "content": request.user_prompt},
        ],
        "max_tokens": min(
            request.max_output_tokens,
            target.max_output_tokens,
        ),
        "response_format": response_format,
    }
    if target.thinking_mode != "provider_default":
        payload["thinking"] = {"type": target.thinking_mode}
    try:
        response = request_json(
            "POST",
            f"{target.base_url.rstrip('/')}/chat/completions",
            target.api_key,
            allow_private_network=target.allow_private_network,
            timeout_seconds=target.timeout_seconds,
            max_response_bytes=_MAX_EXECUTION_RESPONSE_BYTES,
            user_agent="music-assistant-model-executor/1",
            payload=payload,
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

    if target.adapter_id in {
        OPENAI_COMPATIBLE_ADAPTER,
        OPENAI_COMPATIBLE_JSON_SCHEMA_ADAPTER,
    }:
        return _execute_openai_compatible(target, request)
    return StructuredModelResult(False, "unsupported_adapter")


def run_provider_conformance(
    target: ProviderExecutionTarget,
    challenge: str,
) -> ProviderConformanceResult:
    """Check transport plus strict JSON instruction following with synthetic data."""

    conformance_schema: dict[str, object] = {
        "type": "object",
        "additionalProperties": False,
        "required": ["contract", "challenge", "accepted"],
        "properties": {
            "contract": {"type": "string", "const": CONFORMANCE_CONTRACT},
            "challenge": {"type": "string", "const": challenge},
            "accepted": {"type": "boolean", "const": True},
        },
    }
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
        output_schema_name="assistant-provider-conformance",
        output_schema=conformance_schema,
    )
    result = execute_structured_model_request(target, request)
    if not result.succeeded:
        return ProviderConformanceResult(
            False,
            result.error_code,
            provider_model_id=result.provider_model_id,
            finish_reason=result.finish_reason,
            input_tokens=result.input_tokens,
            output_tokens=result.output_tokens,
        )
    if result.payload != {
        "contract": CONFORMANCE_CONTRACT,
        "challenge": challenge,
        "accepted": True,
    }:
        return ProviderConformanceResult(
            False,
            "conformance_mismatch",
            provider_model_id=result.provider_model_id,
            finish_reason=result.finish_reason,
            input_tokens=result.input_tokens,
            output_tokens=result.output_tokens,
        )
    return ProviderConformanceResult(
        True,
        None,
        provider_model_id=result.provider_model_id,
        finish_reason=result.finish_reason,
        input_tokens=result.input_tokens,
        output_tokens=result.output_tokens,
    )
