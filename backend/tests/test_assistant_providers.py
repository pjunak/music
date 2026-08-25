from __future__ import annotations

import base64
import json
import os
import stat
import time
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any, cast

import pytest
from fastapi.testclient import TestClient
from pydantic import SecretStr
from sqlalchemy import delete

from app.assistant.model_playlist import MODEL_PLAYLIST_OUTPUT_CONTRACT
from app.assistant.providers.credentials import CredentialVaultError
from app.assistant.providers.definitions import (
    MODEL_ROLE_BY_ID,
    MODEL_ROLE_RUNTIME_CONTRACTS,
)
from app.assistant.providers.execution import (
    ProviderConformanceResult,
    StructuredModelRequest,
    StructuredModelResult,
)
from app.assistant.providers.schemas import ProviderConnectionUpdate
from app.assistant.providers.service import (
    ProviderServiceError,
    current_role_runtime_fingerprint,
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
from app.models.background_job import BackgroundJob

from .assistant_test_values import (
    TEST_EXISTING_API_KEY,
    TEST_PROVIDER_API_KEY,
    TEST_REPLACEMENT_API_KEY,
)
from .conftest import TEST_PASSWORD

PLAYLIST_QUALITY_JOB_KIND = "assistant.model-evaluation.playlist-quality-v1"


@pytest.fixture(autouse=True)
def _clean_provider_configuration() -> Iterator[None]:
    with SessionLocal() as db:
        db.execute(delete(AssistantModelEvaluation))
        db.execute(delete(AssistantModelRole))
        db.execute(delete(AssistantProviderConnection))
        db.execute(
            delete(BackgroundJob).where(BackgroundJob.kind.like("assistant.model%"))
        )
        db.commit()
    yield
    with SessionLocal() as db:
        db.execute(delete(AssistantModelEvaluation))
        db.execute(delete(AssistantModelRole))
        db.execute(delete(AssistantProviderConnection))
        db.execute(
            delete(BackgroundJob).where(BackgroundJob.kind.like("assistant.model%"))
        )
        db.commit()


def _connection_payload(**overrides: object) -> dict[str, object]:
    payload: dict[str, object] = {
        "name": "Primary models",
        "adapter_id": "openai-compatible/v1",
        "base_url": "https://models.example.test/v1",
        "api_key": TEST_PROVIDER_API_KEY,
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


@contextmanager
def _file_backed_credential_storage(tmp_path: Path) -> Iterator[Path]:
    settings = get_settings()
    previous_key = settings.assistant_credential_key
    previous_file = settings.assistant_credential_key_file
    secrets_dir = tmp_path / "assistant-secrets"
    secrets_dir.mkdir(mode=0o700)
    if os.name == "posix":
        secrets_dir.chmod(0o700)
    key_file = secrets_dir / "assistant-credential.key"
    settings.assistant_credential_key = None
    settings.assistant_credential_key_file = key_file
    try:
        yield key_file
    finally:
        settings.assistant_credential_key = previous_key
        settings.assistant_credential_key_file = previous_file


def _verify_success(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        "app.api.assistant_providers.verify_provider_connection",
        lambda *args, **kwargs: ProviderVerificationResult(
            True,
            None,
            ("planner-large", "tagger-small"),
            ("structured-text/v1",),
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
    if "burglary" in payload["request"]["prompt"].casefold():
        candidates = sorted(
            candidates,
            key=lambda item: "stealth" not in item["manual_tags"],
        )
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
        client.post("/api/assistant/providers/credential-storage/initialize"),
        client.post(
            "/api/assistant/providers/credential-storage/reset",
            json={"current_password": TEST_PASSWORD},
        ),
        client.get("/api/assistant/providers/connections"),
        client.post(
            "/api/assistant/providers/connections",
            json=_connection_payload(),
        ),
        client.delete(
            "/api/assistant/providers/connections/connection-1/credential"
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
    assert finished["progress_current"] == 11
    assert finished["progress_total"] == 11
    assert finished["result"]["evaluation"]["passed"] is True
    assert finished["result"]["evaluation"]["summary"]["passed_cases"] == 11
    assert TEST_PROVIDER_API_KEY not in json.dumps(finished)
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
            "suite_id": "model-dnd-playlist-quality-v5",
            "passed_cases": 11,
            "total_cases": 11,
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
    assert quality.json()[0]["suite_id"] == "model-dnd-playlist-quality-v5"
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
    assert finished["result"]["evaluation"]["summary"]["failed_cases"] == 11
    assert quality["status"] == "failed"
    assert quality["passed_cases"] == 0
    assert quality["total_cases"] == 11


def test_every_model_role_has_a_runtime_contract() -> None:
    assert set(MODEL_ROLE_RUNTIME_CONTRACTS) == set(MODEL_ROLE_BY_ID)


def test_role_fingerprint_includes_executable_contract_digest(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _enabled_playlist_role(auth_client, monkeypatch)
    with SessionLocal() as db:
        before = current_role_runtime_fingerprint(db, "playlist_planner")

    monkeypatch.setattr(
        "app.assistant.providers.service.role_executable_contract_digest",
        lambda _role_id: "f" * 64,
    )
    with SessionLocal() as db:
        after = current_role_runtime_fingerprint(db, "playlist_planner")

    assert before is not None
    assert after is not None
    assert after != before


def test_status_lists_supported_adapters_and_roles(auth_client: TestClient) -> None:
    response = auth_client.get("/api/assistant/providers/status")

    assert response.status_code == 200
    payload = response.json()
    assert payload["credential_storage_ready"] is True
    assert payload["credential_storage_source"] == "environment"
    assert len(payload["credential_storage_key_id"]) == 16
    assert payload["credential_storage_key_file_path"] is None
    assert payload["credential_storage_host_directory_hint"] is None
    assert payload["credential_storage_can_initialize"] is False
    assert (
        payload["credential_storage_initialization_error"]
        == "master_key_already_configured"
    )
    assert [adapter["id"] for adapter in payload["adapters"]] == [
        "openai-compatible/v1",
        "openai-compatible-json-schema/v1",
    ]
    assert payload["capabilities"] == [
        {
            "id": "structured-text/v1",
            "label": "Structured text",
            "description": (
                "Sends text instructions and receives a validated "
                "machine-readable result."
            ),
        },
        {
            "id": "strict-json-schema/v1",
            "label": "Strict JSON Schema",
            "description": (
                "Constrains model responses with the task's exact JSON Schema at the API."
            ),
        },
        {
            "id": "audio-input/v1",
            "label": "Audio input",
            "description": (
                "Accepts bounded audio content through a dedicated provider adapter."
            ),
        },
    ]
    assert payload["adapters"][0]["capability_ids"] == ["structured-text/v1"]
    assert payload["adapters"][1]["capability_ids"] == [
        "structured-text/v1",
        "strict-json-schema/v1",
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
    roles = {role["id"]: role for role in payload["roles"]}
    assert roles["playlist_planner"]["required_capability_ids"] == [
        "structured-text/v1"
    ]
    assert roles["playlist_planner"]["configuration_available"] is True
    assert roles["eq_assistant"]["required_capability_ids"] == [
        "structured-text/v1"
    ]
    assert roles["eq_assistant"]["configuration_available"] is True
    assert roles["audio_analyzer"]["required_capability_ids"] == [
        "audio-input/v1"
    ]
    assert roles["audio_analyzer"]["configuration_available"] is False
    assert "Reserved" not in descriptions["playlist_planner"]
    assert "Reserved" not in descriptions["music_tagger"]
    assert "Reserved" not in descriptions["tag_cleanup"]
    assert "Reserved" not in descriptions["eq_assistant"]
    for role_id in (
        "library_cleanup",
        "audio_analyzer",
    ):
        assert descriptions[role_id].startswith("Reserved for")


def test_status_exposes_nonsecret_credential_host_directory_hint(
    auth_client: TestClient,
) -> None:
    settings = get_settings()
    previous = settings.assistant_credential_host_directory_hint
    settings.assistant_credential_host_directory_hint = "/opt/stacks/music/secrets"
    try:
        response = auth_client.get("/api/assistant/providers/status")
    finally:
        settings.assistant_credential_host_directory_hint = previous

    assert response.status_code == 200
    assert response.json()["credential_storage_host_directory_hint"] == (
        "/opt/stacks/music/secrets"
    )


def test_environment_master_key_takes_precedence_over_configured_file(
    auth_client: TestClient,
    tmp_path: Path,
) -> None:
    settings = get_settings()
    previous_file = settings.assistant_credential_key_file
    key_file = tmp_path / "invalid-file-key"
    key_file.write_text("not-a-valid-key", encoding="ascii")
    settings.assistant_credential_key_file = key_file
    try:
        response = auth_client.get("/api/assistant/providers/status")
    finally:
        settings.assistant_credential_key_file = previous_file

    assert response.status_code == 200
    assert response.json()["credential_storage_ready"] is True
    assert response.json()["credential_storage_source"] == "environment"


def test_connection_secret_is_encrypted_and_never_returned(
    auth_client: TestClient,
) -> None:
    secret = TEST_PROVIDER_API_KEY
    created = _create_connection(auth_client, api_key=secret)

    assert created["credential_saved"] is True
    assert created["key_hint"] == "••••1234"
    assert created["verified_capability_ids"] == []
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


def test_authenticated_ui_can_initialize_fixed_key_file_storage(
    auth_client: TestClient,
    tmp_path: Path,
) -> None:
    settings = get_settings()
    previous_key = settings.assistant_credential_key
    previous_file = settings.assistant_credential_key_file
    secrets_dir = tmp_path / "assistant-secrets"
    secrets_dir.mkdir(mode=0o700)
    if os.name == "posix":
        secrets_dir.chmod(0o700)
    key_file = secrets_dir / "assistant-credential.key"
    requested_file = tmp_path / "client-chosen.key"
    settings.assistant_credential_key = None
    settings.assistant_credential_key_file = key_file
    try:
        before = auth_client.get("/api/assistant/providers/status")
        initialized = auth_client.post(
            "/api/assistant/providers/credential-storage/initialize",
            json={"key_file_path": str(requested_file)},
        )
        created = auth_client.post(
            "/api/assistant/providers/connections",
            json=_connection_payload(),
        )
    finally:
        settings.assistant_credential_key = previous_key
        settings.assistant_credential_key_file = previous_file

    assert before.status_code == 200
    assert before.json()["credential_storage_ready"] is False
    assert before.json()["credential_storage_can_initialize"] is True
    assert before.json()["credential_storage_key_file_path"] == str(key_file)
    assert initialized.status_code == 201, initialized.text
    payload = initialized.json()
    assert payload["credential_storage_ready"] is True
    assert payload["credential_storage_source"] == "file"
    assert payload["credential_storage_can_initialize"] is False
    assert len(payload["credential_storage_key_id"]) == 16
    assert key_file.exists()
    encoded_key = key_file.read_text(encoding="ascii")
    assert len(base64.urlsafe_b64decode(encoded_key)) == 32
    assert encoded_key not in initialized.text
    assert not requested_file.exists()
    if os.name == "posix":
        assert stat.S_IMODE(key_file.stat().st_mode) == 0o600
    assert created.status_code == 201, created.text


def test_complete_storage_reset_requires_current_password(
    auth_client: TestClient,
    tmp_path: Path,
) -> None:
    with _file_backed_credential_storage(tmp_path) as key_file:
        assert auth_client.post(
            "/api/assistant/providers/credential-storage/initialize"
        ).status_code == 201
        created = _create_connection(auth_client)

        response = auth_client.post(
            "/api/assistant/providers/credential-storage/reset",
            json={"current_password": "wrong-password"},
        )

        assert response.status_code == 403
        assert response.json()["detail"]["code"] == "current_password_invalid"
        assert key_file.exists()
        with SessionLocal() as db:
            row = db.get(AssistantProviderConnection, created["id"])
            assert row is not None
            assert row.encrypted_api_key


def test_complete_storage_reset_erases_credentials_then_file_key(
    auth_client: TestClient,
    tmp_path: Path,
) -> None:
    with _file_backed_credential_storage(tmp_path) as key_file:
        assert auth_client.post(
            "/api/assistant/providers/credential-storage/initialize"
        ).status_code == 201
        created = _create_connection(auth_client)
        with SessionLocal() as db:
            connection = db.get(AssistantProviderConnection, created["id"])
            assert connection is not None
            connection.verification_status = "verified"
            connection.verified_models_json = '["planner-large"]'
            connection.verified_capabilities_json = '["structured-text/v1"]'
            role = AssistantModelRole(
                role_id="playlist_planner",
                connection_id=connection.id,
                model_id="planner-large",
                enabled=True,
                conformance_status="passed",
                conformance_fingerprint="f" * 64,
            )
            job = BackgroundJob(
                id="reset-quality-job",
                kind=PLAYLIST_QUALITY_JOB_KIND,
                status="succeeded",
                parameters_json="{}",
            )
            db.add_all((role, job))
            db.flush()
            db.add(
                AssistantModelEvaluation(
                    role_id=role.role_id,
                    evaluation_id="playlist-quality-v1",
                    role_fingerprint="f" * 64,
                    status="passed",
                    suite_id="playlist-quality-v1",
                    engine_id="configured-model/v1",
                    passed_cases=1,
                    total_cases=1,
                    job_id=job.id,
                )
            )
            db.commit()

        response = auth_client.post(
            "/api/assistant/providers/credential-storage/reset",
            json={"current_password": TEST_PASSWORD},
        )

        assert response.status_code == 200, response.text
        payload = response.json()
        assert payload["deleted_credentials"] == 1
        assert payload["master_key_removed"] is True
        assert payload["master_key_removal_error"] is None
        assert payload["status"]["credential_storage_ready"] is False
        assert payload["status"]["credential_storage_can_initialize"] is True
        assert not key_file.exists()
        with SessionLocal() as db:
            connection = db.get(AssistantProviderConnection, created["id"])
            assert connection is not None
            assert connection.encrypted_api_key == ""
            assert connection.api_key_nonce == ""
            assert connection.api_key_hint == ""
            assert connection.verification_status == "never"
            assert connection.verified_models_json == "[]"
            assert connection.verified_capabilities_json == "[]"
            stored_role = db.get(AssistantModelRole, "playlist_planner")
            assert stored_role is not None
            assert stored_role.enabled is True
            assert stored_role.conformance_status == "never"
            assert stored_role.conformance_fingerprint is None
            assert db.get(
                AssistantModelEvaluation,
                ("playlist_planner", "playlist-quality-v1"),
            ) is None

        initialized = auth_client.post(
            "/api/assistant/providers/credential-storage/initialize"
        )
        assert initialized.status_code == 201, initialized.text
        replacement = auth_client.put(
            f"/api/assistant/providers/connections/{created['id']}",
            json={"api_key": TEST_REPLACEMENT_API_KEY},
        )
        assert replacement.status_code == 200, replacement.text
        assert replacement.json()["key_hint"] == "••••9999"


def test_complete_storage_reset_refuses_environment_managed_key(
    auth_client: TestClient,
) -> None:
    created = _create_connection(auth_client)

    response = auth_client.post(
        "/api/assistant/providers/credential-storage/reset",
        json={"current_password": TEST_PASSWORD},
    )

    assert response.status_code == 409
    assert response.json()["detail"]["code"] == (
        "master_key_managed_by_environment"
    )
    with SessionLocal() as db:
        row = db.get(AssistantProviderConnection, created["id"])
        assert row is not None
        assert row.encrypted_api_key


def test_complete_storage_reset_refuses_active_model_job(
    auth_client: TestClient,
    tmp_path: Path,
) -> None:
    with _file_backed_credential_storage(tmp_path) as key_file:
        assert auth_client.post(
            "/api/assistant/providers/credential-storage/initialize"
        ).status_code == 201
        created = _create_connection(auth_client)
        with SessionLocal() as db:
            db.add(
                BackgroundJob(
                    id="active-provider-job",
                    kind="assistant.model-playlist-suggestion",
                    status="queued",
                    parameters_json="{}",
                )
            )
            db.commit()

        response = auth_client.post(
            "/api/assistant/providers/credential-storage/reset",
            json={"current_password": TEST_PASSWORD},
        )

        assert response.status_code == 409
        assert response.json()["detail"]["code"] == "model_job_active"
        assert key_file.exists()
        with SessionLocal() as db:
            row = db.get(AssistantProviderConnection, created["id"])
            assert row is not None
            assert row.encrypted_api_key


def test_complete_storage_reset_reports_key_file_removal_failure(
    auth_client: TestClient,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    with _file_backed_credential_storage(tmp_path) as key_file:
        assert auth_client.post(
            "/api/assistant/providers/credential-storage/initialize"
        ).status_code == 201
        created = _create_connection(auth_client)

        def fail_removal(_key_file: Path) -> bool:
            raise CredentialVaultError("master_key_file_delete_failed")

        monkeypatch.setattr(
            "app.assistant.providers.service.remove_credential_storage_key_file",
            fail_removal,
        )
        response = auth_client.post(
            "/api/assistant/providers/credential-storage/reset",
            json={"current_password": TEST_PASSWORD},
        )

        assert response.status_code == 200, response.text
        payload = response.json()
        assert payload["deleted_credentials"] == 1
        assert payload["master_key_removed"] is False
        assert payload["master_key_removal_error"] == (
            "master_key_file_delete_failed"
        )
        assert payload["status"]["credential_storage_ready"] is True
        assert key_file.exists()
        with SessionLocal() as db:
            row = db.get(AssistantProviderConnection, created["id"])
            assert row is not None
            assert row.encrypted_api_key == ""


def test_key_file_initialization_never_overwrites_existing_file(
    auth_client: TestClient,
    tmp_path: Path,
) -> None:
    settings = get_settings()
    previous_key = settings.assistant_credential_key
    previous_file = settings.assistant_credential_key_file
    secrets_dir = tmp_path / "assistant-secrets"
    secrets_dir.mkdir(mode=0o700)
    key_file = secrets_dir / "assistant-credential.key"
    key_file.write_text("existing-invalid-value", encoding="ascii")
    if os.name == "posix":
        key_file.chmod(0o600)
    settings.assistant_credential_key = None
    settings.assistant_credential_key_file = key_file
    try:
        response = auth_client.post(
            "/api/assistant/providers/credential-storage/initialize"
        )
    finally:
        settings.assistant_credential_key = previous_key
        settings.assistant_credential_key_file = previous_file

    assert response.status_code == 409
    assert response.json()["detail"]["code"] == "master_key_file_exists"
    assert key_file.read_text(encoding="ascii") == "existing-invalid-value"


def test_key_file_initialization_refuses_to_orphan_saved_credentials(
    auth_client: TestClient,
    tmp_path: Path,
) -> None:
    _create_connection(auth_client, api_key=TEST_EXISTING_API_KEY)
    settings = get_settings()
    previous_key = settings.assistant_credential_key
    previous_file = settings.assistant_credential_key_file
    secrets_dir = tmp_path / "assistant-secrets"
    secrets_dir.mkdir(mode=0o700)
    key_file = secrets_dir / "assistant-credential.key"
    settings.assistant_credential_key = None
    settings.assistant_credential_key_file = key_file
    try:
        status = auth_client.get("/api/assistant/providers/status")
        response = auth_client.post(
            "/api/assistant/providers/credential-storage/initialize"
        )
    finally:
        settings.assistant_credential_key = previous_key
        settings.assistant_credential_key_file = previous_file

    assert status.status_code == 200
    assert status.json()["credential_storage_can_initialize"] is False
    assert status.json()["credential_storage_initialization_error"] == (
        "saved_credentials_require_existing_key"
    )
    assert response.status_code == 409
    assert response.json()["detail"]["code"] == (
        "saved_credentials_require_existing_key"
    )
    assert not key_file.exists()


@pytest.mark.skipif(os.name != "posix", reason="POSIX permission bits only")
def test_key_file_initialization_requires_private_parent_directory(
    auth_client: TestClient,
    tmp_path: Path,
) -> None:
    settings = get_settings()
    previous_key = settings.assistant_credential_key
    previous_file = settings.assistant_credential_key_file
    secrets_dir = tmp_path / "shared-secrets"
    secrets_dir.mkdir(mode=0o755)
    secrets_dir.chmod(0o755)
    key_file = secrets_dir / "assistant-credential.key"
    settings.assistant_credential_key = None
    settings.assistant_credential_key_file = key_file
    try:
        status = auth_client.get("/api/assistant/providers/status")
        response = auth_client.post(
            "/api/assistant/providers/credential-storage/initialize"
        )
    finally:
        settings.assistant_credential_key = previous_key
        settings.assistant_credential_key_file = previous_file

    assert status.status_code == 200
    assert status.json()["credential_storage_can_initialize"] is False
    assert status.json()["credential_storage_initialization_error"] == (
        "master_key_directory_permissions"
    )
    assert response.status_code == 503
    assert response.json()["detail"]["code"] == "master_key_directory_permissions"
    assert not key_file.exists()


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
            ("structured-text/v1",),
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
    assert response.json()["connection"]["verified_capability_ids"] == [
        "structured-text/v1"
    ]
    assert observed["args"] == (
        "openai-compatible/v1",
        "https://models.example.test/v1",
        TEST_PROVIDER_API_KEY,
    )
    assert TEST_PROVIDER_API_KEY not in response.text


def test_connection_invalidation_refuses_active_assigned_model_job(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    role = _enabled_playlist_role(auth_client, monkeypatch)
    with SessionLocal() as db:
        db.add(
            BackgroundJob(
                id="active-quality-during-reverification",
                kind=PLAYLIST_QUALITY_JOB_KIND,
                status="running",
                parameters_json=json.dumps(
                    {
                        "role_id": "playlist_planner",
                        "evaluation_id": "playlist-quality-v1",
                    }
                ),
            )
        )
        db.commit()

    response = auth_client.post(
        f"/api/assistant/providers/connections/{role['connection_id']}/verify"
    )

    assert response.status_code == 409
    assert response.json()["detail"]["code"] == "connection_model_job_active"
    assert response.json()["detail"]["message"] == (
        "Wait for or cancel active model work before changing or verifying this connection."
    )
    changed = auth_client.put(
        f"/api/assistant/providers/connections/{role['connection_id']}",
        json={"base_url": "https://other.example.test/api"},
    )
    deleted = auth_client.delete(
        f"/api/assistant/providers/connections/{role['connection_id']}/credential"
    )
    assert changed.status_code == 409
    assert changed.json()["detail"]["code"] == "connection_model_job_active"
    assert deleted.status_code == 409
    assert deleted.json()["detail"]["code"] == "connection_model_job_active"
    roles = auth_client.get("/api/assistant/providers/roles").json()
    stored = next(item for item in roles if item["role_id"] == "playlist_planner")
    assert stored["conformance_status"] == "passed"
    assert stored["effective_enabled"] is True


def test_reverification_finish_refuses_job_started_during_provider_request(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    role = _enabled_playlist_role(auth_client, monkeypatch)
    with SessionLocal() as db:
        target = prepare_verification(db, cast(str, role["connection_id"]))
    with SessionLocal() as db:
        db.add(
            BackgroundJob(
                id="quality-started-during-reverification",
                kind=PLAYLIST_QUALITY_JOB_KIND,
                status="running",
                parameters_json=json.dumps(
                    {
                        "role_id": "playlist_planner",
                        "evaluation_id": "playlist-quality-v1",
                    }
                ),
            )
        )
        db.commit()
    result = ProviderVerificationResult(
        True,
        None,
        ("planner-large",),
        ("structured-text/v1",),
    )

    with SessionLocal() as db, pytest.raises(ProviderServiceError) as exc_info:
        finish_verification(db, target, result)

    assert exc_info.value.code == "connection_model_job_active"
    with SessionLocal() as db:
        stored = db.get(AssistantModelRole, "playlist_planner")
        assert stored is not None
        assert stored.conformance_status == "passed"


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
    assert response.json()["verified_capability_ids"] == []
    assert response.json()["last_verified_at"] is None


def test_saved_connection_credential_cannot_be_replaced_in_place(
    auth_client: TestClient,
) -> None:
    created = _create_connection(auth_client)
    replacement_key = TEST_REPLACEMENT_API_KEY

    response = auth_client.put(
        f"/api/assistant/providers/connections/{created['id']}",
        json={"api_key": replacement_key},
    )

    assert response.status_code == 409
    assert response.json()["detail"]["code"] == "credential_already_saved"
    listed = auth_client.get("/api/assistant/providers/connections")
    assert listed.status_code == 200
    assert listed.json()[0]["key_hint"] == "••••1234"


def test_connection_credential_can_be_deleted_then_replaced(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    created = _create_connection(auth_client)
    _verify_success(monkeypatch)
    assert auth_client.post(
        f"/api/assistant/providers/connections/{created['id']}/verify"
    ).status_code == 200
    role_payload = {
        "connection_id": created["id"],
        "model_id": "planner-large",
        "enabled": False,
    }
    assert auth_client.put(
        "/api/assistant/providers/roles/playlist_planner",
        json=role_payload,
    ).status_code == 200
    _conformance_success(monkeypatch)
    assert auth_client.post(
        "/api/assistant/providers/roles/playlist_planner/test"
    ).status_code == 200
    assert auth_client.put(
        "/api/assistant/providers/roles/playlist_planner",
        json={**role_payload, "enabled": True},
    ).json()["effective_enabled"] is True

    deleted = auth_client.delete(
        f"/api/assistant/providers/connections/{created['id']}/credential"
    )

    assert deleted.status_code == 200, deleted.text
    assert deleted.json()["credential_saved"] is False
    assert deleted.json()["key_hint"] is None
    assert deleted.json()["verification_status"] == "never"
    assert deleted.json()["verified_models"] == []
    assert deleted.json()["verified_capability_ids"] == []
    assert deleted.json()["last_verified_at"] is None
    with SessionLocal() as db:
        row = db.get(AssistantProviderConnection, created["id"])
        assert row is not None
        assert row.encrypted_api_key == ""
        assert row.api_key_nonce == ""
        assert row.api_key_hint == ""
        assert row.verified_capabilities_json == "[]"

    roles = auth_client.get("/api/assistant/providers/roles")
    planner = next(
        role for role in roles.json() if role["role_id"] == "playlist_planner"
    )
    assert planner["enabled"] is True
    assert planner["effective_enabled"] is False
    assert planner["verification_status"] == "never"
    assert planner["conformance_status"] == "never"
    missing = auth_client.post(
        f"/api/assistant/providers/connections/{created['id']}/verify"
    )
    assert missing.status_code == 409
    assert missing.json()["detail"]["code"] == "credential_missing"

    replacement = auth_client.put(
        f"/api/assistant/providers/connections/{created['id']}",
        json={"api_key": TEST_REPLACEMENT_API_KEY},
    )
    assert replacement.status_code == 200, replacement.text
    assert replacement.json()["credential_saved"] is True
    assert replacement.json()["key_hint"] == "••••9999"
    assert replacement.json()["verification_status"] == "never"

    observed: dict[str, object] = {}

    def verify(*args: object, **kwargs: object) -> ProviderVerificationResult:
        observed["args"] = args
        return ProviderVerificationResult(
            True,
            None,
            ("planner-large",),
            ("structured-text/v1",),
        )

    monkeypatch.setattr(
        "app.api.assistant_providers.verify_provider_connection",
        verify,
    )
    verified = auth_client.post(
        f"/api/assistant/providers/connections/{created['id']}/verify"
    )
    assert verified.status_code == 200
    assert observed["args"] == (
        "openai-compatible/v1",
        "https://models.example.test/v1",
        TEST_REPLACEMENT_API_KEY,
    )


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
            ProviderVerificationResult(
                True,
                None,
                ("stale-model",),
                ("structured-text/v1",),
            ),
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
    assert target.api_key == TEST_PROVIDER_API_KEY


@pytest.mark.parametrize(
    ("contract_name", "next_contract"),
    [
        (
            "CONFORMANCE_CONTRACT",
            "assistant-provider-conformance/test-next-version",
        ),
        (
            "STRUCTURED_HARNESS_CONTRACT",
            "assistant-structured-harness/test-next-version",
        ),
    ],
)
def test_execution_contract_change_invalidates_existing_model_test(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
    contract_name: str,
    next_contract: str,
) -> None:
    created = _create_connection(auth_client)
    _verify_success(monkeypatch)
    assert auth_client.post(
        f"/api/assistant/providers/connections/{created['id']}/verify"
    ).status_code == 200
    role_payload = {
        "connection_id": created["id"],
        "model_id": "planner-large",
        "enabled": False,
    }
    assert auth_client.put(
        "/api/assistant/providers/roles/playlist_planner",
        json=role_payload,
    ).status_code == 200
    _conformance_success(monkeypatch)
    assert auth_client.post(
        "/api/assistant/providers/roles/playlist_planner/test"
    ).status_code == 200
    role_payload["enabled"] = True
    assert auth_client.put(
        "/api/assistant/providers/roles/playlist_planner",
        json=role_payload,
    ).status_code == 200

    monkeypatch.setattr(
        f"app.assistant.providers.service.{contract_name}",
        next_contract,
    )
    roles = auth_client.get("/api/assistant/providers/roles")
    planner = next(
        item for item in roles.json() if item["role_id"] == "playlist_planner"
    )

    assert planner["enabled"] is True
    assert planner["effective_enabled"] is False
    assert planner["conformance_status"] == "never"


def test_reserved_roles_cannot_be_configured_or_tested(
    auth_client: TestClient,
) -> None:
    created = _create_connection(auth_client)

    configured = auth_client.put(
        "/api/assistant/providers/roles/library_cleanup",
        json={
            "connection_id": created["id"],
            "model_id": "future-eq-model",
            "enabled": False,
        },
    )
    tested = auth_client.post("/api/assistant/providers/roles/library_cleanup/test")

    assert configured.status_code == 409
    assert configured.json()["detail"]["code"] == "role_not_available"
    assert tested.status_code == 409
    assert tested.json()["detail"]["code"] == "role_not_available"


def test_role_fails_closed_when_verification_does_not_confirm_capability(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    created = _create_connection(auth_client)
    monkeypatch.setattr(
        "app.api.assistant_providers.verify_provider_connection",
        lambda *args, **kwargs: ProviderVerificationResult(
            True,
            None,
            ("planner-large",),
            (),
        ),
    )
    verified = auth_client.post(
        f"/api/assistant/providers/connections/{created['id']}/verify"
    )
    role_payload = {
        "connection_id": created["id"],
        "model_id": "planner-large",
        "enabled": False,
    }
    saved = auth_client.put(
        "/api/assistant/providers/roles/playlist_planner",
        json=role_payload,
    )
    tested = auth_client.post(
        "/api/assistant/providers/roles/playlist_planner/test"
    )
    enabled = auth_client.put(
        "/api/assistant/providers/roles/playlist_planner",
        json={**role_payload, "enabled": True},
    )

    assert verified.status_code == 200
    assert verified.json()["connection"]["verified_capability_ids"] == []
    assert saved.status_code == 200
    assert saved.json()["effective_enabled"] is False
    assert tested.status_code == 409
    assert tested.json()["detail"]["code"] == "incompatible_connection"
    assert enabled.status_code == 409
    assert enabled.json()["detail"]["code"] == "incompatible_connection"


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
            provider_model_id="tagger-small-2026",
            finish_reason="stop",
            input_tokens=31,
            output_tokens=9,
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
    assert tested.json()["contract_version"] == "assistant-provider-conformance/v3"
    assert tested.json()["provider_model_id"] == "tagger-small-2026"
    assert tested.json()["finish_reason"] == "stop"
    assert tested.json()["input_tokens"] == 31
    assert tested.json()["output_tokens"] == 9
    assert tested.json()["duration_ms"] >= 0
    assert tested.json()["role"]["conformance_status"] == "failed"
    assert enable.status_code == 409
    assert enable.json()["detail"]["code"] == "model_not_tested"


def test_changing_thinking_mode_invalidates_previous_model_test(
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
            "thinking_mode": "disabled",
            "timeout_seconds": 30,
            "max_output_tokens": 2000,
        },
    )

    assert changed.status_code == 200
    assert changed.json()["thinking_mode"] == "disabled"
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
