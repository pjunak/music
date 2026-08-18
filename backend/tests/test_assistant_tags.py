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
