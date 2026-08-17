"""Versioned JSON Authoring import validation, review, and atomic commit."""
from __future__ import annotations

from copy import deepcopy

import pytest
from fastapi.testclient import TestClient


def _create_target(auth_client: TestClient, mode_id: str) -> None:
    response = auth_client.post(
        "/api/modes",
        json={"id": mode_id, "name": "Document Target"},
    )
    assert response.status_code == 201, response.text


def _document(track_path: str) -> dict:
    return {
        "schema": "authoring-import/v1",
        "name": "Assistant draft",
        "playlists": [
            {
                "name": "Night Walk",
                "category": "exploration",
                "tracks": [track_path, "missing/not-in-library.flac"],
            }
        ],
        "soundboards": [
            {
                "id": "storms",
                "name": "Storms",
                "categories": [
                    {
                        "id": "weather",
                        "name": "Weather",
                        "items": [
                            {
                                "file": "dnd/door.ogg",
                                "name": "Thunder",
                            }
                        ],
                    }
                ],
            }
        ],
        "interrupts": [{"name": "Ambush", "playlist": "Night Walk"}],
        "presets": [
            {
                "id": "dark-hall",
                "name": "Dark Hall",
                "effects": [{"type": "reverb", "wet": 0.35}],
            }
        ],
        "cues": [
            {
                "id": "arrival",
                "name": "Arrival",
                "preset": "dark-hall",
                "playlist": "Night Walk",
                "sfx": [
                    {
                        "soundboard": "storms",
                        "item": "dnd/door.ogg",
                        "volume": 0.7,
                    }
                ],
            }
        ],
    }


def _preview_document(
    auth_client: TestClient,
    target_id: str,
    document: dict,
    *,
    source_name: str = "authoring-draft.json",
) -> dict:
    response = auth_client.post(
        "/api/authoring/import/document/preview",
        json={
            "target_mode_id": target_id,
            "source_name": source_name,
            "document": document,
        },
    )
    assert response.status_code == 200, response.text
    return response.json()


def _selection(item: dict) -> dict[str, str]:
    return {"kind": item["kind"], "resource_id": item["resource_id"]}


def test_document_import_requires_auth(client: TestClient) -> None:
    response = client.post(
        "/api/authoring/import/document/preview",
        json={
            "target_mode_id": "dnd",
            "document": {
                "schema": "authoring-import/v1",
                "presets": [{"id": "plain", "name": "Plain"}],
            },
        },
    )
    assert response.status_code == 401
    assert client.get("/api/authoring/import/document/schema").status_code == 401


def test_document_schema_is_available_to_authenticated_authoring_tools(
    auth_client: TestClient,
) -> None:
    response = auth_client.get("/api/authoring/import/document/schema")
    assert response.status_code == 200
    schema = response.json()
    assert schema["properties"]["schema"]["const"] == "authoring-import/v1"
    assert schema["additionalProperties"] is False


def test_document_preview_and_commit_all_resource_kinds(
    auth_client: TestClient,
    seeded_track_id: int,
) -> None:
    target_id = "document-import-all"
    _create_target(auth_client, target_id)
    track_path = auth_client.get(
        f"/api/library/tracks/{seeded_track_id}"
    ).json()["path"]
    document = _document(track_path)

    preview = _preview_document(auth_client, target_id, document)
    assert preview["source"] == {
        "type": "document",
        "id": "authoring-import/v1",
        "name": "Assistant draft",
    }
    assert preview["target_mode"] == {
        "id": target_id,
        "name": "Document Target",
    }
    assert {item["kind"] for item in preview["items"]} == {
        "playlist",
        "soundboard",
        "interrupt",
        "preset",
        "cue",
    }
    assert all(item["status"] == "ready" for item in preview["items"])

    playlist = next(item for item in preview["items"] if item["kind"] == "playlist")
    assert {issue["code"] for issue in playlist["issues"]} == {"missing_tracks"}
    cue = next(item for item in preview["items"] if item["kind"] == "cue")
    assert {
        issue["related_item"]["kind"]
        for issue in cue["issues"]
        if issue["code"] == "dependency_selection_required"
    } == {"playlist", "soundboard", "preset"}

    # Preview is read-only.
    assert auth_client.get(
        "/api/playlists", params={"mode_id": target_id}
    ).json() == []

    response = auth_client.post(
        "/api/authoring/import/document/commit",
        json={
            "target_mode_id": target_id,
            "source_name": "authoring-draft.json",
            "document": document,
            "items": [_selection(item) for item in preview["items"]],
        },
    )
    assert response.status_code == 200, response.text
    result = response.json()
    assert len(result["imported"]) == 5
    assert result["skipped"] == []
    assert result["missing_track_paths"] == ["missing/not-in-library.flac"]

    target = auth_client.get(f"/api/modes/{target_id}").json()
    assert target["playlist_categories"] == ["exploration"]
    assert set(target["soundboards"]) == {"storms"}
    assert set(target["presets"]) == {"dark-hall"}
    assert set(target["cues"]) == {"arrival"}
    assert [item["name"] for item in target["interrupts"]] == ["Ambush"]
    playlists = auth_client.get(
        "/api/playlists", params={"mode_id": target_id}
    ).json()
    tracks = auth_client.get(
        f"/api/playlists/{playlists[0]['id']}/tracks"
    ).json()
    assert [track["track_id"] for track in tracks] == [seeded_track_id]


def test_document_commit_rejects_unselected_dependency_atomically(
    auth_client: TestClient,
    seeded_track_id: int,
) -> None:
    target_id = "document-import-dependency"
    _create_target(auth_client, target_id)
    track_path = auth_client.get(
        f"/api/library/tracks/{seeded_track_id}"
    ).json()["path"]
    document = {
        "schema": "authoring-import/v1",
        "playlists": [{"name": "Night Walk", "tracks": [track_path]}],
        "cues": [
            {"id": "arrival", "name": "Arrival", "playlist": "Night Walk"}
        ],
    }
    preview = _preview_document(auth_client, target_id, document)
    cue = next(item for item in preview["items"] if item["kind"] == "cue")

    response = auth_client.post(
        "/api/authoring/import/document/commit",
        json={
            "target_mode_id": target_id,
            "document": document,
            "items": [_selection(cue)],
        },
    )
    assert response.status_code == 400
    assert "requires playlist" in response.json()["detail"]
    assert auth_client.get(
        "/api/playlists", params={"mode_id": target_id}
    ).json() == []
    assert auth_client.get(f"/api/modes/{target_id}").json()["cues"] == {}


@pytest.mark.parametrize(
    "mutation",
    [
        lambda document: document.update(schema="authoring-import/v2"),
        lambda document: document.update(unexpected=True),
        lambda document: document.clear(),
        lambda document: document.update(
            playlists=[
                {"name": "Duplicate", "tracks": []},
                {"name": "Duplicate", "tracks": []},
            ]
        ),
    ],
)
def test_document_structure_is_strictly_validated(
    auth_client: TestClient,
    mutation,
) -> None:
    document = {
        "schema": "authoring-import/v1",
        "presets": [{"id": "plain", "name": "Plain"}],
    }
    mutation(document)
    response = auth_client.post(
        "/api/authoring/import/document/preview",
        json={"target_mode_id": "dnd", "document": document},
    )
    assert response.status_code == 422


def test_document_semantic_errors_are_reported_per_item(
    auth_client: TestClient,
) -> None:
    target_id = "document-import-invalid"
    _create_target(auth_client, target_id)
    document = {
        "schema": "authoring-import/v1",
        "playlists": [{"name": "Unsafe", "tracks": ["../outside.mp3"]}],
        "presets": [
            {
                "id": "unsupported",
                "name": "Unsupported",
                "effects": [{"type": "pitch_shift", "semitones": -3}],
            }
        ],
        "cues": [
            {"id": "orphan", "name": "Orphan", "playlist": "Not present"}
        ],
    }
    preview = _preview_document(auth_client, target_id, document)
    assert {item["status"] for item in preview["items"]} == {"invalid"}
    issue_codes = {
        item["kind"]: {issue["code"] for issue in item["issues"]}
        for item in preview["items"]
    }
    assert "invalid_path" in issue_codes["playlist"]
    assert "unsupported_effect" in issue_codes["preset"]
    assert "missing_dependency" in issue_codes["cue"]


def test_document_commit_replans_target_conflicts_without_overwriting(
    auth_client: TestClient,
) -> None:
    target_id = "document-import-conflict"
    _create_target(auth_client, target_id)
    document = {
        "schema": "authoring-import/v1",
        "presets": [{"id": "plain", "name": "Imported name"}],
    }
    preview = _preview_document(auth_client, target_id, document)
    selected = _selection(preview["items"][0])
    created = auth_client.post(
        f"/api/modes/{target_id}/presets",
        json={"id": "plain", "name": "Existing name", "effects": []},
    )
    assert created.status_code == 201, created.text

    response = auth_client.post(
        "/api/authoring/import/document/commit",
        json={
            "target_mode_id": target_id,
            "document": deepcopy(document),
            "items": [selected],
        },
    )
    assert response.status_code == 200, response.text
    assert response.json()["imported"] == []
    assert response.json()["skipped"][0]["status"] == "conflict"
    target = auth_client.get(f"/api/modes/{target_id}").json()
    assert target["presets"]["plain"]["name"] == "Existing name"
