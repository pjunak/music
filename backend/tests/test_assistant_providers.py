from __future__ import annotations

import base64
from collections.abc import Iterator

import pytest
from fastapi.testclient import TestClient
from pydantic import SecretStr
from sqlalchemy import delete

from app.assistant.providers.schemas import ProviderConnectionUpdate
from app.assistant.providers.service import (
    ProviderServiceError,
    finish_verification,
    prepare_verification,
    update_connection,
)
from app.assistant.providers.verification import ProviderVerificationResult
from app.core.config import get_settings
from app.core.db import SessionLocal
from app.models.assistant_model_role import AssistantModelRole
from app.models.assistant_provider_connection import AssistantProviderConnection


@pytest.fixture(autouse=True)
def _clean_provider_configuration() -> Iterator[None]:
    with SessionLocal() as db:
        db.execute(delete(AssistantModelRole))
        db.execute(delete(AssistantProviderConnection))
        db.commit()
    yield
    with SessionLocal() as db:
        db.execute(delete(AssistantModelRole))
        db.execute(delete(AssistantProviderConnection))
        db.commit()


def _connection_payload(**overrides: object) -> dict[str, object]:
    payload: dict[str, object] = {
        "name": "Primary models",
        "adapter_id": "openai-compatible/v1",
        "base_url": "https://models.example.test/v1",
        "api_key": "secret-provider-key-1234",
        "allow_private_network": False,
    }
    payload.update(overrides)
    return payload


def _create_connection(client: TestClient, **overrides: object) -> dict[str, object]:
    response = client.post(
        "/api/assistant/providers/connections",
        json=_connection_payload(**overrides),
    )
    assert response.status_code == 201, response.text
    return response.json()


def _verify_success(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        "app.api.assistant_providers.verify_provider_connection",
        lambda *args, **kwargs: ProviderVerificationResult(
            True,
            None,
            ("planner-large", "tagger-small"),
        ),
    )


def test_provider_endpoints_require_authentication(client: TestClient) -> None:
    requests = [
        client.get("/api/assistant/providers/status"),
        client.get("/api/assistant/providers/connections"),
        client.post(
            "/api/assistant/providers/connections",
            json=_connection_payload(),
        ),
        client.get("/api/assistant/providers/roles"),
    ]
    assert {response.status_code for response in requests} == {401}


def test_status_lists_supported_adapters_and_roles(auth_client: TestClient) -> None:
    response = auth_client.get("/api/assistant/providers/status")

    assert response.status_code == 200
    payload = response.json()
    assert payload["credential_storage_ready"] is True
    assert [adapter["id"] for adapter in payload["adapters"]] == [
        "openai-compatible/v1"
    ]
    assert {role["id"] for role in payload["roles"]} == {
        "music_tagger",
        "playlist_planner",
        "tag_cleanup",
        "library_cleanup",
        "eq_assistant",
        "audio_analyzer",
    }


def test_connection_secret_is_encrypted_and_never_returned(
    auth_client: TestClient,
) -> None:
    secret = "secret-provider-key-1234"
    created = _create_connection(auth_client, api_key=secret)

    assert created["key_hint"] == "••••1234"
    assert secret not in str(created)
    assert "encrypted_api_key" not in created
    listed = auth_client.get("/api/assistant/providers/connections")
    assert listed.status_code == 200
    assert secret not in listed.text

    with SessionLocal() as db:
        row = db.get(AssistantProviderConnection, created["id"])
        assert row is not None
        assert secret not in row.encrypted_api_key
        assert row.api_key_nonce


def test_connection_storage_fails_closed_without_master_key(
    auth_client: TestClient,
) -> None:
    settings = get_settings()
    previous = settings.assistant_credential_key
    settings.assistant_credential_key = None
    try:
        status = auth_client.get("/api/assistant/providers/status")
        response = auth_client.post(
            "/api/assistant/providers/connections",
            json=_connection_payload(),
        )
    finally:
        settings.assistant_credential_key = previous

    assert status.json()["credential_storage_ready"] is False
    assert status.json()["credential_storage_error"] == "master_key_not_configured"
    assert response.status_code == 503
    assert response.json()["detail"]["code"] == "master_key_not_configured"


@pytest.mark.parametrize(
    "base_url,allow_private",
    [
        ("http://models.example.test/v1", False),
        ("https://user:pass@models.example.test/v1", False),
        ("https://models.example.test/v1?token=oops", False),
        ("file:///tmp/models", True),
    ],
)
def test_connection_rejects_unsafe_provider_urls(
    auth_client: TestClient,
    base_url: str,
    allow_private: bool,
) -> None:
    response = auth_client.post(
        "/api/assistant/providers/connections",
        json=_connection_payload(
            base_url=base_url,
            allow_private_network=allow_private,
        ),
    )

    assert response.status_code == 422
    assert response.json()["detail"]["code"] == "invalid_provider_url"


def test_connection_names_are_unique_without_case_sensitivity(
    auth_client: TestClient,
) -> None:
    _create_connection(auth_client, name="Home Models")
    response = auth_client.post(
        "/api/assistant/providers/connections",
        json=_connection_payload(name="home models"),
    )

    assert response.status_code == 409
    assert response.json()["detail"]["code"] == "duplicate_connection_name"


def test_verification_records_models_without_returning_key(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    created = _create_connection(auth_client)
    observed: dict[str, object] = {}

    def verify(*args: object, **kwargs: object) -> ProviderVerificationResult:
        observed["args"] = args
        observed["kwargs"] = kwargs
        return ProviderVerificationResult(
            True,
            None,
            ("planner-large", "tagger-small"),
        )

    monkeypatch.setattr(
        "app.api.assistant_providers.verify_provider_connection",
        verify,
    )
    response = auth_client.post(
        f"/api/assistant/providers/connections/{created['id']}/verify"
    )

    assert response.status_code == 200, response.text
    assert response.json()["verified"] is True
    assert response.json()["models"] == ["planner-large", "tagger-small"]
    assert response.json()["connection"]["verification_status"] == "verified"
    assert observed["args"] == (
        "openai-compatible/v1",
        "https://models.example.test/v1",
        "secret-provider-key-1234",
    )
    assert "secret-provider-key-1234" not in response.text


def test_changing_connection_inputs_resets_verification(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    created = _create_connection(auth_client)
    _verify_success(monkeypatch)
    verified = auth_client.post(
        f"/api/assistant/providers/connections/{created['id']}/verify"
    )
    assert verified.json()["verified"] is True

    response = auth_client.put(
        f"/api/assistant/providers/connections/{created['id']}",
        json={"base_url": "https://other.example.test/api"},
    )

    assert response.status_code == 200
    assert response.json()["verification_status"] == "never"
    assert response.json()["verified_models"] == []
    assert response.json()["last_verified_at"] is None


def test_verification_result_is_rejected_if_connection_changed_mid_request(
    auth_client: TestClient,
) -> None:
    created = _create_connection(auth_client)
    with SessionLocal() as db:
        target = prepare_verification(db, str(created["id"]))
    with SessionLocal() as db:
        update_connection(
            db,
            str(created["id"]),
            ProviderConnectionUpdate(base_url="https://other.example.test/v1"),
        )

    with SessionLocal() as db, pytest.raises(ProviderServiceError) as error:
        finish_verification(
            db,
            target,
            ProviderVerificationResult(True, None, ("stale-model",)),
        )

    assert error.value.code == "connection_changed"


def test_role_cannot_be_enabled_until_connection_is_verified(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    created = _create_connection(auth_client)
    role_payload = {
        "connection_id": created["id"],
        "model_id": "planner-large",
        "enabled": True,
        "timeout_seconds": 30,
        "max_output_tokens": 2000,
    }

    rejected = auth_client.put(
        "/api/assistant/providers/roles/playlist_planner",
        json=role_payload,
    )
    assert rejected.status_code == 409
    assert rejected.json()["detail"]["code"] == "connection_not_verified"

    _verify_success(monkeypatch)
    auth_client.post(
        f"/api/assistant/providers/connections/{created['id']}/verify"
    )
    saved = auth_client.put(
        "/api/assistant/providers/roles/playlist_planner",
        json=role_payload,
    )
    assert saved.status_code == 200
    assert saved.json()["enabled"] is True
    assert saved.json()["effective_enabled"] is True


def test_failed_reverification_disables_role_effectively_but_keeps_draft(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    created = _create_connection(auth_client)
    _verify_success(monkeypatch)
    auth_client.post(
        f"/api/assistant/providers/connections/{created['id']}/verify"
    )
    auth_client.put(
        "/api/assistant/providers/roles/playlist_planner",
        json={
            "connection_id": created["id"],
            "model_id": "planner-large",
            "enabled": True,
        },
    )
    monkeypatch.setattr(
        "app.api.assistant_providers.verify_provider_connection",
        lambda *args, **kwargs: ProviderVerificationResult(False, "unauthorized"),
    )

    failed = auth_client.post(
        f"/api/assistant/providers/connections/{created['id']}/verify"
    )
    roles = auth_client.get("/api/assistant/providers/roles")
    planner = next(
        role for role in roles.json() if role["role_id"] == "playlist_planner"
    )

    assert failed.json()["verified"] is False
    assert failed.json()["error_code"] == "unauthorized"
    assert planner["enabled"] is True
    assert planner["effective_enabled"] is False
    assert planner["verification_status"] == "failed"


def test_connection_cannot_be_deleted_while_assigned_to_role(
    auth_client: TestClient,
) -> None:
    created = _create_connection(auth_client)
    saved = auth_client.put(
        "/api/assistant/providers/roles/music_tagger",
        json={
            "connection_id": created["id"],
            "model_id": "tagger-small",
            "enabled": False,
        },
    )
    assert saved.status_code == 200

    blocked = auth_client.delete(
        f"/api/assistant/providers/connections/{created['id']}"
    )
    assert blocked.status_code == 409
    assert blocked.json()["detail"]["code"] == "connection_in_use"

    assert (
        auth_client.delete("/api/assistant/providers/roles/music_tagger").status_code
        == 204
    )
    assert (
        auth_client.delete(
            f"/api/assistant/providers/connections/{created['id']}"
        ).status_code
        == 204
    )


def test_wrong_master_key_cannot_decrypt_stored_credential(
    auth_client: TestClient,
) -> None:
    created = _create_connection(auth_client)
    settings = get_settings()
    previous = settings.assistant_credential_key
    settings.assistant_credential_key = SecretStr(
        base64.urlsafe_b64encode(b"W" * 32).decode()
    )
    try:
        response = auth_client.post(
            f"/api/assistant/providers/connections/{created['id']}/verify"
        )
    finally:
        settings.assistant_credential_key = previous

    assert response.status_code == 503
    assert response.json()["detail"]["code"] == "credential_unreadable"


def test_enabled_role_fails_closed_when_master_key_becomes_unavailable(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    created = _create_connection(auth_client)
    _verify_success(monkeypatch)
    auth_client.post(
        f"/api/assistant/providers/connections/{created['id']}/verify"
    )
    role_payload = {
        "connection_id": created["id"],
        "model_id": "planner-large",
        "enabled": True,
    }
    assert (
        auth_client.put(
            "/api/assistant/providers/roles/playlist_planner",
            json=role_payload,
        ).status_code
        == 200
    )

    settings = get_settings()
    previous = settings.assistant_credential_key
    settings.assistant_credential_key = None
    try:
        roles = auth_client.get("/api/assistant/providers/roles")
        rejected = auth_client.put(
            "/api/assistant/providers/roles/playlist_planner",
            json=role_payload,
        )
    finally:
        settings.assistant_credential_key = previous

    planner = next(
        item for item in roles.json() if item["role_id"] == "playlist_planner"
    )
    assert planner["enabled"] is True
    assert planner["effective_enabled"] is False
    assert rejected.status_code == 503
    assert rejected.json()["detail"]["code"] == "master_key_not_configured"
