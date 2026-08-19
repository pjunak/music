from collections.abc import Iterator

import pytest
from fastapi.testclient import TestClient
from sqlalchemy import create_engine, delete, inspect

from app.assistant.analysis import LOCAL_METADATA_ANALYZER_ID, track_source_signature
from app.core.db import SessionLocal
from app.main import _apply_additive_columns
from app.models.playlist import Playlist
from app.models.track import Track
from app.models.track_analysis import TrackAnalysis
from app.models.track_user_tag import TrackUserTag


@pytest.fixture(autouse=True)
def _clean_automatic_playlist_data() -> Iterator[None]:
    def clean() -> None:
        with SessionLocal() as db:
            db.execute(delete(Playlist))
            db.execute(delete(Track).where(Track.path.like("Automatic/%")))
            db.commit()

    clean()
    yield
    clean()


def _add_track(
    path: str,
    title: str,
    *,
    bpm: int | None,
    tags: tuple[str, ...],
) -> int:
    with SessionLocal() as db:
        track = Track(
            path=path,
            title=title,
            artist="Automatic Fixtures",
            album="Rule Tests",
            genre="",
            length_s=180,
            bpm=bpm,
            size_bytes=100,
            mtime=1,
        )
        db.add(track)
        db.flush()
        db.add_all(TrackUserTag(track_id=track.id, tag=tag) for tag in tags)
        db.commit()
        return track.id


def _create_playlist(client: TestClient) -> int:
    response = client.post(
        "/api/playlists",
        json={"name": "Living Tavern", "mode_id": "dnd"},
    )
    assert response.status_code == 201, response.text
    assert response.json()["automatic"] is False
    return int(response.json()["id"])


def _rule(**overrides: object) -> dict[str, object]:
    rule: dict[str, object] = {
        "schema": "automatic-playlist/v1",
        "include_tags": ["tavern"],
        "match": "any",
        "exclude_tags": [],
        "tag_sources": "manual",
        "min_bpm": None,
        "max_bpm": None,
        "include_unknown_bpm": True,
        "maximum_tracks": 200,
        "order_by": "title",
    }
    rule.update(overrides)
    return rule


def test_rule_preview_is_read_only_and_explainable(auth_client: TestClient) -> None:
    _add_track(
        "Automatic/dance.wav",
        "Tavern Dance",
        bpm=120,
        tags=("tavern", "dancing"),
    )
    _add_track(
        "Automatic/rest.wav",
        "Quiet Inn",
        bpm=80,
        tags=("tavern", "calm"),
    )
    _add_track(
        "Automatic/battle.wav",
        "Battle",
        bpm=140,
        tags=("combat",),
    )
    playlist_id = _create_playlist(auth_client)

    preview = auth_client.post(
        f"/api/playlists/{playlist_id}/automatic/preview",
        json={"rule": _rule(include_tags=["tavern", "dancing"], match="all")},
    )
    tracks = auth_client.get(f"/api/playlists/{playlist_id}/tracks")

    assert preview.status_code == 200, preview.text
    payload = preview.json()
    assert payload["schema_version"] == "automatic-playlist-preview/v1"
    assert payload["matched_tracks"] == 1
    assert [track["title"] for track in payload["tracks"]] == ["Tavern Dance"]
    assert payload["tracks"][0]["bpm"] == 120
    assert tracks.status_code == 200
    assert tracks.json() == []


def test_configuration_rejects_a_stale_preview(auth_client: TestClient) -> None:
    track_id = _add_track(
        "Automatic/song.wav",
        "Unlabelled Song",
        bpm=100,
        tags=(),
    )
    playlist_id = _create_playlist(auth_client)
    preview = auth_client.post(
        f"/api/playlists/{playlist_id}/automatic/preview",
        json={"rule": _rule()},
    ).json()
    with SessionLocal() as db:
        db.add(TrackUserTag(track_id=track_id, tag="tavern"))
        db.commit()

    configured = auth_client.put(
        f"/api/playlists/{playlist_id}/automatic",
        json={"rule": _rule(), "source_signature": preview["source_signature"]},
    )

    assert configured.status_code == 409
    assert configured.json()["detail"]["code"] == "automatic_playlist_preview_stale"
    assert auth_client.get(f"/api/playlists/{playlist_id}").json()["automatic"] is False


def test_automatic_playlist_refreshes_on_read_and_keeps_normal_playback_rows(
    auth_client: TestClient,
) -> None:
    first_id = _add_track(
        "Automatic/first.wav",
        "First Tavern",
        bpm=90,
        tags=("tavern",),
    )
    playlist_id = _create_playlist(auth_client)
    preview = auth_client.post(
        f"/api/playlists/{playlist_id}/automatic/preview",
        json={"rule": _rule()},
    ).json()
    configured = auth_client.put(
        f"/api/playlists/{playlist_id}/automatic",
        json={"rule": _rule(), "source_signature": preview["source_signature"]},
    )
    assert configured.status_code == 200, configured.text
    assert configured.json()["playlist"]["automatic"] is True
    assert configured.json()["playlist"]["automatic_rule"]["schema"] == (
        "automatic-playlist/v1"
    )
    assert configured.json()["materialized_tracks"] == 1

    second_id = _add_track(
        "Automatic/second.wav",
        "Second Tavern",
        bpm=110,
        tags=("tavern",),
    )
    refreshed = auth_client.get(f"/api/playlists/{playlist_id}/tracks")

    assert refreshed.status_code == 200, refreshed.text
    assert [item["track_id"] for item in refreshed.json()] == [first_id, second_id]
    assert [item["position"] for item in refreshed.json()] == [0, 1]


def test_automatic_items_are_locked_until_playlist_is_made_manual(
    auth_client: TestClient,
) -> None:
    track_id = _add_track(
        "Automatic/song.wav",
        "Tavern Song",
        bpm=100,
        tags=("tavern",),
    )
    playlist_id = _create_playlist(auth_client)
    preview = auth_client.post(
        f"/api/playlists/{playlist_id}/automatic/preview",
        json={"rule": _rule()},
    ).json()
    assert auth_client.put(
        f"/api/playlists/{playlist_id}/automatic",
        json={"rule": _rule(), "source_signature": preview["source_signature"]},
    ).status_code == 200

    blocked_add = auth_client.post(
        f"/api/playlists/{playlist_id}/tracks",
        json={"track_id": track_id},
    )
    blocked_remove = auth_client.delete(f"/api/playlists/{playlist_id}/tracks/0")
    disabled = auth_client.delete(f"/api/playlists/{playlist_id}/automatic")
    added = auth_client.post(
        f"/api/playlists/{playlist_id}/tracks",
        json={"track_id": track_id},
    )

    assert blocked_add.status_code == 409
    assert blocked_add.json()["detail"]["code"] == "automatic_playlist_items_managed"
    assert blocked_remove.status_code == 409
    assert disabled.status_code == 200
    assert disabled.json()["automatic"] is False
    assert added.status_code == 201


def test_rule_validation_and_bounded_ordering(auth_client: TestClient) -> None:
    _add_track(
        "Automatic/slow.wav",
        "Slow",
        bpm=80,
        tags=("tavern",),
    )
    _add_track(
        "Automatic/fast.wav",
        "Fast",
        bpm=130,
        tags=("tavern",),
    )
    playlist_id = _create_playlist(auth_client)

    invalid = auth_client.post(
        f"/api/playlists/{playlist_id}/automatic/preview",
        json={
            "rule": _rule(
                include_tags=["tavern"],
                exclude_tags=["TAVERN"],
            )
        },
    )
    ordered = auth_client.post(
        f"/api/playlists/{playlist_id}/automatic/preview",
        json={
            "rule": _rule(
                min_bpm=90,
                maximum_tracks=1,
                order_by="bpm_descending",
            )
        },
    )

    assert invalid.status_code == 422
    assert ordered.status_code == 200
    assert [track["title"] for track in ordered.json()["tracks"]] == ["Fast"]


def test_rule_can_include_current_local_analysis_without_model_tags(
    auth_client: TestClient,
) -> None:
    track_id = _add_track(
        "Automatic/local.wav",
        "Locally Analysed Inn",
        bpm=95,
        tags=(),
    )
    with SessionLocal() as db:
        track = db.get(Track, track_id)
        assert track is not None
        db.add(
            TrackAnalysis(
                track_id=track.id,
                analyzer_id=LOCAL_METADATA_ANALYZER_ID,
                source_signature=track_source_signature(track),
                job_id="automatic-test",
                energy=0.4,
                brightness=0.3,
                tension=0.2,
                moods_json='["automatic-local-fixture"]',
                evidence_json='["synthetic fixture"]',
                confidence="high",
            )
        )
        db.commit()
    playlist_id = _create_playlist(auth_client)

    manual_only = auth_client.post(
        f"/api/playlists/{playlist_id}/automatic/preview",
        json={
            "rule": _rule(
                include_tags=["automatic-local-fixture"],
                tag_sources="manual",
            )
        },
    )
    local = auth_client.post(
        f"/api/playlists/{playlist_id}/automatic/preview",
        json={
            "rule": _rule(
                include_tags=["automatic-local-fixture"],
                tag_sources="manual_and_local",
            )
        },
    )

    assert manual_only.status_code == 200
    assert manual_only.json()["matched_tracks"] == 0
    assert local.status_code == 200
    assert [track["id"] for track in local.json()["tracks"]] == [track_id]


def test_additive_upgrade_creates_automatic_columns(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    legacy_engine = create_engine(f"sqlite:///{tmp_path / 'legacy.db'}")
    with legacy_engine.begin() as connection:
        connection.exec_driver_sql(
            "CREATE TABLE playlists (id INTEGER PRIMARY KEY, name VARCHAR(256))"
        )
    monkeypatch.setattr("app.main.engine", legacy_engine)

    _apply_additive_columns()

    columns = {column["name"] for column in inspect(legacy_engine).get_columns("playlists")}
    assert {
        "automatic_rule_json",
        "automatic_source_signature",
        "automatic_refreshed_at",
    }.issubset(columns)
    legacy_engine.dispose()


def test_automatic_endpoints_require_authentication(client: TestClient) -> None:
    assert client.post(
        "/api/playlists/1/automatic/preview",
        json={"rule": _rule()},
    ).status_code == 401
    assert client.put(
        "/api/playlists/1/automatic",
        json={"rule": _rule(), "source_signature": "0" * 64},
    ).status_code == 401
    assert client.delete("/api/playlists/1/automatic").status_code == 401
