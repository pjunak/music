from __future__ import annotations

import base64
import json
import time
from collections.abc import Iterator
from typing import Any, cast

import pytest
from fastapi.testclient import TestClient
from pydantic import SecretStr
from sqlalchemy import delete

from app.assistant.model_playlist import MODEL_PLAYLIST_OUTPUT_CONTRACT
from app.assistant.providers.execution import (
    ProviderConformanceResult,
    StructuredModelRequest,
    StructuredModelResult,
)
from app.assistant.providers.schemas import ProviderConnectionUpdate
from app.assistant.providers.service import (
    ProviderServiceError,
    finish_role_conformance,
    finish_verification,
    prepare_role_conformance,
    prepare_role_execution,
    prepare_verification,
    update_connection,
)
from app.assistant.providers.verification import ProviderVerificationResult
from app.core.config import get_settings
from app.core.db import SessionLocal
from app.models.assistant_model_evaluation import AssistantModelEvaluation
from app.models.assistant_model_role import AssistantModelRole
from app.models.assistant_provider_connection import AssistantProviderConnection

PLAYLIST_QUALITY_JOB_KIND = "assistant.model-evaluation.playlist-quality-v1"


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


def _conformance_success(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        "app.api.assistant_providers.run_provider_conformance",
        lambda *args, **kwargs: ProviderConformanceResult(True, None),
    )


def _reference_playlist_model(
    _target: object,
    request: StructuredModelRequest,
) -> StructuredModelResult:
    payload = json.loads(request.user_prompt)
    candidates = payload["candidates"]
    ranked = candidates[: payload["request"]["candidate_limit"]]
    selected: list[dict[str, Any]] = []
    selected_seconds = 0.0
    target_seconds = payload["request"]["target_minutes"] * 60
    for candidate in ranked:
        if selected_seconds >= target_seconds:
            break
        selected.append(candidate)
        selected_seconds += candidate["length_s"] or 180.0
    curve = payload["request"]["energy_curve"]
    if curve == "rising":
        selected.sort(key=lambda item: cast(float, item["planning_energy"]))
    elif curve == "falling":
        selected.sort(key=lambda item: -cast(float, item["planning_energy"]))
    return StructuredModelResult(
        True,
        None,
        {
            "schema_version": MODEL_PLAYLIST_OUTPUT_CONTRACT,
            "ranked_track_ids": [item["track_id"] for item in ranked],
            "selected_track_ids": [item["track_id"] for item in selected],
        },
    )


def _empty_playlist_model(
    _target: object,
    _request: StructuredModelRequest,
) -> StructuredModelResult:
    return StructuredModelResult(
        True,
        None,
        {
            "schema_version": MODEL_PLAYLIST_OUTPUT_CONTRACT,
            "ranked_track_ids": [],
            "selected_track_ids": [],
        },
    )


def _enabled_playlist_role(
    client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> dict[str, object]:
    connection = _create_connection(client)
    _verify_success(monkeypatch)
    verified = client.post(
        f"/api/assistant/providers/connections/{connection['id']}/verify"
    )
    assert verified.status_code == 200
    payload = {
        "connection_id": connection["id"],
        "model_id": "planner-large",
        "enabled": False,
    }
    assert client.put(
        "/api/assistant/providers/roles/playlist_planner",
        json=payload,
    ).status_code == 200
    _conformance_success(monkeypatch)
    assert client.post(
        "/api/assistant/providers/roles/playlist_planner/test"
    ).status_code == 200
    payload["enabled"] = True
    enabled = client.put(
        "/api/assistant/providers/roles/playlist_planner",
        json=payload,
    )
    assert enabled.status_code == 200
    return enabled.json()


def _wait_for_job(
    client: TestClient,
    job_id: str,
    expected: set[str],
) -> dict[str, Any]:
    deadline = time.monotonic() + 5
    latest: dict[str, Any] = {}
    while time.monotonic() < deadline:
        response = client.get(f"/api/jobs/{job_id}")
        assert response.status_code == 200
        latest = response.json()
        if latest["status"] in expected:
            return latest
        time.sleep(0.02)
    raise AssertionError(f"job did not reach {expected}; latest={latest}")


def test_provider_endpoints_require_authentication(client: TestClient) -> None:
    requests = [
        client.get("/api/assistant/providers/status"),
        client.get("/api/assistant/providers/connections"),
        client.post(
            "/api/assistant/providers/connections",
            json=_connection_payload(),
        ),
        client.get("/api/assistant/providers/roles"),
        client.post("/api/assistant/providers/roles/playlist_planner/test"),
        client.get(
            "/api/assistant/providers/roles/playlist_planner/evaluations"
        ),
        client.post(
            "/api/assistant/providers/roles/playlist_planner/"
            "evaluations/playlist-quality-v1/jobs"
        ),
    ]
    assert {response.status_code for response in requests} == {401}


def test_playlist_model_quality_job_persists_progress_and_current_gate(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _enabled_playlist_role(auth_client, monkeypatch)
    monkeypatch.setattr(
        "app.assistant.model_evaluation.execute_structured_model_request",
        _reference_playlist_model,
    )

    started = auth_client.post(
        "/api/assistant/providers/roles/playlist_planner/"
        "evaluations/playlist-quality-v1/jobs"
    )

    assert started.status_code == 202, started.text
    job_id = started.json()["id"]
    finished = _wait_for_job(auth_client, job_id, {"succeeded"})
    assert finished["kind"] == PLAYLIST_QUALITY_JOB_KIND
    assert finished["progress_current"] == 8
    assert finished["progress_total"] == 8
    assert finished["result"]["evaluation"]["passed"] is True
    assert finished["result"]["evaluation"]["summary"]["passed_cases"] == 8
    assert "secret-provider-key-1234" not in json.dumps(finished)
    assert "path" not in json.dumps(finished["parameters"])

    restored = auth_client.get(
        "/api/jobs",
        params={"kind": PLAYLIST_QUALITY_JOB_KIND},
    )
    quality = auth_client.get(
        "/api/assistant/providers/roles/playlist_planner/evaluations"
    )
    assert restored.status_code == 200
    assert restored.json()[0]["id"] == job_id
    assert quality.status_code == 200
    assert quality.json() == [
        {
            "evaluation_id": "playlist-quality-v1",
            "role_id": "playlist_planner",
            "label": "Playlist planning quality",
            "description": (
                "Runs fixed synthetic D&D playlist scenarios through this model. "
                "No songs or live library data are sent."
            ),
            "status": "passed",
            "suite_id": "local-dnd-playlist-baseline-v2",
            "passed_cases": 8,
            "total_cases": 8,
            "last_job_id": job_id,
            "last_evaluated_at": quality.json()[0]["last_evaluated_at"],
        }
    ]
    assert quality.json()[0]["last_evaluated_at"] is not None


def test_playlist_quality_gate_is_invalidated_by_runtime_change(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    role = _enabled_playlist_role(auth_client, monkeypatch)
    monkeypatch.setattr(
        "app.assistant.model_evaluation.execute_structured_model_request",
        _reference_playlist_model,
    )
    started = auth_client.post(
        "/api/assistant/providers/roles/playlist_planner/"
        "evaluations/playlist-quality-v1/jobs"
    )
    _wait_for_job(auth_client, started.json()["id"], {"succeeded"})

    changed = auth_client.put(
        "/api/assistant/providers/roles/playlist_planner",
        json={
            "connection_id": role["connection_id"],
            "model_id": role["model_id"],
            "enabled": False,
            "timeout_seconds": 45,
            "max_output_tokens": role["max_output_tokens"],
        },
    )
    quality = auth_client.get(
        "/api/assistant/providers/roles/playlist_planner/evaluations"
    )

    assert changed.status_code == 200
    assert changed.json()["conformance_status"] == "never"
    assert quality.json()[0]["status"] == "never"
    assert quality.json()[0]["last_job_id"] is None


def test_playlist_quality_status_is_stale_when_suite_version_changes(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _enabled_playlist_role(auth_client, monkeypatch)
    monkeypatch.setattr(
        "app.assistant.model_evaluation.execute_structured_model_request",
        _reference_playlist_model,
    )
    started = auth_client.post(
        "/api/assistant/providers/roles/playlist_planner/"
        "evaluations/playlist-quality-v1/jobs"
    )
    finished = _wait_for_job(auth_client, started.json()["id"], {"succeeded"})
    with SessionLocal() as db:
        row = db.get(
            AssistantModelEvaluation,
            ("playlist_planner", "playlist-quality-v1"),
        )
        assert row is not None
        row.suite_id = "local-dnd-playlist-baseline-v1"
        db.commit()

    quality = auth_client.get(
        "/api/assistant/providers/roles/playlist_planner/evaluations"
    )

    assert quality.status_code == 200
    assert quality.json()[0]["status"] == "stale"
    assert quality.json()[0]["suite_id"] == "local-dnd-playlist-baseline-v2"
    assert quality.json()[0]["last_job_id"] == finished["id"]


def test_failed_playlist_quality_is_a_completed_report_not_a_broken_job(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _enabled_playlist_role(auth_client, monkeypatch)
    monkeypatch.setattr(
        "app.assistant.model_evaluation.execute_structured_model_request",
        _empty_playlist_model,
    )

    started = auth_client.post(
        "/api/assistant/providers/roles/playlist_planner/"
        "evaluations/playlist-quality-v1/jobs"
    )
    finished = _wait_for_job(auth_client, started.json()["id"], {"succeeded"})
    quality = auth_client.get(
        "/api/assistant/providers/roles/playlist_planner/evaluations"
    ).json()[0]

    assert finished["result"]["evaluation"]["passed"] is False
    assert finished["result"]["evaluation"]["summary"]["failed_cases"] == 8
    assert quality["status"] == "failed"
    assert quality["passed_cases"] == 0
    assert quality["total_cases"] == 8


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
    descriptions = {role["id"]: role["description"] for role in payload["roles"]}
    assert "Reserved" not in descriptions["playlist_planner"]
    assert "Reserved" not in descriptions["music_tagger"]
    assert "Reserved" not in descriptions["tag_cleanup"]
    for role_id in (
        "library_cleanup",
        "eq_assistant",
        "audio_analyzer",
    ):
        assert descriptions[role_id].startswith("Reserved for")


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


def test_role_requires_verified_connection_and_model_test_before_enablement(
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
    untested = auth_client.put(
        "/api/assistant/providers/roles/playlist_planner",
        json=role_payload,
    )
    assert untested.status_code == 409
    assert untested.json()["detail"]["code"] == "model_not_tested"

    saved = auth_client.put(
        "/api/assistant/providers/roles/playlist_planner",
        json={**role_payload, "enabled": False},
    )
    assert saved.status_code == 200
    assert saved.json()["conformance_status"] == "never"

    _conformance_success(monkeypatch)
    tested = auth_client.post(
        "/api/assistant/providers/roles/playlist_planner/test"
    )
    assert tested.status_code == 200
    assert tested.json()["passed"] is True
    assert tested.json()["role"]["conformance_status"] == "passed"
    assert tested.json()["role"]["effective_enabled"] is False

    enabled = auth_client.put(
        "/api/assistant/providers/roles/playlist_planner",
        json=role_payload,
    )
    assert enabled.status_code == 200
    assert enabled.json()["enabled"] is True
    assert enabled.json()["effective_enabled"] is True

    with SessionLocal() as db:
        target = prepare_role_execution(db, "playlist_planner")
    assert target.model_id == "planner-large"
    assert target.api_key == "secret-provider-key-1234"


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
            "enabled": False,
        },
    )
    _conformance_success(monkeypatch)
    auth_client.post("/api/assistant/providers/roles/playlist_planner/test")
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
    assert planner["conformance_status"] == "never"


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
        "enabled": False,
    }
    assert (
        auth_client.put(
            "/api/assistant/providers/roles/playlist_planner",
            json=role_payload,
        ).status_code
        == 200
    )
    _conformance_success(monkeypatch)
    assert (
        auth_client.post(
            "/api/assistant/providers/roles/playlist_planner/test"
        ).status_code
        == 200
    )
    role_payload["enabled"] = True
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


def test_failed_model_test_is_persisted_without_enabling_role(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    created = _create_connection(auth_client)
    _verify_success(monkeypatch)
    auth_client.post(
        f"/api/assistant/providers/connections/{created['id']}/verify"
    )
    auth_client.put(
        "/api/assistant/providers/roles/music_tagger",
        json={
            "connection_id": created["id"],
            "model_id": "tagger-small",
            "enabled": False,
        },
    )
    monkeypatch.setattr(
        "app.api.assistant_providers.run_provider_conformance",
        lambda *args, **kwargs: ProviderConformanceResult(
            False,
            "invalid_structured_output",
        ),
    )

    tested = auth_client.post("/api/assistant/providers/roles/music_tagger/test")
    enable = auth_client.put(
        "/api/assistant/providers/roles/music_tagger",
        json={
            "connection_id": created["id"],
            "model_id": "tagger-small",
            "enabled": True,
        },
    )

    assert tested.status_code == 200
    assert tested.json()["passed"] is False
    assert tested.json()["error_code"] == "invalid_structured_output"
    assert tested.json()["role"]["conformance_status"] == "failed"
    assert enable.status_code == 409
    assert enable.json()["detail"]["code"] == "model_not_tested"


def test_changing_model_limits_invalidates_previous_model_test(
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
            "enabled": False,
        },
    )
    _conformance_success(monkeypatch)
    auth_client.post("/api/assistant/providers/roles/playlist_planner/test")

    changed = auth_client.put(
        "/api/assistant/providers/roles/playlist_planner",
        json={
            "connection_id": created["id"],
            "model_id": "planner-large",
            "enabled": False,
            "timeout_seconds": 45,
            "max_output_tokens": 2000,
        },
    )

    assert changed.status_code == 200
    assert changed.json()["conformance_status"] == "never"
    assert changed.json()["last_conformance_at"] is None


def test_model_test_result_is_rejected_if_role_changes_mid_request(
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
            "enabled": False,
        },
    )
    with SessionLocal() as db:
        target = prepare_role_conformance(db, "playlist_planner")
    with SessionLocal() as db:
        row = db.get(AssistantModelRole, "playlist_planner")
        assert row is not None
        row.model_id = "planner-new"
        db.commit()

    with SessionLocal() as db, pytest.raises(ProviderServiceError) as error:
        finish_role_conformance(
            db,
            target,
            ProviderConformanceResult(True, None),
        )

    assert error.value.code == "role_changed"
