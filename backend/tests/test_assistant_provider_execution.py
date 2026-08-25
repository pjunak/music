from __future__ import annotations

import pytest

from app.assistant.providers import execution
from app.assistant.providers.definitions import PROVIDER_ADAPTER_BY_ID
from app.assistant.providers.execution import (
    CONFORMANCE_CONTRACT,
    ProviderExecutionTarget,
    StructuredModelRequest,
)
from app.assistant.providers.handlers import (
    GOOGLE_GEMINI_OPENAI_BASE_URL,
    PROVIDER_ADAPTER_HANDLER_BY_ID,
)
from app.assistant.providers.transport import JsonHttpResponse, ProviderTransportError

from .assistant_test_values import TEST_SHORT_API_KEY


def _target(
    adapter_id: str = "openai-compatible/v1",
    thinking_mode: execution.ThinkingMode = "provider_default",
    *,
    base_url: str = "https://models.example/v1",
    model_id: str = "planner-large",
) -> ProviderExecutionTarget:
    return ProviderExecutionTarget(
        adapter_id=adapter_id,
        base_url=base_url,
        api_key=TEST_SHORT_API_KEY,
        allow_private_network=False,
        model_id=model_id,
        timeout_seconds=30,
        max_output_tokens=2000,
        thinking_mode=thinking_mode,
    )


def test_every_advertised_adapter_has_an_execution_handler() -> None:
    assert PROVIDER_ADAPTER_HANDLER_BY_ID.keys() == PROVIDER_ADAPTER_BY_ID.keys()


def test_execution_normalizes_structured_response_and_usage(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    observed: dict[str, object] = {}

    def request(*args: object, **kwargs: object) -> JsonHttpResponse:
        observed.update(args=args, kwargs=kwargs)
        return JsonHttpResponse(
            200,
            {
                "model": "planner-large-2026",
                "choices": [
                    {
                        "message": {"content": '{"answer":"ok"}'},
                        "finish_reason": "stop",
                    }
                ],
                "usage": {"prompt_tokens": 21, "completion_tokens": 7},
            },
        )

    monkeypatch.setattr(execution, "request_json", request)
    result = execution.execute_structured_model_request(
        _target(),
        StructuredModelRequest("system", "user", 512),
    )

    assert result.succeeded is True
    assert result.payload == {"answer": "ok"}
    assert result.provider_model_id == "planner-large-2026"
    assert result.input_tokens == 21
    assert result.output_tokens == 7
    request_payload = observed["kwargs"]
    assert isinstance(request_payload, dict)
    assert request_payload["payload"] == {
        "model": "planner-large",
        "messages": [
            {"role": "system", "content": "system"},
            {"role": "user", "content": "user"},
        ],
        "max_tokens": 512,
        "response_format": {"type": "json_object"},
    }


@pytest.mark.parametrize("thinking_mode", ["enabled", "disabled"])
def test_execution_sends_explicit_thinking_override(
    monkeypatch: pytest.MonkeyPatch,
    thinking_mode: execution.ThinkingMode,
) -> None:
    observed: dict[str, object] = {}

    def request(*args: object, **kwargs: object) -> JsonHttpResponse:
        observed.update(kwargs)
        return JsonHttpResponse(
            200,
            {"choices": [{"message": {"content": '{"answer":"ok"}'}}]},
        )

    monkeypatch.setattr(execution, "request_json", request)

    result = execution.execute_structured_model_request(
        _target(thinking_mode=thinking_mode),
        StructuredModelRequest("system", "user", 512),
    )

    assert result.succeeded is True
    payload = observed["payload"]
    assert isinstance(payload, dict)
    assert payload["thinking"] == {"type": thinking_mode}


@pytest.mark.parametrize(
    "thinking_mode,reasoning_effort",
    [("enabled", "high"), ("disabled", "none")],
)
def test_gemini_handler_normalizes_model_and_maps_thinking_controls(
    monkeypatch: pytest.MonkeyPatch,
    thinking_mode: execution.ThinkingMode,
    reasoning_effort: str,
) -> None:
    observed: dict[str, object] = {}

    def request(*args: object, **kwargs: object) -> JsonHttpResponse:
        observed.update(args=args, kwargs=kwargs)
        return JsonHttpResponse(
            200,
            {"choices": [{"message": {"content": '{"answer":"ok"}'}}]},
        )

    monkeypatch.setattr(execution, "request_json", request)
    result = execution.execute_structured_model_request(
        _target(
            "google-gemini-openai/v1",
            thinking_mode,
            base_url=GOOGLE_GEMINI_OPENAI_BASE_URL,
            model_id="models/gemini-3.7-flash",
        ),
        StructuredModelRequest("system", "user", 512),
    )

    assert result.succeeded is True
    args = observed["args"]
    assert isinstance(args, tuple)
    assert args[1] == f"{GOOGLE_GEMINI_OPENAI_BASE_URL}/chat/completions"
    kwargs = observed["kwargs"]
    assert isinstance(kwargs, dict)
    payload = kwargs["payload"]
    assert isinstance(payload, dict)
    assert payload["model"] == "gemini-3.7-flash"
    assert payload["reasoning_effort"] == reasoning_effort
    assert "thinking" not in payload


def test_gemini_handler_omits_provider_default_thinking_override(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    observed: dict[str, object] = {}

    def request(*args: object, **kwargs: object) -> JsonHttpResponse:
        observed.update(kwargs)
        return JsonHttpResponse(
            200,
            {"choices": [{"message": {"content": '{"answer":"ok"}'}}]},
        )

    monkeypatch.setattr(execution, "request_json", request)
    result = execution.execute_structured_model_request(
        _target(
            "google-gemini-openai/v1",
            base_url=GOOGLE_GEMINI_OPENAI_BASE_URL,
            model_id="models/gemini-3.7-flash",
        ),
        StructuredModelRequest("system", "user", 512),
    )

    assert result.succeeded is True
    payload = observed["payload"]
    assert isinstance(payload, dict)
    assert "thinking" not in payload
    assert "reasoning_effort" not in payload


@pytest.mark.parametrize(
    "adapter_id,base_url",
    [
        ("openai-compatible-json-schema/v1", "https://models.example/v1"),
        (
            "google-gemini-openai-json-schema/v1",
            GOOGLE_GEMINI_OPENAI_BASE_URL,
        ),
    ],
)
def test_strict_adapter_sends_exact_task_json_schema(
    monkeypatch: pytest.MonkeyPatch,
    adapter_id: str,
    base_url: str,
) -> None:
    observed: dict[str, object] = {}

    def request(*args: object, **kwargs: object) -> JsonHttpResponse:
        observed.update(kwargs)
        return JsonHttpResponse(
            200,
            {"choices": [{"message": {"content": '{"answer":"ok"}'}}]},
        )

    schema: dict[str, object] = {
        "type": "object",
        "additionalProperties": False,
        "required": ["answer"],
        "properties": {"answer": {"type": "string"}},
    }
    monkeypatch.setattr(execution, "request_json", request)

    result = execution.execute_structured_model_request(
        _target(adapter_id, base_url=base_url),
        StructuredModelRequest(
            "system",
            "user",
            512,
            output_schema_name="test-answer",
            output_schema=schema,
        ),
    )

    assert result.succeeded is True
    payload = observed["payload"]
    assert isinstance(payload, dict)
    assert payload["response_format"] == {
        "type": "json_schema",
        "json_schema": {
            "name": "test-answer",
            "strict": True,
            "schema": schema,
        },
    }


def test_strict_adapter_requires_a_schema() -> None:
    result = execution.execute_structured_model_request(
        _target("openai-compatible-json-schema/v1"),
        StructuredModelRequest("system", "user", 512),
    )

    assert result.succeeded is False
    assert result.error_code == "output_schema_required"


def test_execution_maps_provider_validation_failures_separately(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        execution,
        "request_json",
        lambda *a, **k: JsonHttpResponse(400, {"error": {"message": "private"}}),
    )

    result = execution.execute_structured_model_request(
        _target(),
        StructuredModelRequest("system", "user", 512),
    )

    assert result.succeeded is False
    assert result.error_code == "invalid_request"
    assert "private" not in repr(result)


@pytest.mark.parametrize(
    "content,error_code",
    [
        ("```json\n{}\n```", "invalid_structured_output"),
        ("[]", "invalid_structured_output"),
        ("", "empty_structured_output"),
        ("   \n", "empty_structured_output"),
    ],
)
def test_execution_rejects_non_object_or_wrapped_output(
    monkeypatch: pytest.MonkeyPatch,
    content: str,
    error_code: str,
) -> None:
    monkeypatch.setattr(
        execution,
        "request_json",
        lambda *a, **k: JsonHttpResponse(
            200,
            {
                "model": "planner-large-2026",
                "choices": [
                    {
                        "message": {"content": content},
                        "finish_reason": "stop",
                    }
                ],
                "usage": {"prompt_tokens": 17, "completion_tokens": 4},
            },
        ),
    )

    result = execution.execute_structured_model_request(
        _target(),
        StructuredModelRequest("system", "user", 512),
    )

    assert result.succeeded is False
    assert result.error_code == error_code
    assert result.provider_model_id == "planner-large-2026"
    assert result.finish_reason == "stop"
    assert result.input_tokens == 17
    assert result.output_tokens == 4


def test_execution_distinguishes_truncated_json_from_malformed_json(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        execution,
        "request_json",
        lambda *a, **k: JsonHttpResponse(
            200,
            {
                "choices": [
                    {
                        "message": {"content": '{"answer":'},
                        "finish_reason": "length",
                    }
                ]
            },
        ),
    )

    result = execution.execute_structured_model_request(
        _target(),
        StructuredModelRequest("system", "user", 512),
    )

    assert result.succeeded is False
    assert result.error_code == "incomplete_structured_output"
    assert result.finish_reason == "length"


def test_execution_returns_safe_transport_error_without_secret(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def fail(*args: object, **kwargs: object) -> JsonHttpResponse:
        raise ProviderTransportError("timeout")

    monkeypatch.setattr(execution, "request_json", fail)

    result = execution.execute_structured_model_request(
        _target(),
        StructuredModelRequest("system", "user", 512),
    )

    assert result.error_code == "timeout"
    assert TEST_SHORT_API_KEY not in repr(result)


def test_conformance_requires_exact_challenge_response(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def request(*args: object, **kwargs: object) -> JsonHttpResponse:
        payload = kwargs["payload"]
        assert isinstance(payload, dict)
        messages = payload["messages"]
        assert isinstance(messages, list)
        assert payload["response_format"] == {"type": "json_object"}
        return JsonHttpResponse(
            200,
            {
                "model": "planner-large-2026",
                "choices": [
                    {
                        "message": {
                            "content": (
                                '{"contract":"'
                                + CONFORMANCE_CONTRACT
                                + '","challenge":"challenge-123","accepted":true}'
                            )
                        },
                        "finish_reason": "stop",
                    }
                ],
                "usage": {"prompt_tokens": 23, "completion_tokens": 8},
            },
        )

    monkeypatch.setattr(execution, "request_json", request)

    passed = execution.run_provider_conformance(_target(), "challenge-123")
    mismatched = execution.run_provider_conformance(_target(), "different")

    assert passed.passed is True
    assert passed.provider_model_id == "planner-large-2026"
    assert passed.finish_reason == "stop"
    assert passed.input_tokens == 23
    assert passed.output_tokens == 8
    assert mismatched.error_code == "conformance_mismatch"
