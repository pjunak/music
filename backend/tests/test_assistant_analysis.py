from __future__ import annotations

import time
from collections.abc import Iterator
from typing import Any

import pytest
from fastapi.testclient import TestClient
from sqlalchemy import delete, select


@pytest.fixture(autouse=True)
def _isolate_analysis_state(client: TestClient) -> Iterator[None]:
    """Keep shared-session library metadata and profiles independent per test."""

    from app.core.db import SessionLocal
    from app.models.track import Track
    from app.models.track_analysis import TrackAnalysis

    with SessionLocal() as db:
        original_genres = {
            track.id: track.genre for track in db.scalars(select(Track)).all()
        }
        db.execute(delete(TrackAnalysis))
        db.commit()

    yield

    with SessionLocal() as db:
        db.execute(delete(TrackAnalysis))
        for track in db.scalars(select(Track)).all():
            if track.id in original_genres:
                track.genre = original_genres[track.id]
        db.commit()


def _wait_for_job(
    client: TestClient,
    job_id: str,
    *,
    timeout: float = 3.0,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    latest: dict[str, Any] = {}
    while time.monotonic() < deadline:
        response = client.get(f"/api/jobs/{job_id}")
        assert response.status_code == 200, response.text
        latest = response.json()
        if latest["status"] in {"succeeded", "failed", "cancelled"}:
            return latest
        time.sleep(0.02)
    raise AssertionError(f"analysis job did not finish; latest={latest}")


def _start_and_wait(client: TestClient, *, force: bool = False) -> dict[str, Any]:
    response = client.post(
        "/api/assistant/library-analysis/jobs",
        json={"force": force},
    )
    assert response.status_code == 202, response.text
    return _wait_for_job(client, response.json()["id"])


def test_library_analysis_requires_auth(client: TestClient) -> None:
    assert client.get("/api/assistant/library-analysis/summary").status_code == 401
    assert (
        client.post(
            "/api/assistant/library-analysis/jobs",
            json={"force": False},
        ).status_code
        == 401
    )


def test_analysis_profiles_library_and_reuses_current_results(
    auth_client: TestClient,
    seeded_track_id: int,
) -> None:
    from app.assistant.analysis import LOCAL_METADATA_ANALYZER_ID
    from app.core.db import SessionLocal
    from app.models.track_analysis import TrackAnalysis

    first = _start_and_wait(auth_client)
    assert first["status"] == "succeeded", first
    library_tracks = first["result"]["tracks"]
    assert first["progress_current"] == first["progress_total"] == library_tracks
    assert first["result"] == {
        "tracks": library_tracks,
        "updated": library_tracks,
        "unchanged": 0,
        "current_profiles": library_tracks,
        "analyzer": LOCAL_METADATA_ANALYZER_ID,
    }

    with SessionLocal() as db:
        profile = db.get(
            TrackAnalysis,
            (seeded_track_id, LOCAL_METADATA_ANALYZER_ID),
        )
        assert profile is not None
        assert profile.analyzer_id == LOCAL_METADATA_ANALYZER_ID
        assert 0.0 <= profile.energy <= 1.0
        assert profile.confidence in {"high", "medium", "low"}
        db.add(
            TrackAnalysis(
                track_id=seeded_track_id,
                analyzer_id="future-audio/v1",
                source_signature="future-signal-signature",
                job_id="future-job",
                energy=0.7,
                brightness=0.4,
                tension=0.6,
                moods_json='["cinematic"]',
                evidence_json='["audio signal"]',
                confidence="medium",
            )
        )
        db.commit()

    unchanged = _start_and_wait(auth_client)
    assert unchanged["status"] == "succeeded", unchanged
    assert unchanged["result"]["updated"] == 0
    assert unchanged["result"]["unchanged"] == library_tracks

    summary = auth_client.get("/api/assistant/library-analysis/summary")
    assert summary.status_code == 200, summary.text
    payload = summary.json()
    assert payload["library_tracks"] == library_tracks
    assert payload["analyzed_tracks"] == library_tracks
    assert (
        payload["high_confidence"]
        + payload["medium_confidence"]
        + payload["low_confidence"]
        == library_tracks
    )
    assert payload["last_updated_at"] is not None

    with SessionLocal() as db:
        assert db.get(TrackAnalysis, (seeded_track_id, "future-audio/v1")) is not None


def test_analysis_refreshes_changed_metadata_and_force_rebuilds(
    auth_client: TestClient,
    seeded_track_id: int,
) -> None:
    from app.core.db import SessionLocal
    from app.models.track import Track

    initial = _start_and_wait(auth_client)
    assert initial["status"] == "succeeded", initial

    with SessionLocal() as db:
        track = db.scalar(select(Track).where(Track.id == seeded_track_id))
        assert track is not None
        track.genre = "ambient"
        db.commit()

    refreshed = _start_and_wait(auth_client)
    assert refreshed["status"] == "succeeded", refreshed
    assert refreshed["result"]["updated"] == 1

    forced = _start_and_wait(auth_client, force=True)
    assert forced["status"] == "succeeded", forced
    assert forced["result"]["updated"] == forced["result"]["tracks"]
