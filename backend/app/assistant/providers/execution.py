"""Provider-neutral structured model execution contracts.

Feature code must supply a fixed, reviewed prompt and validate the returned
payload against its own schema. This layer only normalizes provider transport,
JSON-object extraction, bounded usage metadata, and safe error codes.
"""

import json
from dataclasses import dataclass
from typing import Literal

from app.assistant.providers.handlers import get_provider_adapter_handler
from app.assistant.providers.transport import (
    ProviderTransportError,
    request_json,
    safe_http_error_code,
    safe_provider_error_code,
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


def _parse_structured_text(
    content: str,
    *,
    provider_model_id: str | None,
    finish_reason: str | None,
    input_tokens: int | None,
    output_tokens: int | None,
) -> StructuredModelResult:
    if not content.strip():
        return StructuredModelResult(
            False,
            "empty_structured_output",
            provider_model_id=provider_model_id,
            finish_reason=finish_reason,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
        )
    try:
        structured = json.loads(content)
    except json.JSONDecodeError:
        return StructuredModelResult(
            False,
            (
                "incomplete_structured_output"
                if finish_reason in {"length", "max_tokens"}
                else "invalid_structured_output"
            ),
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
    return _parse_structured_text(
        message["content"],
        provider_model_id=provider_model_id,
        finish_reason=finish_reason,
        input_tokens=input_tokens,
        output_tokens=output_tokens,
    )


def _parse_openai_responses_response(payload: object) -> StructuredModelResult:
    if not isinstance(payload, dict):
        return StructuredModelResult(False, "invalid_response")
    provider_model_id = _optional_bounded_text(payload.get("model"), max_length=256)
    usage = payload.get("usage")
    usage_dict = usage if isinstance(usage, dict) else {}
    input_tokens = _optional_token_count(usage_dict.get("input_tokens"))
    output_tokens = _optional_token_count(usage_dict.get("output_tokens"))
    status = payload.get("status")
    if status == "failed":
        return StructuredModelResult(
            False,
            safe_provider_error_code(payload, fallback="upstream_error"),
            provider_model_id=provider_model_id,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
        )
    if status == "incomplete":
        incomplete_details = payload.get("incomplete_details")
        finish_reason = (
            _optional_bounded_text(incomplete_details.get("reason"), max_length=64)
            if isinstance(incomplete_details, dict)
            else None
        )
        return StructuredModelResult(
            False,
            "incomplete_structured_output",
            provider_model_id=provider_model_id,
            finish_reason=finish_reason,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
        )
    if status != "completed":
        return StructuredModelResult(
            False,
            "invalid_response",
            provider_model_id=provider_model_id,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
        )

    output = payload.get("output")
    if not isinstance(output, list):
        return StructuredModelResult(
            False,
            "invalid_response",
            provider_model_id=provider_model_id,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
        )
    text_parts: list[str] = []
    refused = False
    for item in output:
        if not isinstance(item, dict) or item.get("type") != "message":
            continue
        content = item.get("content")
        if not isinstance(content, list):
            continue
        for part in content:
            if not isinstance(part, dict):
                continue
            if part.get("type") == "output_text" and isinstance(part.get("text"), str):
                text_parts.append(part["text"])
            elif part.get("type") == "refusal":
                refused = True
    if not text_parts:
        return StructuredModelResult(
            False,
            "model_refusal" if refused else "empty_structured_output",
            provider_model_id=provider_model_id,
            finish_reason="stop",
            input_tokens=input_tokens,
            output_tokens=output_tokens,
        )
    return _parse_structured_text(
        "".join(text_parts),
        provider_model_id=provider_model_id,
        finish_reason="stop",
        input_tokens=input_tokens,
        output_tokens=output_tokens,
    )


def _execute_provider_handler(
    target: ProviderExecutionTarget,
    request: StructuredModelRequest,
) -> StructuredModelResult:
    handler = get_provider_adapter_handler(target.adapter_id)
    if handler is None:
        return StructuredModelResult(False, "unsupported_adapter")
    response_format: dict[str, object] = {"type": "json_object"}
    if handler.structured_output_mode == "json_schema":
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
    maximum_output_tokens = min(request.max_output_tokens, target.max_output_tokens)
    if handler.execution_api_style == "responses":
        assert request.output_schema_name is not None
        assert request.output_schema is not None
        payload: dict[str, object] = {
            "model": handler.normalize_model_id(target.model_id),
            "instructions": request.system_prompt,
            "input": request.user_prompt,
            "max_output_tokens": maximum_output_tokens,
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": request.output_schema_name,
                    "strict": True,
                    "schema": request.output_schema,
                }
            },
            "store": False,
        }
    else:
        payload = {
            "model": handler.normalize_model_id(target.model_id),
            "messages": [
                {"role": "system", "content": request.system_prompt},
                {"role": "user", "content": request.user_prompt},
            ],
            "max_tokens": maximum_output_tokens,
            "response_format": response_format,
        }
    handler.apply_thinking_mode(payload, target.thinking_mode)
    try:
        response = request_json(
            "POST",
            handler.completion_url(target.base_url),
            target.api_key,
            allow_private_network=target.allow_private_network,
            timeout_seconds=target.timeout_seconds,
            max_response_bytes=_MAX_EXECUTION_RESPONSE_BYTES,
            user_agent="music-assistant-model-executor/1",
            payload=payload,
            additional_headers=dict(handler.additional_headers),
        )
    except ProviderTransportError as exc:
        return StructuredModelResult(False, exc.code)

    if not 200 <= response.status_code < 300:
        return StructuredModelResult(
            False,
            safe_http_error_code(
                response.status_code,
                not_found_code="completion_endpoint_not_found",
                payload=response.payload,
            ),
        )
    if handler.execution_api_style == "responses":
        return _parse_openai_responses_response(response.payload)
    return _parse_openai_compatible_response(response.payload)


def execute_structured_model_request(
    target: ProviderExecutionTarget,
    request: StructuredModelRequest,
) -> StructuredModelResult:
    """Execute one request; feature-specific schema checks happen above this layer."""

    return _execute_provider_handler(target, request)


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
