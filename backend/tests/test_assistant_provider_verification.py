from __future__ import annotations

import json

import pytest

from app.assistant.providers import verification


class _Response:
    def __init__(self, payload: object, *, content_length: int | None = None) -> None:
        self._body = json.dumps(payload).encode()
        self.status = 200
        self.content_length = content_length

    def __enter__(self) -> _Response:
        return self

    def __exit__(self, *args: object) -> None:
        return None

    def read(self, size: int) -> bytes:
        return self._body[:size]

    def getheader(self, name: str) -> str | None:
        if name == "Content-Length" and self.content_length is not None:
            return str(self.content_length)
        return None


def test_public_verification_blocks_non_global_destination(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        verification.socket,
        "getaddrinfo",
        lambda *args, **kwargs: [(2, 1, 6, "", ("127.0.0.1", 443))],
    )
    called = False

    def should_not_request(*args: object, **kwargs: object) -> tuple[int, object]:
        nonlocal called
        called = True
        return 200, {"data": []}

    monkeypatch.setattr(verification, "_http_get_json", should_not_request)

    result = verification.verify_provider_connection(
        "openai-compatible/v1",
        "https://localhost/v1",
        "key",
        allow_private_network=False,
    )

    assert result.error_code == "destination_blocked"
    assert called is False


def test_verification_accepts_unique_bounded_model_ids(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        verification,
        "_destination_addresses",
        lambda *a, **k: ("203.0.113.15",),
    )
    monkeypatch.setattr(
        verification,
        "_http_get_json",
        lambda *a, **k: (
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


def test_verification_does_not_follow_redirects(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        verification,
        "_destination_addresses",
        lambda *a, **k: ("203.0.113.15",),
    )
    monkeypatch.setattr(
        verification,
        "_http_get_json",
        lambda *a, **k: (302, {"data": []}),
    )

    result = verification.verify_provider_connection(
        "openai-compatible/v1",
        "https://models.example/v1",
        "key",
        allow_private_network=False,
    )

    assert result.error_code == "redirect_blocked"


def test_http_reader_rejects_declared_oversized_response() -> None:
    with pytest.raises(OverflowError):
        verification._read_json_response(
            _Response(
                {"data": []},
                content_length=verification._MAX_VERIFICATION_BYTES + 1,
            )
        )


def test_verification_pins_request_to_the_approved_dns_result(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        verification,
        "_destination_addresses",
        lambda *args, **kwargs: ("203.0.113.15",),
    )
    observed: dict[str, object] = {}

    def request(
        url: str,
        api_key: str,
        addresses: tuple[str, ...],
    ) -> tuple[int, object]:
        observed.update(url=url, api_key=api_key, addresses=addresses)
        return 200, {"data": []}

    monkeypatch.setattr(verification, "_http_get_json", request)

    result = verification.verify_provider_connection(
        "openai-compatible/v1",
        "https://models.example/v1",
        "key",
        allow_private_network=False,
    )

    assert result.verified is True
    assert observed["addresses"] == ("203.0.113.15",)
