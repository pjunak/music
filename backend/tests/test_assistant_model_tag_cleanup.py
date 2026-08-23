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
from app.assistant.model_tag_cleanup_job import MODEL_TAG_CLEANUP_JOB_KIND
from app.assistant.providers.execution import (
    ProviderConformanceResult,
    StructuredModelRequest,
    StructuredModelResult,
)
from app.assistant.providers.verification import ProviderVerificationResult
from app.assistant.tag_vocabulary import (
    TagVocabularyDocument,
    TagVocabularyEntry,
    TagVocabularyGroup,
    TagVocabularySnapshot,
    vocabulary_fingerprint,
)
from app.assistant.tags import TagUsage
from app.core.db import SessionLocal
from app.models.assistant_model_evaluation import AssistantModelEvaluation
from app.models.assistant_model_role import AssistantModelRole
from app.models.assistant_provider_connection import AssistantProviderConnection
from app.models.assistant_tag_vocabulary import AssistantTagVocabulary
from app.models.background_job import BackgroundJob
from app.models.track_user_tag import TrackUserTag

from .assistant_test_values import TEST_CLEANUP_API_KEY

_SUITE_PATH = (
    Path(__file__).resolve().parents[1]
    / "app"
    / "assistant"
    / "evaluation_suites"
    / "tag-cleanup-v1.json"
)


@pytest.fixture(autouse=True)
def _clean_tag_cleanup_model_configuration() -> Iterator[None]:
    def clean() -> None:
        with SessionLocal() as db:
            db.execute(delete(TrackUserTag))
            db.execute(delete(AssistantModelEvaluation))
            db.execute(
                delete(BackgroundJob).where(
                    BackgroundJob.kind.in_(
                        [TAG_CLEANUP_QUALITY_JOB_KIND, MODEL_TAG_CLEANUP_JOB_KIND]
                    )
                )
            )
            db.execute(delete(AssistantModelRole))
            db.execute(delete(AssistantProviderConnection))
            db.execute(delete(AssistantTagVocabulary))
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
    "detective work": "investigation",
    "quiet sleep": "rest",
    "sad": "melancholy",
    "love theme": "romantic",
    "clue hunting": "investigation",
    "camp recovery": "rest",
    "wistful grief": "melancholy",
    "tender love": "romantic",
}


def _reference_cleanup_model(
    _target: object,
    request: StructuredModelRequest,
) -> StructuredModelResult:
    assert "Example JSON shape" in request.system_prompt
    payload = json.loads(request.user_prompt)
    target_id_by_name = {
        item["name"]: item["tag_id"] for item in payload["canonical_tags"]
    }
    assert payload["remaining_suggestion_slots"] <= 100
    decisions = [
        {
            "source_id": source["source_id"],
            "target_tag_id": (
                target_id_by_name[target]
                if (target := _REFERENCE_MERGES.get(source["tag"])) is not None
                else None
            ),
            "confidence": "high",
            "reason": "Synthetic reference synonym or spelling match",
        }
        for source in payload["candidate_sources"]
    ]
    return StructuredModelResult(
        True,
        None,
        {
            "schema_version": MODEL_TAG_CLEANUP_OUTPUT_CONTRACT,
            "decisions": decisions,
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
            "api_key": TEST_CLEANUP_API_KEY,
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
            ("structured-text/v1",),
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


def _configure_quality_passed_cleanup_role(
    client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _configure_enabled_cleanup_role(client, monkeypatch)
    monkeypatch.setattr(
        "app.assistant.model_evaluation.execute_structured_model_request",
        _reference_cleanup_model,
    )
    started = client.post(
        "/api/assistant/providers/roles/tag_cleanup/"
        "evaluations/tag-cleanup-quality-v1/jobs"
    )
    assert started.status_code == 202, started.text
    finished = _wait_for_job(client, started.json()["id"], {"succeeded"})
    assert finished["result"]["evaluation"]["passed"] is True


def test_cleanup_model_rejects_unknown_targets_and_wrong_source_order() -> None:
    usage = [
        TagUsage(tag="ale room", track_count=3),
        TagUsage(tag="road music", track_count=2),
    ]

    def execute_with(decisions: list[dict[str, object]]) -> StructuredModelResult:
        return StructuredModelResult(
            True,
            None,
            {
                "schema_version": MODEL_TAG_CLEANUP_OUTPUT_CONTRACT,
                "decisions": decisions,
            },
        )

    with pytest.raises(ModelTagCleanupError, match="model_output_unknown_target"):
        suggest_model_tag_cleanup(
            usage,
            lambda _request: execute_with(
                [
                    {
                        "source_id": "source-001",
                        "target_tag_id": "mood.invented-tag",
                        "confidence": "high",
                        "reason": "Not allowed",
                    },
                    {
                        "source_id": "source-002",
                        "target_tag_id": None,
                        "confidence": "low",
                        "reason": "No safe match",
                    },
                ]
            ),
        )

    with pytest.raises(
        ModelTagCleanupError,
        match="model_output_source_set_mismatch",
    ):
        suggest_model_tag_cleanup(
            usage,
            lambda _request: execute_with(
                [
                    {
                        "source_id": "source-002",
                        "target_tag_id": None,
                        "confidence": "high",
                        "reason": "Second source first",
                    },
                    {
                        "source_id": "source-001",
                        "target_tag_id": None,
                        "confidence": "high",
                        "reason": "First source second",
                    },
                ]
            ),
        )


def test_cleanup_model_reports_safe_schema_diagnostic() -> None:
    usage = [TagUsage(tag="ale room", track_count=3)]

    with pytest.raises(ModelTagCleanupError) as invalid:
        suggest_model_tag_cleanup(
            usage,
            lambda _request: StructuredModelResult(
                True,
                None,
                {
                    "schema_version": MODEL_TAG_CLEANUP_OUTPUT_CONTRACT,
                    "decisions": [
                        {
                            "source_id": "source-001",
                            "target_tag_id": "setting.tavern",
                            "reason": "Clear synonym",
                        }
                    ],
                },
            ),
        )

    assert invalid.value.code == "model_output_schema_invalid"
    assert invalid.value.diagnostic is not None
    assert "decisions.0.confidence" in invalid.value.diagnostic


def test_cleanup_model_bounds_incidental_reason_text() -> None:
    suggestions = suggest_model_tag_cleanup(
        [TagUsage(tag="ale room", track_count=3)],
        lambda _request: StructuredModelResult(
            True,
            None,
            {
                "schema_version": MODEL_TAG_CLEANUP_OUTPUT_CONTRACT,
                "decisions": [
                    {
                        "source_id": "source-001",
                        "target_tag_id": "setting.tavern",
                        "confidence": "high",
                        "reason": "r" * 700,
                    }
                ],
            },
        ),
    )

    assert len(suggestions) == 1
    assert len(suggestions[0].reason) == 512
    assert suggestions[0].reason.endswith("...")


def test_cleanup_resolves_unambiguous_sources_without_provider_call() -> None:
    def should_not_run(_request: StructuredModelRequest) -> StructuredModelResult:
        raise AssertionError("deterministic cleanup must not call the provider")

    suggestions = suggest_model_tag_cleanup(
        [
            TagUsage(tag="medival", track_count=3),
            TagUsage(tag="medieval", track_count=12),
            TagUsage(tag="taverns", track_count=2),
        ],
        should_not_run,
    )

    assert {(item.source, item.target) for item in suggestions} == {
        ("medival", "medieval"),
        ("taverns", "tavern"),
    }
    assert all(item.confidence == "high" for item in suggestions)


def test_cleanup_uses_operator_aliases_without_a_provider_call() -> None:
    document = TagVocabularyDocument(
        groups=[
            TagVocabularyGroup(
                key="mood",
                label="Mood",
                description="Operator-defined emotional tones.",
                tags=[
                    TagVocabularyEntry(
                        id="mood.dreamlike",
                        name="dreamlike",
                        description="Soft, unreal, and oneiric atmosphere.",
                        aliases=["oneiric"],
                    )
                ],
            )
        ]
    )
    vocabulary = TagVocabularySnapshot(
        document=document,
        revision=3,
        fingerprint=vocabulary_fingerprint(document),
    )

    def should_not_run(_request: StructuredModelRequest) -> StructuredModelResult:
        raise AssertionError("declared vocabulary aliases must resolve locally")

    suggestions = suggest_model_tag_cleanup(
        [TagUsage(tag="oneiric", track_count=4)],
        should_not_run,
        vocabulary,
    )

    assert [(item.source, item.target) for item in suggestions] == [
        ("oneiric", "dreamlike")
    ]


def test_cleanup_model_receives_only_remaining_result_capacity() -> None:
    observed_slots: list[int] = []

    def execute(request: StructuredModelRequest) -> StructuredModelResult:
        payload = json.loads(request.user_prompt)
        observed_slots.append(payload["remaining_suggestion_slots"])
        assert request.output_schema is not None
        definitions = request.output_schema["$defs"]
        assert isinstance(definitions, dict)
        decision = definitions["ModelTagCleanupDecision"]
        assert isinstance(decision, dict)
        properties = decision["properties"]
        assert isinstance(properties, dict)
        source_schema = properties["source_id"]
        assert isinstance(source_schema, dict)
        assert source_schema["enum"] == ["source-001"]
        target_schema = properties["target_tag_id"]
        assert isinstance(target_schema, dict)
        target_choices = target_schema["anyOf"]
        assert isinstance(target_choices, list)
        target_ids = next(
            item["enum"]
            for item in target_choices
            if isinstance(item, dict) and item.get("type") == "string"
        )
        assert "setting.tavern" in target_ids
        return StructuredModelResult(
            True,
            None,
            {
                "schema_version": MODEL_TAG_CLEANUP_OUTPUT_CONTRACT,
                "decisions": [
                    {
                        "source_id": "source-001",
                        "target_tag_id": None,
                        "confidence": "low",
                        "reason": "No safe match",
                    }
                ],
            },
        )

    suggest_model_tag_cleanup(
        [
            TagUsage(tag="medival", track_count=3),
            TagUsage(tag="ale room", track_count=2),
        ],
        execute,
    )

    assert observed_slots == [99]


def test_reference_cleanup_model_passes_fixed_quality_suite() -> None:
    suite = load_tag_cleanup_quality_suite(_SUITE_PATH)
    result = evaluate_model_tag_cleanup(
        lambda request: _reference_cleanup_model(object(), request),
        suite,
    )

    assert result.passed is True
    assert result.passed_cases == result.total_cases == 12


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
    assert finished["progress_current"] == 12
    assert finished["progress_total"] == 12
    assert finished["result"]["evaluation"]["passed"] is True
    assert finished["result"]["usage"] == {
        "schema_version": "assistant-provider-usage/v1",
        "attempted_requests": 3,
        "input_tokens": 180,
        "output_tokens": 45,
        "input_tokens_reported_requests": 3,
        "output_tokens_reported_requests": 3,
        "provider_model_ids": ["cleanup-response-model"],
        "provider_model_ids_truncated": False,
    }
    assert TEST_CLEANUP_API_KEY not in json.dumps(finished)

    evaluations = auth_client.get(
        "/api/assistant/providers/roles/tag_cleanup/evaluations"
    )
    assert evaluations.status_code == 200, evaluations.text
    assert evaluations.json()[0]["status"] == "passed"
    assert evaluations.json()[0]["suite_id"] == (
        "controlled-vocabulary-cleanup-baseline-v3"
    )


def test_model_tag_cleanup_job_discloses_catalog_only_and_applies_selection(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
    seeded_track_id: int,
) -> None:
    _configure_quality_passed_cleanup_role(auth_client, monkeypatch)
    response = auth_client.patch(
        f"/api/assistant/library-tags/{seeded_track_id}",
        json={"add": ["inn", "tavern"], "remove": []},
    )
    assert response.status_code == 200, response.text

    status = auth_client.get(
        "/api/assistant/library-tags/catalog/model-cleanup-status"
    )
    assert status.status_code == 200, status.text
    assert status.json()["available"] is True
    assert status.json()["manual_tags"] == 2
    assert status.json()["estimated_provider_requests"] == 0
    assert status.json()["disclosure"] == {
        "version": "assistant-model-tag-cleanup-disclosure/v3",
        "shared_with_provider": [
            "Manual source tags not already resolved by deterministic cleanup rules",
            "The number of tracks using each shared source tag",
            (
                "The operator-managed canonical tag IDs, names, groups, and "
                "definitions; the model may return only those IDs or no match"
            ),
        ],
        "never_shared": [
            "Audio or media files",
            "Track titles, artists, albums, metadata, or filesystem paths",
            "Playlists, generated tags, review history, or provider credentials",
        ],
        "maximum_tags": 500,
        "may_incur_cost": True,
    }

    monkeypatch.setattr(
        "app.assistant.model_tag_cleanup_job.execute_structured_model_request",
        _reference_cleanup_model,
    )
    started = auth_client.post(
        "/api/assistant/library-tags/catalog/model-cleanup-jobs",
        json={
            "disclosure_version": "assistant-model-tag-cleanup-disclosure/v3",
            "consent": True,
        },
    )
    assert started.status_code == 202, started.text
    finished = _wait_for_job(auth_client, started.json()["id"], {"succeeded"})
    assert finished["kind"] == MODEL_TAG_CLEANUP_JOB_KIND
    assert finished["progress_current"] == finished["progress_total"] == 1
    assert finished["result"]["suggestions"] == [
        {
            "id": finished["result"]["suggestions"][0]["id"],
            "source": "inn",
            "target": "tavern",
            "origin": "local-rule",
            "confidence": "high",
            "reason": "Matches an alias defined in the controlled vocabulary.",
            "source_track_count": 1,
            "target_track_count": 1,
            "merged": True,
        }
    ]
    assert len(finished["result"]["suggestions"][0]["id"]) == 64
    assert finished["result"]["usage"]["attempted_requests"] == 0
    assert "inn" not in json.dumps(finished["parameters"])
    assert "tavern" not in json.dumps(finished["parameters"])
    assert TEST_CLEANUP_API_KEY not in json.dumps(finished)

    applied = auth_client.post(
        "/api/assistant/library-tags/catalog/model-cleanup-apply",
        json={
            "job_id": finished["id"],
            "catalog_signature": finished["result"]["catalog_signature"],
            "vocabulary_fingerprint": finished["result"][
                "vocabulary_fingerprint"
            ],
            "items": [{"source": "inn", "target": "tavern"}],
        },
    )
    assert applied.status_code == 200, applied.text
    assert applied.json()["applied"] == [
        {
            "source": "inn",
            "target": "tavern",
            "affected_tracks": 1,
            "merged": True,
        }
    ]
    catalog = auth_client.get("/api/assistant/library-tags/catalog").json()
    assert catalog["tag_usage"] == [{"tag": "tavern", "track_count": 1}]


def test_model_tag_cleanup_apply_rejects_stale_or_invented_selection(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
    seeded_track_id: int,
) -> None:
    _configure_quality_passed_cleanup_role(auth_client, monkeypatch)
    response = auth_client.patch(
        f"/api/assistant/library-tags/{seeded_track_id}",
        json={"add": ["inn"], "remove": []},
    )
    assert response.status_code == 200, response.text
    monkeypatch.setattr(
        "app.assistant.model_tag_cleanup_job.execute_structured_model_request",
        _reference_cleanup_model,
    )
    started = auth_client.post(
        "/api/assistant/library-tags/catalog/model-cleanup-jobs",
        json={
            "disclosure_version": "assistant-model-tag-cleanup-disclosure/v3",
            "consent": True,
        },
    )
    finished = _wait_for_job(auth_client, started.json()["id"], {"succeeded"})

    invented = auth_client.post(
        "/api/assistant/library-tags/catalog/model-cleanup-apply",
        json={
            "job_id": finished["id"],
            "catalog_signature": finished["result"]["catalog_signature"],
            "vocabulary_fingerprint": finished["result"][
                "vocabulary_fingerprint"
            ],
            "items": [{"source": "inn", "target": "combat"}],
        },
    )
    assert invented.status_code == 422, invented.text
    assert invented.json()["detail"]["code"] == "tag_cleanup_invalid_selection"

    changed = auth_client.patch(
        f"/api/assistant/library-tags/{seeded_track_id}",
        json={"add": ["tavern"], "remove": []},
    )
    assert changed.status_code == 200, changed.text
    stale = auth_client.post(
        "/api/assistant/library-tags/catalog/model-cleanup-apply",
        json={
            "job_id": finished["id"],
            "catalog_signature": finished["result"]["catalog_signature"],
            "vocabulary_fingerprint": finished["result"][
                "vocabulary_fingerprint"
            ],
            "items": [{"source": "inn", "target": "tavern"}],
        },
    )
    assert stale.status_code == 409, stale.text
    assert stale.json()["detail"]["code"] == "tag_cleanup_stale"
    catalog = auth_client.get("/api/assistant/library-tags/catalog").json()
    assert catalog["used_tags"] == ["inn", "tavern"]


def test_model_tag_cleanup_apply_rejects_changed_vocabulary(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
    seeded_track_id: int,
) -> None:
    _configure_quality_passed_cleanup_role(auth_client, monkeypatch)
    added = auth_client.patch(
        f"/api/assistant/library-tags/{seeded_track_id}",
        json={"add": ["inn"], "remove": []},
    )
    assert added.status_code == 200, added.text
    started = auth_client.post(
        "/api/assistant/library-tags/catalog/model-cleanup-jobs",
        json={
            "disclosure_version": "assistant-model-tag-cleanup-disclosure/v3",
            "consent": True,
        },
    )
    finished = _wait_for_job(auth_client, started.json()["id"], {"succeeded"})

    vocabulary = auth_client.get("/api/assistant/library-tags/vocabulary").json()
    vocabulary["groups"][0]["description"] = "Changed setting definitions."
    saved = auth_client.put(
        "/api/assistant/library-tags/vocabulary",
        json={
            "schema_version": vocabulary["schema_version"],
            "expected_revision": vocabulary["revision"],
            "groups": vocabulary["groups"],
        },
    )
    assert saved.status_code == 200, saved.text

    stale = auth_client.post(
        "/api/assistant/library-tags/catalog/model-cleanup-apply",
        json={
            "job_id": finished["id"],
            "catalog_signature": finished["result"]["catalog_signature"],
            "vocabulary_fingerprint": finished["result"][
                "vocabulary_fingerprint"
            ],
            "items": [{"source": "inn", "target": "tavern"}],
        },
    )

    assert stale.status_code == 409, stale.text
    assert stale.json()["detail"]["code"] == "tag_cleanup_stale"
