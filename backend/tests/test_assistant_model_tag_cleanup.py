import json
import time
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from fastapi.testclient import TestClient
from sqlalchemy import delete

from app.assistant.model_evaluation import TAG_CLEANUP_QUALITY_JOB_KIND
from app.assistant.model_tag_cleanup import (
    MODEL_TAG_CLEANUP_OUTPUT_CONTRACT,
    ModelTagCleanupError,
    evaluate_model_tag_cleanup,
    load_tag_cleanup_quality_suite,
    suggest_model_tag_cleanup,
)
from app.assistant.providers.execution import (
    ProviderConformanceResult,
    StructuredModelRequest,
    StructuredModelResult,
)
from app.assistant.providers.verification import ProviderVerificationResult
from app.assistant.tags import TagUsage
from app.core.db import SessionLocal
from app.models.assistant_model_evaluation import AssistantModelEvaluation
from app.models.assistant_model_role import AssistantModelRole
from app.models.assistant_provider_connection import AssistantProviderConnection
from app.models.background_job import BackgroundJob

_SUITE_PATH = (
    Path(__file__).resolve().parents[1] / "evaluation" / "tag-cleanup-v1.json"
)


@pytest.fixture(autouse=True)
def _clean_tag_cleanup_model_configuration() -> Iterator[None]:
    def clean() -> None:
        with SessionLocal() as db:
            db.execute(delete(AssistantModelEvaluation))
            db.execute(
                delete(BackgroundJob).where(
                    BackgroundJob.kind == TAG_CLEANUP_QUALITY_JOB_KIND
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


_REFERENCE_MERGES = {
    "inn": "tavern",
    "pub": "tavern",
    "battle": "combat",
    "journey": "travel",
    "medival": "medieval",
    "taverns": "tavern",
    "ruin": "ruins",
    "ocean voyage": "seafaring",
}


def _reference_cleanup_model(
    _target: object,
    request: StructuredModelRequest,
) -> StructuredModelResult:
    payload = json.loads(request.user_prompt)
    used = {item["tag"] for item in payload["used_tags"]}
    allowed = used | set(payload["starter_tags"])
    suggestions = [
        {
            "source": source,
            "target": target,
            "confidence": "high",
            "reason": "Synthetic reference synonym or spelling match",
        }
        for source, target in _REFERENCE_MERGES.items()
        if source in used and target in allowed
    ]
    return StructuredModelResult(
        True,
        None,
        {
            "schema_version": MODEL_TAG_CLEANUP_OUTPUT_CONTRACT,
            "suggestions": suggestions,
        },
        provider_model_id="cleanup-response-model",
        input_tokens=60,
        output_tokens=15,
    )


def _configure_enabled_cleanup_role(
    client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    created = client.post(
        "/api/assistant/providers/connections",
        json={
            "name": "Cleanup models",
            "adapter_id": "openai-compatible/v1",
            "base_url": "https://models.example.test/v1",
            "api_key": "secret-cleanup-key-1234",
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
            ("cleanup-small",),
        ),
    )
    assert client.post(
        f"/api/assistant/providers/connections/{connection['id']}/verify"
    ).status_code == 200
    role_payload = {
        "connection_id": connection["id"],
        "model_id": "cleanup-small",
        "enabled": False,
    }
    assert client.put(
        "/api/assistant/providers/roles/tag_cleanup",
        json=role_payload,
    ).status_code == 200
    monkeypatch.setattr(
        "app.api.assistant_providers.run_provider_conformance",
        lambda *args, **kwargs: ProviderConformanceResult(True, None),
    )
    assert client.post(
        "/api/assistant/providers/roles/tag_cleanup/test"
    ).status_code == 200
    role_payload["enabled"] = True
    assert client.put(
        "/api/assistant/providers/roles/tag_cleanup",
        json=role_payload,
    ).status_code == 200


def test_cleanup_model_rejects_unknown_targets_and_chained_renames() -> None:
    usage = [TagUsage(tag="inn", track_count=3), TagUsage(tag="pub", track_count=2)]

    def execute_with(suggestions: list[dict[str, object]]) -> StructuredModelResult:
        return StructuredModelResult(
            True,
            None,
            {
                "schema_version": MODEL_TAG_CLEANUP_OUTPUT_CONTRACT,
                "suggestions": suggestions,
            },
        )

    with pytest.raises(ModelTagCleanupError, match="model_output_unknown_target"):
        suggest_model_tag_cleanup(
            usage,
            lambda _request: execute_with(
                [
                    {
                        "source": "inn",
                        "target": "invented-tag",
                        "confidence": "high",
                        "reason": "Not allowed",
                    }
                ]
            ),
        )

    with pytest.raises(ModelTagCleanupError, match="model_output_chained_rename"):
        suggest_model_tag_cleanup(
            usage,
            lambda _request: execute_with(
                [
                    {
                        "source": "inn",
                        "target": "pub",
                        "confidence": "high",
                        "reason": "First link",
                    },
                    {
                        "source": "pub",
                        "target": "tavern",
                        "confidence": "high",
                        "reason": "Second link",
                    },
                ]
            ),
        )


def test_reference_cleanup_model_passes_fixed_quality_suite() -> None:
    suite = load_tag_cleanup_quality_suite(_SUITE_PATH)
    result = evaluate_model_tag_cleanup(
        lambda request: _reference_cleanup_model(object(), request),
        suite,
    )

    assert result.passed is True
    assert result.passed_cases == result.total_cases == 8


def test_tag_cleanup_quality_job_persists_certification_and_usage(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _configure_enabled_cleanup_role(auth_client, monkeypatch)
    monkeypatch.setattr(
        "app.assistant.model_evaluation.execute_structured_model_request",
        _reference_cleanup_model,
    )

    started = auth_client.post(
        "/api/assistant/providers/roles/tag_cleanup/"
        "evaluations/tag-cleanup-quality-v1/jobs"
    )
    assert started.status_code == 202, started.text
    finished = _wait_for_job(auth_client, started.json()["id"], {"succeeded"})

    assert finished["kind"] == TAG_CLEANUP_QUALITY_JOB_KIND
    assert finished["progress_current"] == 8
    assert finished["progress_total"] == 8
    assert finished["result"]["evaluation"]["passed"] is True
    assert finished["result"]["usage"] == {
        "schema_version": "assistant-provider-usage/v1",
        "attempted_requests": 8,
        "input_tokens": 480,
        "output_tokens": 120,
        "input_tokens_reported_requests": 8,
        "output_tokens_reported_requests": 8,
        "provider_model_ids": ["cleanup-response-model"],
        "provider_model_ids_truncated": False,
    }
    assert "secret-cleanup-key-1234" not in json.dumps(finished)

    evaluations = auth_client.get(
        "/api/assistant/providers/roles/tag_cleanup/evaluations"
    )
    assert evaluations.status_code == 200, evaluations.text
    assert evaluations.json()[0]["status"] == "passed"
    assert evaluations.json()[0]["suite_id"] == "dnd-tag-cleanup-baseline-v1"
