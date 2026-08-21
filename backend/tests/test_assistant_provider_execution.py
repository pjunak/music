from __future__ import annotations

import pytest

from app.assistant.providers import execution
from app.assistant.providers.execution import (
    CONFORMANCE_CONTRACT,
    ProviderExecutionTarget,
    StructuredModelRequest,
)
from app.assistant.providers.transport import JsonHttpResponse, ProviderTransportError

from .assistant_test_values import TEST_SHORT_API_KEY


def _target() -> ProviderExecutionTarget:
    return ProviderExecutionTarget(
        adapter_id="openai-compatible/v1",
        base_url="https://models.example/v1",
        api_key=TEST_SHORT_API_KEY,
        allow_private_network=False,
        model_id="planner-large",
        timeout_seconds=30,
        max_output_tokens=2000,
    )


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
    }


@pytest.mark.parametrize(
    "content,error_code",
    [
        ("```json\n{}\n```", "invalid_structured_output"),
        ("[]", "invalid_structured_output"),
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
            {"choices": [{"message": {"content": content}}]},
        ),
    )

    result = execution.execute_structured_model_request(
        _target(),
        StructuredModelRequest("system", "user", 512),
    )

    assert result.succeeded is False
    assert result.error_code == error_code


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
        return JsonHttpResponse(
            200,
            {
                "choices": [
                    {
                        "message": {
                            "content": (
                                '{"contract":"'
                                + CONFORMANCE_CONTRACT
                                + '","challenge":"challenge-123","accepted":true}'
                            )
                        }
                    }
                ]
            },
        )

    monkeypatch.setattr(execution, "request_json", request)

    passed = execution.run_provider_conformance(_target(), "challenge-123")
    mismatched = execution.run_provider_conformance(_target(), "different")

    assert passed.passed is True
    assert mismatched.error_code == "conformance_mismatch"
