from __future__ import annotations

from collections.abc import Iterator

import pytest
from fastapi.testclient import TestClient
from sqlalchemy import delete


@pytest.fixture(autouse=True)
def _isolate_tag_state(client: TestClient) -> Iterator[None]:
    from app.core.db import SessionLocal
    from app.models.assistant_tag_vocabulary import AssistantTagVocabulary
    from app.models.track_analysis import TrackAnalysis
    from app.models.track_analysis_tag_review import TrackAnalysisTagReview
    from app.models.track_user_tag import TrackUserTag

    with SessionLocal() as db:
        db.execute(delete(TrackAnalysisTagReview))
        db.execute(delete(TrackUserTag))
        db.execute(delete(TrackAnalysis))
        db.execute(delete(AssistantTagVocabulary))
        db.commit()
    yield
    with SessionLocal() as db:
        db.execute(delete(TrackAnalysisTagReview))
        db.execute(delete(TrackUserTag))
        db.execute(delete(TrackAnalysis))
        db.execute(delete(AssistantTagVocabulary))
        db.commit()


def _seed_analysis(track_id: int, moods_json: str = '["dark", "tense"]') -> str:
    from app.assistant.analysis import (
        LOCAL_METADATA_ANALYZER_ID,
        track_source_signature,
    )
    from app.core.db import SessionLocal
    from app.models.track import Track
    from app.models.track_analysis import TrackAnalysis

    with SessionLocal() as db:
        track = db.get(Track, track_id)
        assert track is not None
        signature = track_source_signature(track)
        db.add(
            TrackAnalysis(
                track_id=track.id,
                analyzer_id=LOCAL_METADATA_ANALYZER_ID,
                source_signature=signature,
                job_id="a" * 32,
                energy=0.4,
                brightness=0.2,
                tension=0.8,
                moods_json=moods_json,
                evidence_json='["Mood metadata: dark, tense"]',
                confidence="medium",
            )
        )
        db.commit()
    return signature


def test_default_vocabulary_gives_every_tag_bounded_context_cues() -> None:
    from app.assistant.tag_vocabulary import default_tag_vocabulary_snapshot

    vocabulary = default_tag_vocabulary_snapshot()

    assert vocabulary.entries
    assert all(tag.context_cues for tag in vocabulary.entries)
    assert all(len(tag.context_cues) <= 8 for tag in vocabulary.entries)
    assert all(len(tag.context_cues) == len(set(tag.context_cues)) for tag in vocabulary.entries)


def test_manual_tag_endpoints_require_auth(client: TestClient) -> None:
    assert client.get("/api/assistant/library-tags").status_code == 401
    assert client.get("/api/assistant/library-tags/catalog").status_code == 401
    assert client.get("/api/assistant/library-tags/vocabulary").status_code == 401
    assert (
        client.get("/api/assistant/library-tags/catalog/cleanup-preview").status_code
        == 401
    )
    assert (
        client.get(
            "/api/assistant/library-tags/catalog/model-cleanup-status"
        ).status_code
        == 401
    )
    assert (
        client.post(
            "/api/assistant/library-tags/catalog/model-cleanup-jobs",
            json={
                "disclosure_version": (
                    "assistant-model-tag-cleanup-disclosure/v3"
                ),
                "consent": True,
            },
        ).status_code
        == 401
    )
    assert (
        client.post(
            "/api/assistant/library-tags/catalog/model-cleanup-apply",
            json={
                "job_id": "a" * 32,
                "catalog_signature": "0" * 64,
                "vocabulary_fingerprint": "0" * 64,
                "items": [{"source": "inn", "target": "tavern"}],
            },
        ).status_code
        == 401
    )
    assert (
        client.post(
            "/api/assistant/library-tags/catalog/cleanup-apply",
            json={
                "catalog_signature": "0" * 64,
                "vocabulary_fingerprint": "0" * 64,
                "items": [],
            },
        ).status_code
        == 401
    )
    assert (
        client.post(
            "/api/assistant/library-tags/bulk",
            json={"track_ids": [1], "add": ["tavern"], "remove": []},
        ).status_code
        == 401
    )
    assert (
        client.post(
            "/api/assistant/library-tags/catalog/rename",
            json={"source": "tavern", "target": "inn"},
        ).status_code
        == 401
    )
    assert (
        client.put(
            "/api/assistant/library-tags/1/analysis-tags/review",
            json={
                "tag": "dark",
                "analyzer_id": "local-metadata/v1",
                "source_signature": "a" * 64,
                "decision": "accepted",
            },
        ).status_code
        == 401
    )
    assert (
        client.post(
            "/api/assistant/library-tags/analysis-tags/reviews/bulk",
            json={
                "items": [
                    {
                        "track_id": 1,
                        "tag": "dark",
                        "analyzer_id": "local-metadata/v1",
                        "source_signature": "a" * 64,
                    }
                ],
                "decision": "accepted",
            },
        ).status_code
        == 401
    )
    assert (
        client.patch(
            "/api/assistant/library-tags/1",
            json={"add": ["tavern"], "remove": []},
        ).status_code
        == 401
    )


def test_catalog_has_dnd_starters_and_tracks_custom_usage(
    auth_client: TestClient,
    seeded_track_id: int,
) -> None:
    response = auth_client.get("/api/assistant/library-tags/catalog")
    assert response.status_code == 200, response.text
    starters = {
        tag
        for group in response.json()["starter_groups"]
        for tag in group["tags"]
    }
    assert {"medieval", "dancing", "tavern"} <= starters

    update = auth_client.patch(
        f"/api/assistant/library-tags/{seeded_track_id}",
        json={"add": ["  Tavern ", "MEDIEVAL", "custom scene"], "remove": []},
    )
    assert update.status_code == 200, update.text
    assert update.json()["manual_tags"] == ["custom scene", "medieval", "tavern"]

    refreshed = auth_client.get("/api/assistant/library-tags/catalog")
    assert refreshed.status_code == 200, refreshed.text
    assert refreshed.json()["used_tags"] == ["custom scene", "medieval", "tavern"]
    assert refreshed.json()["tag_usage"] == [
        {"tag": "custom scene", "track_count": 1},
        {"tag": "medieval", "track_count": 1},
        {"tag": "tavern", "track_count": 1},
    ]


def test_tag_vocabulary_is_editable_revisioned_and_drives_catalog(
    auth_client: TestClient,
) -> None:
    loaded = auth_client.get("/api/assistant/library-tags/vocabulary")
    assert loaded.status_code == 200, loaded.text
    original = loaded.json()
    assert original["schema_version"] == "assistant-tag-vocabulary/v1"
    assert original["revision"] == 1
    assert len(original["fingerprint"]) == 64

    groups = original["groups"]
    mood_group = next(group for group in groups if group["key"] == "mood")
    mood_group["tags"].append(
        {
            "id": "mood.poignant",
            "name": "poignant",
            "description": "Deeply affecting, emotionally sharp, or moving.",
            "aliases": ["emotionally affecting"],
            "context_cues": ["farewell"],
        }
    )
    saved = auth_client.put(
        "/api/assistant/library-tags/vocabulary",
        json={
            "schema_version": original["schema_version"],
            "expected_revision": original["revision"],
            "groups": groups,
        },
    )
    assert saved.status_code == 200, saved.text
    assert saved.json()["revision"] == 2
    assert saved.json()["fingerprint"] != original["fingerprint"]

    catalog = auth_client.get("/api/assistant/library-tags/catalog")
    assert catalog.status_code == 200, catalog.text
    mood_tags = next(
        group["tags"]
        for group in catalog.json()["starter_groups"]
        if group["key"] == "mood"
    )
    assert "poignant" in mood_tags

    conflict = auth_client.put(
        "/api/assistant/library-tags/vocabulary",
        json={
            "schema_version": original["schema_version"],
            "expected_revision": original["revision"],
            "groups": groups,
        },
    )
    assert conflict.status_code == 409


def test_expanded_vocabulary_seed_merges_once_without_resetting_custom_tags(
    auth_client: TestClient,
) -> None:
    import json

    from app.core.db import SessionLocal
    from app.models.assistant_tag_vocabulary import AssistantTagVocabulary

    legacy_document = {
        "schema_version": "assistant-tag-vocabulary/v1",
        "groups": [
            {
                "key": "mood",
                "label": "My moods",
                "description": "Operator-curated choices.",
                "tags": [
                    {
                        "id": "mood.calm",
                        "name": "calm",
                        "description": "My carefully tuned calm definition.",
                        "aliases": ["serene"],
                    },
                    {
                        "id": "mood.personal-favorite",
                        "name": "personal favorite",
                        "description": "A private operator-defined mood label.",
                        "aliases": [],
                    }
                ],
            }
        ],
    }
    with SessionLocal() as db:
        db.add(
            AssistantTagVocabulary(
                key="library",
                revision=7,
                seed_version=1,
                document_json=json.dumps(legacy_document),
            )
        )
        db.commit()

    migrated = auth_client.get("/api/assistant/library-tags/vocabulary")
    repeated = auth_client.get("/api/assistant/library-tags/vocabulary")

    assert migrated.status_code == 200, migrated.text
    assert migrated.json()["revision"] == 8
    assert repeated.json()["revision"] == 8
    names = {
        tag["name"]
        for group in migrated.json()["groups"]
        for tag in group["tags"]
    }
    assert {"personal favorite", "desert", "combat", "calm"} <= names
    custom_group = next(
        group for group in migrated.json()["groups"] if group["key"] == "mood"
    )
    assert custom_group["label"] == "My moods"
    calm = next(tag for tag in custom_group["tags"] if tag["id"] == "mood.calm")
    assert calm["description"] == "My carefully tuned calm definition."
    assert calm["aliases"] == ["serene"]
    assert {"quiet", "lullaby", "rest"} <= set(calm["context_cues"])


def test_manual_and_analysis_tags_remain_separate(
    auth_client: TestClient,
    seeded_track_id: int,
) -> None:
    from app.assistant.analysis import (
        LOCAL_METADATA_ANALYZER_ID,
    )

    signature = _seed_analysis(seeded_track_id)

    update = auth_client.patch(
        f"/api/assistant/library-tags/{seeded_track_id}",
        json={"add": ["tavern", "dancing"], "remove": []},
    )
    assert update.status_code == 200, update.text
    payload = update.json()
    assert payload["manual_tags"] == ["dancing", "tavern"]
    assert payload["analysis_analyzer"] == LOCAL_METADATA_ANALYZER_ID
    assert payload["analysis_tags"] == ["dark", "tense"]
    assert payload["analysis_confidence"] == "medium"
    assert payload["analysis_suggestions"] == [
        {
            "tag": tag,
            "analyzer_id": LOCAL_METADATA_ANALYZER_ID,
            "source_signature": signature,
            "confidence": "medium",
            "evidence": ["Mood metadata: dark, tense"],
            "status": "pending",
        }
        for tag in ("dark", "tense")
    ]
    assert payload["audio_signal"] is None

    listing = auth_client.get(
        "/api/assistant/library-tags",
        params={"tag": "TAVERN", "search": "test-song"},
    )
    assert listing.status_code == 200, listing.text
    assert listing.json()["total"] == 1
    assert listing.json()["items"][0]["manual_tags"] == ["dancing", "tavern"]
    assert listing.json()["items"][0]["analysis_tags"] == ["dark", "tense"]

    changed = auth_client.patch(
        f"/api/assistant/library-tags/{seeded_track_id}",
        json={"add": ["feast"], "remove": ["dancing"]},
    )
    assert changed.status_code == 200, changed.text
    assert changed.json()["manual_tags"] == ["feast", "tavern"]


def test_library_tag_detail_exposes_current_audio_evidence_read_only(
    auth_client: TestClient,
    seeded_track_id: int,
) -> None:
    from app.assistant.audio_analysis import (
        LOCAL_AUDIO_ANALYZER_ID,
        audio_source_signature,
    )
    from app.core.db import SessionLocal
    from app.models.track import Track
    from app.models.track_analysis import TrackAnalysis

    with SessionLocal() as db:
        track = db.get(Track, seeded_track_id)
        assert track is not None
        db.add(
            TrackAnalysis(
                track_id=track.id,
                analyzer_id=LOCAL_AUDIO_ANALYZER_ID,
                source_signature=audio_source_signature(track),
                job_id="b" * 32,
                energy=0.5,
                brightness=0.25,
                tension=0.4,
                moods_json="[]",
                evidence_json='["Measured signal evidence","No mood claim"]',
                metrics_json=(
                    '{"schema":"local-audio/v1","rms_dbfs":-18.0,'
                    '"tempo_bpm":120.0}'
                ),
                confidence="medium",
            )
        )
        db.commit()

    listing = auth_client.get(
        "/api/assistant/library-tags",
        params={"search": "test-song"},
    )
    assert listing.status_code == 200, listing.text
    profile = listing.json()["items"][0]["audio_signal"]
    assert profile == {
        "analyzer_id": LOCAL_AUDIO_ANALYZER_ID,
        "confidence": "medium",
        "evidence": ["Measured signal evidence", "No mood claim"],
        "metrics": {
            "schema": LOCAL_AUDIO_ANALYZER_ID,
            "rms_dbfs": -18.0,
            "tempo_bpm": 120.0,
        },
    }
def test_analysis_tag_reviews_are_durable_and_keep_manual_tags_independent(
    auth_client: TestClient,
    seeded_track_id: int,
) -> None:
    from app.assistant.analysis import LOCAL_METADATA_ANALYZER_ID
    from app.core.db import SessionLocal
    from app.models.track_analysis import TrackAnalysis

    signature = _seed_analysis(seeded_track_id)
    endpoint = f"/api/assistant/library-tags/{seeded_track_id}/analysis-tags/review"
    base = {
        "analyzer_id": LOCAL_METADATA_ANALYZER_ID,
        "source_signature": signature,
    }

    accepted = auth_client.put(
        endpoint,
        json={**base, "tag": "DARK", "decision": "accepted"},
    )
    rejected = auth_client.put(
        endpoint,
        json={**base, "tag": "tense", "decision": "rejected"},
    )
    assert accepted.status_code == 200, accepted.text
    assert accepted.json()["manual_tags"] == ["dark"]
    assert accepted.json()["decision"] == "accepted"
    assert rejected.status_code == 200, rejected.text
    assert rejected.json()["manual_tags"] == ["dark"]

    listing = auth_client.get("/api/assistant/library-tags")
    listed_track = next(
        item
        for item in listing.json()["items"]
        if item["track_id"] == seeded_track_id
    )
    suggestions = listed_track["analysis_suggestions"]
    assert {item["tag"]: item["status"] for item in suggestions} == {
        "dark": "accepted",
        "tense": "rejected",
    }
    playlist = auth_client.post(
        "/api/assistant/playlists/suggest",
        json={"prompt": "dark tense", "candidate_limit": 10},
    )
    assert playlist.status_code == 200, playlist.text
    candidate = next(
        item
        for item in playlist.json()["candidates"]
        if item["track_id"] == seeded_track_id
    )
    assert candidate["manual_tags"] == ["dark"]
    assert candidate["analysis_tags"] == ["dark"]

    reopened = auth_client.put(
        endpoint,
        json={**base, "tag": "dark", "decision": "pending"},
    )
    assert reopened.status_code == 200, reopened.text
    assert reopened.json()["manual_tags"] == ["dark"]

    refreshed = auth_client.get("/api/assistant/library-tags")
    refreshed_track = next(
        item
        for item in refreshed.json()["items"]
        if item["track_id"] == seeded_track_id
    )
    refreshed_suggestions = refreshed_track["analysis_suggestions"]
    assert {item["tag"]: item["status"] for item in refreshed_suggestions} == {
        "dark": "pending",
        "tense": "rejected",
    }
    with SessionLocal() as db:
        analysis = db.get(
            TrackAnalysis,
            (seeded_track_id, LOCAL_METADATA_ANALYZER_ID),
        )
        assert analysis is not None
        assert analysis.moods_json == '["dark", "tense"]'


def test_analysis_tag_review_rejects_stale_profiles(
    auth_client: TestClient,
    seeded_track_id: int,
) -> None:
    from app.assistant.analysis import LOCAL_METADATA_ANALYZER_ID
    from app.core.db import SessionLocal
    from app.models.track import Track

    signature = _seed_analysis(seeded_track_id)
    with SessionLocal() as db:
        track = db.get(Track, seeded_track_id)
        assert track is not None
        original_genre = track.genre
        track.genre = "changed after analysis"
        db.commit()

    try:
        response = auth_client.put(
            f"/api/assistant/library-tags/{seeded_track_id}/analysis-tags/review",
            json={
                "tag": "dark",
                "analyzer_id": LOCAL_METADATA_ANALYZER_ID,
                "source_signature": signature,
                "decision": "accepted",
            },
        )
    finally:
        with SessionLocal() as db:
            track = db.get(Track, seeded_track_id)
            assert track is not None
            track.genre = original_genre
            db.commit()
    assert response.status_code == 409


def test_failed_analysis_tag_acceptance_records_no_decision(
    auth_client: TestClient,
    seeded_track_id: int,
) -> None:
    from app.assistant.analysis import LOCAL_METADATA_ANALYZER_ID
    from app.core.db import SessionLocal
    from app.models.track_analysis_tag_review import TrackAnalysisTagReview

    signature = _seed_analysis(seeded_track_id)
    full = auth_client.patch(
        f"/api/assistant/library-tags/{seeded_track_id}",
        json={"add": [f"tag-{index}" for index in range(32)], "remove": []},
    )
    assert full.status_code == 200, full.text

    response = auth_client.put(
        f"/api/assistant/library-tags/{seeded_track_id}/analysis-tags/review",
        json={
            "tag": "dark",
            "analyzer_id": LOCAL_METADATA_ANALYZER_ID,
            "source_signature": signature,
            "decision": "accepted",
        },
    )
    assert response.status_code == 422
    with SessionLocal() as db:
        assert db.get(
            TrackAnalysisTagReview,
            (seeded_track_id, LOCAL_METADATA_ANALYZER_ID, "dark"),
        ) is None


def test_review_filter_returns_tracks_with_matching_current_decisions(
    auth_client: TestClient,
    seeded_track_id: int,
    extra_seeded_track_ids: list[int],
) -> None:
    from app.assistant.analysis import LOCAL_METADATA_ANALYZER_ID

    accepted_id, rejected_id, pending_id = [
        seeded_track_id,
        *extra_seeded_track_ids[:2],
    ]
    signatures = {
        track_id: _seed_analysis(track_id, '["dark"]')
        for track_id in (accepted_id, rejected_id, pending_id)
    }
    endpoint = "/api/assistant/library-tags/{}/analysis-tags/review"
    for track_id, decision in (
        (accepted_id, "accepted"),
        (rejected_id, "rejected"),
    ):
        response = auth_client.put(
            endpoint.format(track_id),
            json={
                "tag": "dark",
                "analyzer_id": LOCAL_METADATA_ANALYZER_ID,
                "source_signature": signatures[track_id],
                "decision": decision,
            },
        )
        assert response.status_code == 200, response.text

    expected = {
        "pending": pending_id,
        "accepted": accepted_id,
        "rejected": rejected_id,
    }
    for status, track_id in expected.items():
        listing = auth_client.get(
            "/api/assistant/library-tags",
            params={"review": status},
        )
        assert listing.status_code == 200, listing.text
        assert listing.json()["total"] == 1
        assert [item["track_id"] for item in listing.json()["items"]] == [track_id]

    invalid = auth_client.get(
        "/api/assistant/library-tags",
        params={"review": "unknown"},
    )
    assert invalid.status_code == 422


def test_bulk_review_applies_valid_items_and_reports_each_invalid_item(
    auth_client: TestClient,
    seeded_track_id: int,
    extra_seeded_track_ids: list[int],
) -> None:
    from app.assistant.analysis import LOCAL_METADATA_ANALYZER_ID

    other_id = extra_seeded_track_ids[0]
    seeded_signature = _seed_analysis(seeded_track_id)
    other_signature = _seed_analysis(other_id)
    items = [
        {
            "track_id": seeded_track_id,
            "tag": "dark",
            "analyzer_id": LOCAL_METADATA_ANALYZER_ID,
            "source_signature": seeded_signature,
        },
        {
            "track_id": other_id,
            "tag": "tense",
            "analyzer_id": LOCAL_METADATA_ANALYZER_ID,
            "source_signature": other_signature,
        },
        {
            "track_id": seeded_track_id,
            "tag": "tense",
            "analyzer_id": LOCAL_METADATA_ANALYZER_ID,
            "source_signature": "stale-signature",
        },
        {
            "track_id": 999999,
            "tag": "dark",
            "analyzer_id": LOCAL_METADATA_ANALYZER_ID,
            "source_signature": "missing-track",
        },
    ]
    response = auth_client.post(
        "/api/assistant/library-tags/analysis-tags/reviews/bulk",
        json={"items": items, "decision": "accepted"},
    )

    assert response.status_code == 200, response.text
    payload = response.json()
    assert payload["requested_items"] == 4
    assert {
        (item["track_id"], item["tag"], item["decision"])
        for item in payload["applied"]
    } == {
        (seeded_track_id, "dark", "accepted"),
        (other_id, "tense", "accepted"),
    }
    assert {
        (item["track_id"], item["tag"], item["code"])
        for item in payload["failures"]
    } == {
        (seeded_track_id, "tense", "stale"),
        (999999, "dark", "not_found"),
    }

    seeded = auth_client.get(
        "/api/assistant/library-tags",
        params={"search": "test-song"},
    )
    assert seeded.json()["items"][0]["manual_tags"] == ["dark"]


def test_bulk_accept_skips_all_new_tags_for_a_track_that_would_overflow(
    auth_client: TestClient,
    seeded_track_id: int,
    extra_seeded_track_ids: list[int],
) -> None:
    from app.assistant.analysis import LOCAL_METADATA_ANALYZER_ID

    other_id = extra_seeded_track_ids[0]
    seeded_signature = _seed_analysis(seeded_track_id)
    other_signature = _seed_analysis(other_id, '["dark"]')
    full = auth_client.patch(
        f"/api/assistant/library-tags/{seeded_track_id}",
        json={"add": [f"tag-{index}" for index in range(31)], "remove": []},
    )
    assert full.status_code == 200, full.text

    response = auth_client.post(
        "/api/assistant/library-tags/analysis-tags/reviews/bulk",
        json={
            "items": [
                {
                    "track_id": seeded_track_id,
                    "tag": tag,
                    "analyzer_id": LOCAL_METADATA_ANALYZER_ID,
                    "source_signature": seeded_signature,
                }
                for tag in ("dark", "tense")
            ]
            + [
                {
                    "track_id": other_id,
                    "tag": "dark",
                    "analyzer_id": LOCAL_METADATA_ANALYZER_ID,
                    "source_signature": other_signature,
                }
            ],
            "decision": "accepted",
        },
    )

    assert response.status_code == 200, response.text
    payload = response.json()
    assert [(item["track_id"], item["tag"]) for item in payload["applied"]] == [
        (other_id, "dark")
    ]
    assert {
        (item["track_id"], item["tag"], item["code"])
        for item in payload["failures"]
    } == {
        (seeded_track_id, "dark", "tag_limit"),
        (seeded_track_id, "tense", "tag_limit"),
    }
    listing = auth_client.get(
        "/api/assistant/library-tags",
        params={"search": "test-song"},
    )
    item = listing.json()["items"][0]
    assert "dark" not in item["manual_tags"]
    assert "tense" not in item["manual_tags"]
    assert {suggestion["status"] for suggestion in item["analysis_suggestions"]} == {
        "pending"
    }


def test_manual_tag_patch_validates_conflicts_and_missing_tracks(
    auth_client: TestClient,
    seeded_track_id: int,
) -> None:
    conflict = auth_client.patch(
        f"/api/assistant/library-tags/{seeded_track_id}",
        json={"add": ["tavern"], "remove": ["TAVERN"]},
    )
    missing = auth_client.patch(
        "/api/assistant/library-tags/999999",
        json={"add": ["tavern"], "remove": []},
    )

    assert conflict.status_code == 422
    assert missing.status_code == 404


def test_playlist_endpoint_prioritizes_operator_tags(
    auth_client: TestClient,
    extra_seeded_track_ids: list[int],
) -> None:
    tagged_track_id = extra_seeded_track_ids[-1]
    update = auth_client.patch(
        f"/api/assistant/library-tags/{tagged_track_id}",
        json={"add": ["medieval", "tavern", "dancing"], "remove": []},
    )
    assert update.status_code == 200, update.text

    suggestion = auth_client.post(
        "/api/assistant/playlists/suggest",
        json={"prompt": "medieval tavern dancing", "candidate_limit": 10},
    )
    assert suggestion.status_code == 200, suggestion.text
    first = suggestion.json()["candidates"][0]
    assert first["track_id"] == tagged_track_id
    assert first["manual_tags"] == ["dancing", "medieval", "tavern"]
    assert first["reasons"][0].startswith("Your tags:")


def test_bulk_tagging_updates_valid_tracks_and_reports_missing_ones(
    auth_client: TestClient,
    seeded_track_id: int,
    extra_seeded_track_ids: list[int],
) -> None:
    track_ids = [seeded_track_id, extra_seeded_track_ids[0], 999999]
    response = auth_client.post(
        "/api/assistant/library-tags/bulk",
        json={
            "track_ids": track_ids,
            "add": ["tavern", "medieval"],
            "remove": [],
        },
    )

    assert response.status_code == 200, response.text
    payload = response.json()
    assert payload["requested_tracks"] == 3
    assert payload["matched_tracks"] == 2
    assert payload["changed_track_ids"] == sorted(track_ids[:2])
    assert payload["missing_track_ids"] == [999999]
    assert payload["failures"] == []

    listing = auth_client.get(
        "/api/assistant/library-tags",
        params={"tag": "tavern", "limit": 100},
    )
    assert listing.status_code == 200, listing.text
    assert listing.json()["total"] == 2


def test_bulk_tagging_skips_only_tracks_that_would_exceed_limit(
    auth_client: TestClient,
    seeded_track_id: int,
    extra_seeded_track_ids: list[int],
) -> None:
    full = [f"tag-{index}" for index in range(32)]
    seeded = auth_client.patch(
        f"/api/assistant/library-tags/{seeded_track_id}",
        json={"add": full, "remove": []},
    )
    assert seeded.status_code == 200, seeded.text

    other_id = extra_seeded_track_ids[0]
    response = auth_client.post(
        "/api/assistant/library-tags/bulk",
        json={
            "track_ids": [seeded_track_id, other_id],
            "add": ["overflow"],
            "remove": [],
        },
    )

    assert response.status_code == 200, response.text
    payload = response.json()
    assert payload["changed_track_ids"] == [other_id]
    assert payload["failures"] == [
        {
            "track_id": seeded_track_id,
            "error": "track would exceed the 32-tag limit",
        }
    ]


def test_rename_merges_existing_tag_and_updates_usage_counts(
    auth_client: TestClient,
    seeded_track_id: int,
    extra_seeded_track_ids: list[int],
) -> None:
    first_extra, second_extra = extra_seeded_track_ids[:2]
    for track_id, tags in (
        (seeded_track_id, ["tavern", "medieval"]),
        (first_extra, ["tavern"]),
        (second_extra, ["medieval"]),
    ):
        response = auth_client.patch(
            f"/api/assistant/library-tags/{track_id}",
            json={"add": tags, "remove": []},
        )
        assert response.status_code == 200, response.text

    renamed = auth_client.post(
        "/api/assistant/library-tags/catalog/rename",
        json={"source": "TAVERN", "target": "medieval"},
    )
    assert renamed.status_code == 200, renamed.text
    assert renamed.json() == {
        "source": "tavern",
        "target": "medieval",
        "affected_tracks": 2,
        "merged": True,
    }

    catalog = auth_client.get("/api/assistant/library-tags/catalog")
    assert catalog.status_code == 200, catalog.text
    assert catalog.json()["used_tags"] == ["medieval"]
    assert catalog.json()["tag_usage"] == [
        {"tag": "medieval", "track_count": 3}
    ]

    missing = auth_client.post(
        "/api/assistant/library-tags/catalog/rename",
        json={"source": "tavern", "target": "inn"},
    )
    assert missing.status_code == 404


def test_tag_cleanup_preview_is_conservative_and_read_only(
    auth_client: TestClient,
    seeded_track_id: int,
    extra_seeded_track_ids: list[int],
) -> None:
    first_extra, second_extra = extra_seeded_track_ids[:2]
    for track_id, tags in (
        (seeded_track_id, ["medival", "ambient", "quiet"]),
        (first_extra, ["taverns"]),
        (second_extra, ["medieval"]),
    ):
        response = auth_client.patch(
            f"/api/assistant/library-tags/{track_id}",
            json={"add": tags, "remove": []},
        )
        assert response.status_code == 200, response.text

    preview = auth_client.get(
        "/api/assistant/library-tags/catalog/cleanup-preview"
    )
    assert preview.status_code == 200, preview.text
    payload = preview.json()
    assert payload["schema_version"] == "assistant-tag-cleanup-preview/v2"
    assert len(payload["catalog_signature"]) == 64
    assert len(payload["vocabulary_fingerprint"]) == 64
    assert [
        (item["source"], item["target"], item["reason_code"], item["merged"])
        for item in payload["suggestions"]
    ] == [
        ("medival", "medieval", "vocabulary_typo", True),
        ("taverns", "tavern", "vocabulary_plural", False),
    ]

    catalog = auth_client.get("/api/assistant/library-tags/catalog").json()
    assert catalog["used_tags"] == [
        "ambient",
        "medieval",
        "medival",
        "quiet",
        "taverns",
    ]


def test_tag_cleanup_applies_only_explicit_selection_atomically(
    auth_client: TestClient,
    seeded_track_id: int,
    extra_seeded_track_ids: list[int],
) -> None:
    other_id = extra_seeded_track_ids[0]
    for track_id, tag in ((seeded_track_id, "medival"), (other_id, "taverns")):
        response = auth_client.patch(
            f"/api/assistant/library-tags/{track_id}",
            json={"add": [tag], "remove": []},
        )
        assert response.status_code == 200, response.text

    preview = auth_client.get(
        "/api/assistant/library-tags/catalog/cleanup-preview"
    ).json()
    applied = auth_client.post(
        "/api/assistant/library-tags/catalog/cleanup-apply",
        json={
            "catalog_signature": preview["catalog_signature"],
            "vocabulary_fingerprint": preview["vocabulary_fingerprint"],
            "items": [{"source": "medival", "target": "medieval"}],
        },
    )
    assert applied.status_code == 200, applied.text
    assert applied.json()["applied"] == [
        {
            "source": "medival",
            "target": "medieval",
            "affected_tracks": 1,
            "merged": False,
        }
    ]

    catalog = auth_client.get("/api/assistant/library-tags/catalog").json()
    assert catalog["used_tags"] == ["medieval", "taverns"]


def test_tag_cleanup_rejects_stale_preview_without_partial_changes(
    auth_client: TestClient,
    seeded_track_id: int,
    extra_seeded_track_ids: list[int],
) -> None:
    response = auth_client.patch(
        f"/api/assistant/library-tags/{seeded_track_id}",
        json={"add": ["medival"], "remove": []},
    )
    assert response.status_code == 200, response.text
    preview = auth_client.get(
        "/api/assistant/library-tags/catalog/cleanup-preview"
    ).json()

    changed = auth_client.patch(
        f"/api/assistant/library-tags/{extra_seeded_track_ids[0]}",
        json={"add": ["tavern"], "remove": []},
    )
    assert changed.status_code == 200, changed.text
    stale = auth_client.post(
        "/api/assistant/library-tags/catalog/cleanup-apply",
        json={
            "catalog_signature": preview["catalog_signature"],
            "vocabulary_fingerprint": preview["vocabulary_fingerprint"],
            "items": [{"source": "medival", "target": "medieval"}],
        },
    )
    assert stale.status_code == 409, stale.text
    assert stale.json()["detail"]["code"] == "tag_cleanup_stale"

    catalog = auth_client.get("/api/assistant/library-tags/catalog").json()
    assert catalog["used_tags"] == ["medival", "tavern"]


def test_tag_cleanup_rejects_invented_selection_before_valid_rename(
    auth_client: TestClient,
    seeded_track_id: int,
    extra_seeded_track_ids: list[int],
) -> None:
    other_id = extra_seeded_track_ids[0]
    for track_id, tags in (
        (seeded_track_id, ["medival"]),
        (other_id, ["ambient"]),
    ):
        response = auth_client.patch(
            f"/api/assistant/library-tags/{track_id}",
            json={"add": tags, "remove": []},
        )
        assert response.status_code == 200, response.text
    preview = auth_client.get(
        "/api/assistant/library-tags/catalog/cleanup-preview"
    ).json()

    invalid = auth_client.post(
        "/api/assistant/library-tags/catalog/cleanup-apply",
        json={
            "catalog_signature": preview["catalog_signature"],
            "vocabulary_fingerprint": preview["vocabulary_fingerprint"],
            "items": [
                {"source": "medival", "target": "medieval"},
                {"source": "ambient", "target": "tavern"},
            ],
        },
    )
    assert invalid.status_code == 422, invalid.text
    assert invalid.json()["detail"]["code"] == "tag_cleanup_invalid_selection"

    catalog = auth_client.get("/api/assistant/library-tags/catalog").json()
    assert catalog["used_tags"] == ["ambient", "medival"]
