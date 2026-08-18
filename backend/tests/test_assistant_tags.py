from __future__ import annotations

from collections.abc import Iterator

import pytest
from fastapi.testclient import TestClient
from sqlalchemy import delete, select


@pytest.fixture(autouse=True)
def _isolate_tag_state(client: TestClient) -> Iterator[None]:
    from app.core.db import SessionLocal
    from app.models.track_analysis import TrackAnalysis
    from app.models.track_user_tag import TrackUserTag

    with SessionLocal() as db:
        db.execute(delete(TrackUserTag))
        db.execute(delete(TrackAnalysis))
        db.commit()
    yield
    with SessionLocal() as db:
        db.execute(delete(TrackUserTag))
        db.execute(delete(TrackAnalysis))
        db.commit()


def test_manual_tag_endpoints_require_auth(client: TestClient) -> None:
    assert client.get("/api/assistant/library-tags").status_code == 401
    assert client.get("/api/assistant/library-tags/catalog").status_code == 401
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


def test_manual_and_analysis_tags_remain_separate(
    auth_client: TestClient,
    seeded_track_id: int,
) -> None:
    from app.assistant.analysis import (
        LOCAL_METADATA_ANALYZER_ID,
        track_source_signature,
    )
    from app.core.db import SessionLocal
    from app.models.track import Track
    from app.models.track_analysis import TrackAnalysis

    with SessionLocal() as db:
        track = db.scalar(select(Track).where(Track.id == seeded_track_id))
        assert track is not None
        db.add(
            TrackAnalysis(
                track_id=track.id,
                analyzer_id=LOCAL_METADATA_ANALYZER_ID,
                source_signature=track_source_signature(track),
                job_id="a" * 32,
                energy=0.4,
                brightness=0.2,
                tension=0.8,
                moods_json='["dark", "tense"]',
                evidence_json='["metadata"]',
                confidence="medium",
            )
        )
        db.commit()

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
