import json

import pytest

from app.assistant.providers import transport, verification
from app.assistant.providers.handlers import (
    GOOGLE_GEMINI_OPENAI_BASE_URL,
    OPENAI_API_BASE_URL,
)
from app.assistant.providers.transport import JsonHttpResponse, ProviderTransportError


class _Response:
    def __init__(self, payload: object, *, content_length: int | None = None) -> None:
        self._body = json.dumps(payload).encode()
        self.content_length = content_length

    def read(self, size: int) -> bytes:
        return self._body[:size]

    def getheader(self, name: str) -> str | None:
        if name == "Content-Length" and self.content_length is not None:
            return str(self.content_length)
        return None


def test_public_transport_blocks_non_global_destination(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        transport.socket,
        "getaddrinfo",
        lambda *args, **kwargs: [(2, 1, 6, "", ("127.0.0.1", 443))],
    )
    called = False

    def should_not_request(*args: object, **kwargs: object) -> JsonHttpResponse:
        nonlocal called
        called = True
        return JsonHttpResponse(200, {})

    monkeypatch.setattr(transport, "_http_json", should_not_request)

    with pytest.raises(ProviderTransportError) as error:
        transport.request_json(
            "GET",
            "https://localhost/v1/models",
            "key",
            allow_private_network=False,
            timeout_seconds=10,
            max_response_bytes=1024,
            user_agent="test/1",
        )

    assert error.value.code == "destination_blocked"
    assert called is False


def test_verification_accepts_unique_bounded_model_ids(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        verification,
        "request_json",
        lambda *a, **k: JsonHttpResponse(
            200,
            {
                "data": [
                    {"id": "model-b"},
                    {"id": "model-a"},
                    {"id": "model-b"},
                    {"id": ""},
                    {"other": "ignored"},
                ]
            },
        ),
    )

    result = verification.verify_provider_connection(
        "openai-compatible/v1",
        "https://models.example/v1",
        "key",
        allow_private_network=False,
    )

    assert result.verified is True
    assert result.models == ("model-b", "model-a")
    assert result.capability_ids == ("structured-text/v1",)


def test_strict_adapter_verification_advertises_schema_capability(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        verification,
        "request_json",
        lambda *a, **k: JsonHttpResponse(200, {"data": [{"id": "model-a"}]}),
    )

    result = verification.verify_provider_connection(
        "openai-compatible-json-schema/v1",
        "https://models.example/v1",
        "key",
        allow_private_network=False,
    )

    assert result.verified is True
    assert result.capability_ids == (
        "structured-text/v1",
        "strict-json-schema/v1",
    )


def test_openai_verification_uses_the_native_adapter_profile(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    observed: dict[str, object] = {}

    def request(*args: object, **kwargs: object) -> JsonHttpResponse:
        observed.update(args=args, kwargs=kwargs)
        return JsonHttpResponse(200, {"data": [{"id": "gpt-5.6-luna"}]})

    monkeypatch.setattr(verification, "request_json", request)
    result = verification.verify_provider_connection(
        "openai-responses/v1",
        OPENAI_API_BASE_URL,
        "key",
        allow_private_network=False,
    )

    assert result.verified is True
    assert result.models == ("gpt-5.6-luna",)
    assert result.capability_ids == (
        "structured-text/v1",
        "strict-json-schema/v1",
    )
    args = observed["args"]
    assert isinstance(args, tuple)
    assert args[1] == f"{OPENAI_API_BASE_URL}/models"


def test_gemini_verification_normalizes_resource_model_ids(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    observed: dict[str, object] = {}

    def request(*args: object, **kwargs: object) -> JsonHttpResponse:
        observed.update(args=args, kwargs=kwargs)
        return JsonHttpResponse(
            200,
            {
                "data": [
                    {"id": "models/gemini-3.7-flash"},
                    {"id": "gemini-3.7-flash"},
                    {"id": "models/gemini-2.5-flash"},
                ]
            },
        )

    monkeypatch.setattr(verification, "request_json", request)
    result = verification.verify_provider_connection(
        "google-gemini-openai/v1",
        GOOGLE_GEMINI_OPENAI_BASE_URL,
        "key",
        allow_private_network=False,
    )

    assert result.verified is True
    assert result.models == ("gemini-3.7-flash", "gemini-2.5-flash")
    assert result.capability_ids == (
        "structured-text/v1",
        "strict-json-schema/v1",
    )
    args = observed["args"]
    assert isinstance(args, tuple)
    assert args[1] == f"{GOOGLE_GEMINI_OPENAI_BASE_URL}/models"
    kwargs = observed["kwargs"]
    assert isinstance(kwargs, dict)
    assert kwargs["additional_headers"] == {
        "x-goog-api-client": "music-assistant-oai/1.0"
    }


def test_gemini_strict_verification_advertises_schema_capability(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        verification,
        "request_json",
        lambda *a, **k: JsonHttpResponse(
            200,
            {"data": [{"id": "models/gemini-3.7-flash"}]},
        ),
    )

    result = verification.verify_provider_connection(
        "google-gemini-openai-json-schema/v1",
        GOOGLE_GEMINI_OPENAI_BASE_URL,
        "key",
        allow_private_network=False,
    )

    assert result.verified is True
    assert result.models == ("gemini-3.7-flash",)
    assert result.capability_ids == (
        "structured-text/v1",
        "strict-json-schema/v1",
    )


def test_verification_does_not_follow_redirects(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        verification,
        "request_json",
        lambda *a, **k: JsonHttpResponse(302, {"data": []}),
    )

    result = verification.verify_provider_connection(
        "openai-compatible/v1",
        "https://models.example/v1",
        "key",
        allow_private_network=False,
    )

    assert result.error_code == "redirect_blocked"


def test_http_reader_rejects_declared_oversized_response() -> None:
    with pytest.raises(ProviderTransportError) as error:
        transport._read_json_response(
            _Response({"data": []}, content_length=1025),
            max_response_bytes=1024,
        )

    assert error.value.code == "response_too_large"


def test_transport_pins_request_to_the_approved_dns_result(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        transport,
        "_destination_addresses",
        lambda *args, **kwargs: ("203.0.113.15",),
    )
    observed: dict[str, object] = {}

    def request(
        method: str,
        url: str,
        api_key: str,
        addresses: tuple[str, ...],
        **kwargs: object,
    ) -> JsonHttpResponse:
        observed.update(
            method=method,
            url=url,
            api_key=api_key,
            addresses=addresses,
            kwargs=kwargs,
        )
        return JsonHttpResponse(200, {"data": []})

    monkeypatch.setattr(transport, "_http_json", request)

    result = transport.request_json(
        "GET",
        "https://models.example/v1/models",
        "key",
        allow_private_network=False,
        timeout_seconds=10,
        max_response_bytes=1024,
        user_agent="test/1",
    )

    assert result.status_code == 200
    assert observed["addresses"] == ("203.0.113.15",)


def test_transport_rejects_oversized_json_request(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        transport,
        "_destination_addresses",
        lambda *args, **kwargs: ("203.0.113.15",),
    )

    with pytest.raises(ProviderTransportError) as error:
        transport.request_json(
            "POST",
            "https://models.example/v1/chat/completions",
            "key",
            allow_private_network=False,
            timeout_seconds=10,
            max_response_bytes=1024,
            user_agent="test/1",
            payload={"prompt": "x" * transport._MAX_REQUEST_BYTES},
        )

    assert error.value.code == "request_too_large"


@pytest.mark.parametrize("status_code", [400, 422])
def test_transport_maps_provider_validation_statuses(status_code: int) -> None:
    assert (
        transport.safe_http_error_code(
            status_code,
            not_found_code="endpoint_not_found",
        )
        == "invalid_request"
    )


@pytest.mark.parametrize(
    "provider_error,expected",
    [
        ({"code": "unsupported_parameter"}, "parameter_unknown"),
        ({"type": "insufficient_quota"}, "quota_exceeded"),
        ({"status": "UNAVAILABLE"}, "service_unavailable"),
        ({"status": "DEADLINE_EXCEEDED"}, "provider_timeout"),
    ],
)
def test_transport_maps_only_allowlisted_provider_error_details(
    provider_error: dict[str, str],
    expected: str,
) -> None:
    payload = {"error": {**provider_error, "message": "private provider detail"}}

    result = transport.safe_http_error_code(
        400,
        not_found_code="endpoint_not_found",
        payload=payload,
    )

    assert result == expected
    assert "private provider detail" not in result


def test_transport_keeps_authentication_status_authoritative() -> None:
    assert (
        transport.safe_http_error_code(
            401,
            not_found_code="endpoint_not_found",
            payload={"error": {"type": "invalid_request_error"}},
        )
        == "unauthorized"
    )


def test_transport_rejects_unapproved_additional_headers() -> None:
    with pytest.raises(ProviderTransportError) as error:
        transport._http_json(
            "GET",
            "https://models.example/v1/models",
            "key",
            (),
            body=None,
            timeout_seconds=10,
            max_response_bytes=1024,
            user_agent="test/1",
            additional_headers={"X-Unsafe": "value"},
        )

    assert error.value.code == "invalid_request_headers"
