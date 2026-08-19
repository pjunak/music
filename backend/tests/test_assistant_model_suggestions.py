from __future__ import annotations

import json
import threading
import time
from collections.abc import Iterator
from typing import Any, cast

import pytest
from fastapi.testclient import TestClient
from sqlalchemy import delete, func, select

from app.assistant.model_evaluation import PLAYLIST_QUALITY_JOB_KIND
from app.assistant.model_playlist import MODEL_PLAYLIST_OUTPUT_CONTRACT
from app.assistant.model_suggestions import MODEL_PLAYLIST_SUGGESTION_JOB_KIND
from app.assistant.providers.execution import (
    ProviderConformanceResult,
    StructuredModelRequest,
    StructuredModelResult,
)
from app.assistant.providers.verification import ProviderVerificationResult
from app.core.db import SessionLocal
from app.models.assistant_model_evaluation import AssistantModelEvaluation
from app.models.assistant_model_role import AssistantModelRole
from app.models.assistant_provider_connection import AssistantProviderConnection
from app.models.background_job import BackgroundJob
from app.models.playlist import Playlist

DISCLOSURE_VERSION = "assistant-playlist-model-disclosure/v1"


@pytest.fixture(autouse=True)
def _clean_model_suggestion_configuration() -> Iterator[None]:
    def clean() -> None:
        with SessionLocal() as db:
            db.execute(delete(AssistantModelEvaluation))
            db.execute(
                delete(BackgroundJob).where(
                    BackgroundJob.kind.in_(
                        [
                            PLAYLIST_QUALITY_JOB_KIND,
                            MODEL_PLAYLIST_SUGGESTION_JOB_KIND,
                        ]
                    )
                )
            )
            db.execute(delete(AssistantModelRole))
            db.execute(delete(AssistantProviderConnection))
            db.commit()

    clean()
    yield
    clean()


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
        provider_model_id="planner-response-model",
        input_tokens=120,
        output_tokens=30,
    )


def _configure_quality_passed_model(
    client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    created = client.post(
        "/api/assistant/providers/connections",
        json={
            "name": "Playlist models",
            "adapter_id": "openai-compatible/v1",
            "base_url": "https://models.example.test/v1",
            "api_key": "secret-provider-key-1234",
            "allow_private_network": False,
        },
    )
    assert created.status_code == 201, created.text
    connection = created.json()
    monkeypatch.setattr(
        "app.api.assistant_providers.verify_provider_connection",
        lambda *args, **kwargs: ProviderVerificationResult(
            True,
            None,
            ("planner-large",),
            ("structured-text/v1",),
        ),
    )
    assert client.post(
        f"/api/assistant/providers/connections/{connection['id']}/verify"
    ).status_code == 200
    role_payload = {
        "connection_id": connection["id"],
        "model_id": "planner-large",
        "enabled": False,
    }
    assert client.put(
        "/api/assistant/providers/roles/playlist_planner",
        json=role_payload,
    ).status_code == 200
    monkeypatch.setattr(
        "app.api.assistant_providers.run_provider_conformance",
        lambda *args, **kwargs: ProviderConformanceResult(True, None),
    )
    assert client.post(
        "/api/assistant/providers/roles/playlist_planner/test"
    ).status_code == 200
    role_payload["enabled"] = True
    assert client.put(
        "/api/assistant/providers/roles/playlist_planner",
        json=role_payload,
    ).status_code == 200
    monkeypatch.setattr(
        "app.assistant.model_evaluation.execute_structured_model_request",
        _reference_playlist_model,
    )
    started = client.post(
        "/api/assistant/providers/roles/playlist_planner/"
        "evaluations/playlist-quality-v1/jobs"
    )
    assert started.status_code == 202, started.text
    finished = _wait_for_job(client, started.json()["id"], {"succeeded"})
    assert finished["result"]["evaluation"]["passed"] is True
    quality_usage = finished["result"]["usage"]
    assert quality_usage["attempted_requests"] > 0
    assert quality_usage["input_tokens"] == (
        quality_usage["attempted_requests"] * 120
    )
    assert quality_usage["output_tokens"] == (
        quality_usage["attempted_requests"] * 30
    )
    assert quality_usage["provider_model_ids"] == ["planner-response-model"]


def _start_payload(prompt: str = "Warm medieval tavern") -> dict[str, object]:
    return {
        "request": {
            "prompt": prompt,
            "target_minutes": 45,
            "candidate_limit": 20,
            "energy_curve": "arc",
        },
        "disclosure_version": DISCLOSURE_VERSION,
        "consent": True,
    }


def test_model_playlist_endpoints_require_authentication(client: TestClient) -> None:
    status = client.get("/api/assistant/playlists/model-status")
    started = client.post(
        "/api/assistant/playlists/model-suggestions/jobs",
        json=_start_payload(),
    )

    assert status.status_code == 401
    assert started.status_code == 401


def test_model_status_requires_current_quality_pass_and_discloses_scope(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    before = auth_client.get("/api/assistant/playlists/model-status")
    assert before.status_code == 200
    assert before.json()["available"] is False

    _configure_quality_passed_model(auth_client, monkeypatch)
    after = auth_client.get("/api/assistant/playlists/model-status")

    assert after.status_code == 200
    payload = after.json()
    assert payload["available"] is True
    assert payload["connection_name"] == "Playlist models"
    assert payload["model_id"] == "planner-large"
    assert payload["job_kind"] == MODEL_PLAYLIST_SUGGESTION_JOB_KIND
    assert payload["disclosure"]["version"] == DISCLOSURE_VERSION
    assert payload["disclosure"]["maximum_candidates"] == 100
    assert payload["disclosure"]["may_incur_cost"] is True
    assert any(
        "Filesystem" in item for item in payload["disclosure"]["never_shared"]
    )


@pytest.mark.parametrize(
    "payload",
    [
        {**_start_payload(), "consent": False},
        {**_start_payload(), "disclosure_version": "outdated"},
        {key: value for key, value in _start_payload().items() if key != "consent"},
    ],
)
def test_model_suggestion_requires_exact_current_consent(
    auth_client: TestClient,
    payload: dict[str, object],
) -> None:
    response = auth_client.post(
        "/api/assistant/playlists/model-suggestions/jobs",
        json=payload,
    )

    assert response.status_code == 422


def test_model_suggestion_is_path_free_durable_and_preview_only(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _configure_quality_passed_model(auth_client, monkeypatch)
    observed: list[dict[str, Any]] = []

    def capture(
        target: object,
        request: StructuredModelRequest,
    ) -> StructuredModelResult:
        observed.append(json.loads(request.user_prompt))
        return _reference_playlist_model(target, request)

    monkeypatch.setattr(
        "app.assistant.model_suggestions.execute_structured_model_request",
        capture,
    )
    with SessionLocal() as db:
        playlists_before = db.scalar(select(func.count()).select_from(Playlist))

    started = auth_client.post(
        "/api/assistant/playlists/model-suggestions/jobs",
        json=_start_payload(),
    )

    assert started.status_code == 202, started.text
    initial = started.json()
    assert initial["parameters"]["consent"] is True
    assert initial["parameters"]["disclosure_version"] == DISCLOSURE_VERSION
    finished = _wait_for_job(auth_client, initial["id"], {"succeeded"})
    assert finished["progress_current"] == 3
    assert finished["progress_total"] == 3
    assert finished["progress_phase"] == "Complete"
    assert finished["result"]["schema_version"] == (
        "assistant-playlist-suggestion-job-result/v1"
    )
    assert finished["result"]["suggestion"]["engine"] == (
        "model-playlist-planner/v1"
    )
    assert finished["result"]["usage"] == {
        "schema_version": "assistant-provider-usage/v1",
        "attempted_requests": 1,
        "input_tokens": 120,
        "output_tokens": 30,
        "input_tokens_reported_requests": 1,
        "output_tokens_reported_requests": 1,
        "provider_model_ids": ["planner-response-model"],
        "provider_model_ids_truncated": False,
    }
    assert "secret-provider-key-1234" not in json.dumps(finished)
    assert observed
    assert len(observed[0]["candidates"]) <= 100
    assert all("path" not in candidate for candidate in observed[0]["candidates"])
    assert "path" not in json.dumps(finished["parameters"])
    assert finished["result"]["suggestion"]["candidates"][0]["path"]
    restored = auth_client.get(
        "/api/jobs",
        params={"kind": MODEL_PLAYLIST_SUGGESTION_JOB_KIND},
    )
    assert restored.status_code == 200
    assert restored.json()[0]["id"] == initial["id"]
    with SessionLocal() as db:
        assert db.scalar(select(func.count()).select_from(Playlist)) == playlists_before


def test_model_suggestion_rejects_stale_quality_certification(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _configure_quality_passed_model(auth_client, monkeypatch)
    with SessionLocal() as db:
        row = db.get(
            AssistantModelEvaluation,
            ("playlist_planner", "playlist-quality-v1"),
        )
        assert row is not None
        row.role_fingerprint = "0" * 64
        db.commit()

    response = auth_client.post(
        "/api/assistant/playlists/model-suggestions/jobs",
        json=_start_payload(),
    )

    assert response.status_code == 409
    assert response.json()["detail"]["code"] == "model_quality_not_passed"


def test_failed_model_suggestion_retains_attempted_provider_usage(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _configure_quality_passed_model(auth_client, monkeypatch)
    monkeypatch.setattr(
        "app.assistant.model_suggestions.execute_structured_model_request",
        lambda *_args, **_kwargs: StructuredModelResult(False, "timeout"),
    )

    started = auth_client.post(
        "/api/assistant/playlists/model-suggestions/jobs",
        json=_start_payload(),
    )
    finished = _wait_for_job(auth_client, started.json()["id"], {"failed"})

    assert finished["result"] == {
        "schema_version": "assistant-provider-usage-checkpoint/v1",
        "usage": {
            "schema_version": "assistant-provider-usage/v1",
            "attempted_requests": 1,
            "input_tokens": 0,
            "output_tokens": 0,
            "input_tokens_reported_requests": 0,
            "output_tokens_reported_requests": 0,
            "provider_model_ids": [],
            "provider_model_ids_truncated": False,
        },
    }


def test_different_model_suggestion_cannot_join_active_job(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _configure_quality_passed_model(auth_client, monkeypatch)
    entered = threading.Event()
    release = threading.Event()

    def block(
        target: object,
        request: StructuredModelRequest,
    ) -> StructuredModelResult:
        entered.set()
        assert release.wait(timeout=5)
        return _reference_playlist_model(target, request)

    monkeypatch.setattr(
        "app.assistant.model_suggestions.execute_structured_model_request",
        block,
    )
    first = auth_client.post(
        "/api/assistant/playlists/model-suggestions/jobs",
        json=_start_payload("Misty medieval forest"),
    )
    assert first.status_code == 202, first.text
    assert entered.wait(timeout=5)
    try:
        duplicate = auth_client.post(
            "/api/assistant/playlists/model-suggestions/jobs",
            json=_start_payload("Busy dancing tavern"),
        )
        same = auth_client.post(
            "/api/assistant/playlists/model-suggestions/jobs",
            json=_start_payload("Misty medieval forest"),
        )

        assert duplicate.status_code == 409
        assert duplicate.json()["detail"]["code"] == (
            "model_suggestion_in_progress"
        )
        assert same.status_code == 202
        assert same.json()["id"] == first.json()["id"]
    finally:
        release.set()
    _wait_for_job(auth_client, first.json()["id"], {"succeeded"})
