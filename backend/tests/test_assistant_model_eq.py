import json
import time
from collections.abc import Iterator
from typing import Any

import pytest
from fastapi.testclient import TestClient
from sqlalchemy import delete

from app.assistant.model_eq import (
    EQ_DRAFT_OUTPUT_CONTRACT,
    ModelEqError,
    generate_eq_draft,
)
from app.assistant.model_eq_job import MODEL_EQ_DRAFT_JOB_KIND
from app.assistant.model_evaluation import EQ_QUALITY_JOB_KIND
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

DISCLOSURE_VERSION = "assistant-eq-draft-disclosure/v1"


@pytest.fixture(autouse=True)
def _clean_eq_configuration() -> Iterator[None]:
    def clean() -> None:
        with SessionLocal() as db:
            db.execute(delete(AssistantModelEvaluation))
            db.execute(
                delete(BackgroundJob).where(
                    BackgroundJob.kind.in_([EQ_QUALITY_JOB_KIND, MODEL_EQ_DRAFT_JOB_KIND])
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


def _reference_eq_model(
    _target: object,
    request: StructuredModelRequest,
) -> StructuredModelResult:
    model_input = json.loads(request.user_prompt)
    goal = model_input["goal"].casefold()
    gains = [0.0] * 10
    if "warm" in goal or "tavern" in goal:
        gains[3] = 2.0
        gains[8] = -1.0
    elif "harsh" in goal or "fatigue" in goal:
        gains[7] = -2.0
        gains[9] = -1.0
    else:
        gains[0] = -1.0
        gains[6] = 2.0
    return StructuredModelResult(
        True,
        None,
        {
            "schema_version": EQ_DRAFT_OUTPUT_CONTRACT,
            "gains_db": gains,
            "rationale": "Small, reviewable changes matched to the supplied goal.",
            "cautions": ["Check the result on the intended speakers."],
        },
        provider_model_id="eq-reference-model",
        input_tokens=40,
        output_tokens=20,
    )


def _configure_quality_passed_eq(
    client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    created = client.post(
        "/api/assistant/providers/connections",
        json={
            "name": "EQ model",
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
            ("eq-model",),
            ("structured-text/v1",),
        ),
    )
    assert client.post(
        f"/api/assistant/providers/connections/{connection['id']}/verify"
    ).status_code == 200
    role = {
        "connection_id": connection["id"],
        "model_id": "eq-model",
        "enabled": False,
    }
    assert client.put(
        "/api/assistant/providers/roles/eq_assistant",
        json=role,
    ).status_code == 200
    monkeypatch.setattr(
        "app.api.assistant_providers.run_provider_conformance",
        lambda *args, **kwargs: ProviderConformanceResult(True, None),
    )
    assert client.post(
        "/api/assistant/providers/roles/eq_assistant/test"
    ).status_code == 200
    role["enabled"] = True
    assert client.put(
        "/api/assistant/providers/roles/eq_assistant",
        json=role,
    ).status_code == 200
    monkeypatch.setattr(
        "app.assistant.model_evaluation.execute_structured_model_request",
        _reference_eq_model,
    )
    started = client.post(
        "/api/assistant/providers/roles/eq_assistant/"
        "evaluations/eq-quality-v1/jobs"
    )
    assert started.status_code == 202, started.text
    finished = _wait_for_job(client, started.json()["id"], {"succeeded"})
    assert finished["result"]["evaluation"]["passed"] is True


def test_eq_contract_builds_only_canonical_bands() -> None:
    observed: list[StructuredModelRequest] = []

    def capture(request: StructuredModelRequest) -> StructuredModelResult:
        observed.append(request)
        return _reference_eq_model(object(), request)

    draft = generate_eq_draft("Warm Tavern", "warm wooden tavern", capture)

    assert [band["frequency"] for band in draft.bands] == [
        32,
        64,
        125,
        250,
        500,
        1000,
        2000,
        4000,
        8000,
        16000,
    ]
    assert draft.bands[3]["gain"] == 2.0
    assert json.loads(observed[0].user_prompt)["goal"] == "warm wooden tavern"
    assert "untrusted user data" in observed[0].system_prompt


def test_eq_contract_rejects_non_half_db_steps() -> None:
    def invalid(_request: StructuredModelRequest) -> StructuredModelResult:
        return StructuredModelResult(
            True,
            None,
            {
                "schema_version": EQ_DRAFT_OUTPUT_CONTRACT,
                "gains_db": [0.3] * 10,
                "rationale": "Invalid precision.",
                "cautions": [],
            },
        )

    with pytest.raises(ModelEqError, match="model_output_schema_invalid"):
        generate_eq_draft("Invalid", "some sound goal", invalid)


def test_eq_endpoints_are_consent_bound_durable_and_review_only(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    before = auth_client.get("/api/assistant/eq/model-status")
    assert before.status_code == 200
    assert before.json()["available"] is False

    _configure_quality_passed_eq(auth_client, monkeypatch)
    after = auth_client.get("/api/assistant/eq/model-status")
    assert after.status_code == 200
    assert after.json()["available"] is True
    assert after.json()["connection_name"] == "EQ model"
    assert any(
        "Songs" in item for item in after.json()["disclosure"]["never_shared"]
    )
    monkeypatch.setattr(
        "app.assistant.model_eq_job.execute_structured_model_request",
        _reference_eq_model,
    )
    started = auth_client.post(
        "/api/assistant/eq/drafts/jobs",
        json={
            "request": {
                "name": "Warm Tavern",
                "goal": "Warm wooden medieval tavern with softer highs",
            },
            "disclosure_version": DISCLOSURE_VERSION,
            "consent": True,
        },
    )
    assert started.status_code == 202, started.text
    finished = _wait_for_job(auth_client, started.json()["id"], {"succeeded"})
    assert finished["kind"] == MODEL_EQ_DRAFT_JOB_KIND
    assert finished["result"]["draft"]["name"] == "Warm Tavern"
    assert finished["result"]["engine_id"] == "model-graphic-eq/v1"
    assert finished["result"]["usage"]["attempted_requests"] == 1
    assert "secret-provider-key-1234" not in json.dumps(finished)


@pytest.mark.parametrize(
    "payload",
    [
        {
            "request": {"name": "EQ", "goal": "warm sound"},
            "disclosure_version": DISCLOSURE_VERSION,
            "consent": False,
        },
        {
            "request": {"name": "EQ", "goal": "warm sound"},
            "disclosure_version": "outdated",
            "consent": True,
        },
    ],
)
def test_eq_endpoint_requires_exact_current_consent(
    auth_client: TestClient,
    payload: dict[str, object],
) -> None:
    response = auth_client.post("/api/assistant/eq/drafts/jobs", json=payload)
    assert response.status_code == 422


def test_eq_endpoints_require_authentication(client: TestClient) -> None:
    assert client.get("/api/assistant/eq/model-status").status_code == 401
    assert client.post(
        "/api/assistant/eq/drafts/jobs",
        json={
            "request": {"name": "EQ", "goal": "warm sound"},
            "disclosure_version": DISCLOSURE_VERSION,
            "consent": True,
        },
    ).status_code == 401
