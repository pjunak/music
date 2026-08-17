"""Cross-mode Authoring import preview, commit, conflicts, and rollback."""
from __future__ import annotations

import os
from pathlib import Path

import yaml
from fastapi.testclient import TestClient


def _seed_import_pair(
    auth_client: TestClient,
    seeded_track_id: int,
    *,
    source_id: str,
    target_id: str,
) -> None:
    assert auth_client.post(
        "/api/modes", json={"id": source_id, "name": "Source Mode"}
    ).status_code == 201
    assert auth_client.post(
        "/api/modes", json={"id": target_id, "name": "Target Mode"}
    ).status_code == 201

    playlist = auth_client.post(
        "/api/playlists",
        json={
            "name": "Night Walk",
            "mode_id": source_id,
            "category": "exploration",
        },
    )
    assert playlist.status_code == 201
    playlist_id = playlist.json()["id"]
    for _ in range(2):
        assert auth_client.post(
            f"/api/playlists/{playlist_id}/tracks",
            json={"track_id": seeded_track_id},
        ).status_code == 201

    assert auth_client.post(
        f"/api/modes/{source_id}/soundboards",
        json={"id": "storms", "name": "Storms"},
    ).status_code == 201
    assert auth_client.post(
        f"/api/modes/{source_id}/soundboards/storms/categories",
        json={"id": "weather", "name": "Weather"},
    ).status_code == 201
    assert auth_client.post(
        f"/api/modes/{source_id}/soundboards/storms/categories/weather/items",
        json={"file": "dnd/door.ogg", "name": "Thunder"},
    ).status_code == 201

    assert auth_client.post(
        f"/api/modes/{source_id}/presets",
        json={
            "id": "dark-hall",
            "name": "Dark Hall",
            "effects": [{"type": "reverb", "wet": 0.35}],
        },
    ).status_code == 201
    assert auth_client.post(
        f"/api/modes/{source_id}/interrupts",
        json={"name": "Ambush", "playlist": "Night Walk"},
    ).status_code == 201
    assert auth_client.post(
        f"/api/modes/{source_id}/cues",
        json={
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
        },
    ).status_code == 201


def _preview(
    auth_client: TestClient, source_id: str, target_id: str
) -> dict:
    response = auth_client.post(
        "/api/authoring/import/preview",
        json={"source_mode_id": source_id, "target_mode_id": target_id},
    )
    assert response.status_code == 200, response.text
    return response.json()


def _all_selections(preview: dict) -> list[dict[str, str]]:
    return [
        {"kind": item["kind"], "resource_id": item["resource_id"]}
        for item in preview["items"]
    ]


def test_authoring_import_requires_auth(client: TestClient) -> None:
    response = client.post(
        "/api/authoring/import/preview",
        json={"source_mode_id": "dnd", "target_mode_id": "other"},
    )
    assert response.status_code == 401


def test_preview_is_read_only_and_describes_every_authoring_kind(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    source_id = "import-preview-source"
    target_id = "import-preview-target"
    _seed_import_pair(
        auth_client,
        seeded_track_id,
        source_id=source_id,
        target_id=target_id,
    )

    preview = _preview(auth_client, source_id, target_id)
    assert preview["source"] == {
        "type": "mode",
        "id": source_id,
        "name": "Source Mode",
    }
    assert preview["source_mode"] == {"id": source_id, "name": "Source Mode"}
    assert preview["target_mode"] == {"id": target_id, "name": "Target Mode"}
    assert {item["kind"] for item in preview["items"]} == {
        "playlist",
        "soundboard",
        "interrupt",
        "preset",
        "cue",
    }
    assert all(item["status"] == "ready" for item in preview["items"])
    playlist_item = next(
        item for item in preview["items"] if item["kind"] == "playlist"
    )
    assert playlist_item["summary"] == "2 tracks · exploration"

    # Preview must not create anything in either persistence store.
    assert auth_client.get(
        "/api/playlists", params={"mode_id": target_id}
    ).json() == []
    target = auth_client.get(f"/api/modes/{target_id}").json()
    assert target["soundboards"] == {}
    assert target["presets"] == {}
    assert target["cues"] == {}
    assert target["interrupts"] == []


def test_commit_imports_selected_resources_and_repreview_reports_conflicts(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    source_id = "import-commit-source"
    target_id = "import-commit-target"
    _seed_import_pair(
        auth_client,
        seeded_track_id,
        source_id=source_id,
        target_id=target_id,
    )
    preview = _preview(auth_client, source_id, target_id)

    response = auth_client.post(
        "/api/authoring/import/commit",
        json={
            "source_mode_id": source_id,
            "target_mode_id": target_id,
            "items": _all_selections(preview),
        },
    )
    assert response.status_code == 200, response.text
    result = response.json()
    assert len(result["imported"]) == 5
    assert result["skipped"] == []
    assert result["missing_track_paths"] == []

    target = auth_client.get(f"/api/modes/{target_id}").json()
    assert target["playlist_categories"] == ["exploration"]
    assert set(target["soundboards"]) == {"storms"}
    assert set(target["presets"]) == {"dark-hall"}
    assert set(target["cues"]) == {"arrival"}
    assert [interrupt["name"] for interrupt in target["interrupts"]] == ["Ambush"]
    assert target["cues"]["arrival"]["playlist"] == "Night Walk"
    assert target["cues"]["arrival"]["preset"] == "dark-hall"

    playlists = auth_client.get(
        "/api/playlists", params={"mode_id": target_id}
    ).json()
    assert len(playlists) == 1
    assert playlists[0]["name"] == "Night Walk"
    assert playlists[0]["category"] == "exploration"
    tracks = auth_client.get(
        f"/api/playlists/{playlists[0]['id']}/tracks"
    ).json()
    assert [track["track_id"] for track in tracks] == [
        seeded_track_id,
        seeded_track_id,
    ]

    conflict_preview = _preview(auth_client, source_id, target_id)
    assert all(item["status"] == "conflict" for item in conflict_preview["items"])
    second = auth_client.post(
        "/api/authoring/import/commit",
        json={
            "source_mode_id": source_id,
            "target_mode_id": target_id,
            "items": _all_selections(conflict_preview),
        },
    )
    assert second.status_code == 200
    assert second.json()["imported"] == []
    assert len(second.json()["skipped"]) == 5


def test_commit_only_imports_explicit_selection(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    source_id = "import-subset-source"
    target_id = "import-subset-target"
    _seed_import_pair(
        auth_client,
        seeded_track_id,
        source_id=source_id,
        target_id=target_id,
    )
    preview = _preview(auth_client, source_id, target_id)
    preset = next(item for item in preview["items"] if item["kind"] == "preset")

    response = auth_client.post(
        "/api/authoring/import/commit",
        json={
            "source_mode_id": source_id,
            "target_mode_id": target_id,
            "items": [
                {"kind": preset["kind"], "resource_id": preset["resource_id"]}
            ],
        },
    )
    assert response.status_code == 200, response.text
    assert [item["kind"] for item in response.json()["imported"]] == ["preset"]
    target = auth_client.get(f"/api/modes/{target_id}").json()
    assert set(target["presets"]) == {"dark-hall"}
    assert target["soundboards"] == {}
    assert target["cues"] == {}
    assert target["interrupts"] == []
    assert auth_client.get(
        "/api/playlists", params={"mode_id": target_id}
    ).json() == []


def test_import_rejects_same_mode_and_stale_selection(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    same = auth_client.post(
        "/api/authoring/import/preview",
        json={"source_mode_id": "dnd", "target_mode_id": "dnd"},
    )
    assert same.status_code == 400

    source_id = "import-stale-source"
    target_id = "import-stale-target"
    _seed_import_pair(
        auth_client,
        seeded_track_id,
        source_id=source_id,
        target_id=target_id,
    )
    stale = auth_client.post(
        "/api/authoring/import/commit",
        json={
            "source_mode_id": source_id,
            "target_mode_id": target_id,
            "items": [{"kind": "preset", "resource_id": "gone"}],
        },
    )
    assert stale.status_code == 400


def test_import_refuses_late_file_collision_without_partial_changes(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    source_id = "import-race-source"
    target_id = "import-race-target"
    _seed_import_pair(
        auth_client,
        seeded_track_id,
        source_id=source_id,
        target_id=target_id,
    )
    preview = _preview(auth_client, source_id, target_id)

    target_dir = Path(os.environ["MODES_DIR"]) / target_id
    preset_path = target_dir / "presets" / "dark-hall.yaml"
    preset_path.parent.mkdir(parents=True, exist_ok=True)
    preset_path.write_text(
        yaml.safe_dump(
            {
                "id": "dark-hall",
                "name": "Externally created preset",
                "effects": [],
            },
            sort_keys=False,
        ),
        encoding="utf-8",
    )

    response = auth_client.post(
        "/api/authoring/import/commit",
        json={
            "source_mode_id": source_id,
            "target_mode_id": target_id,
            "items": _all_selections(preview),
        },
    )
    assert response.status_code == 409
    assert "target resource appeared during import" in response.json()["detail"]
    assert auth_client.get(
        "/api/playlists", params={"mode_id": target_id}
    ).json() == []
    assert list((target_dir / "soundboards").glob("*.yaml")) == []
    assert list((target_dir / "cues").glob("*.yaml")) == []
    assert "Externally created preset" in preset_path.read_text(encoding="utf-8")
    manifest = yaml.safe_load((target_dir / "manifest.yaml").read_text(encoding="utf-8"))
    assert manifest["playlist_categories"] == []
    assert manifest["interrupts"] == []


def test_import_rolls_back_files_manifest_and_database_on_reload_failure(
    auth_client: TestClient,
    seeded_track_id: int,
    monkeypatch,
) -> None:
    source_id = "import-rollback-source"
    target_id = "import-rollback-target"
    _seed_import_pair(
        auth_client,
        seeded_track_id,
        source_id=source_id,
        target_id=target_id,
    )
    preview = _preview(auth_client, source_id, target_id)

    from app.modes import loader as modes_loader

    original_reload = modes_loader.reload_mode
    failed = False

    def fail_first_target_reload(mode_id: str):
        nonlocal failed
        if mode_id == target_id and not failed:
            failed = True
            raise ValueError("synthetic reload failure")
        return original_reload(mode_id)

    monkeypatch.setattr(modes_loader, "reload_mode", fail_first_target_reload)
    response = auth_client.post(
        "/api/authoring/import/commit",
        json={
            "source_mode_id": source_id,
            "target_mode_id": target_id,
            "items": _all_selections(preview),
        },
    )
    assert response.status_code == 500
    assert "rolled back" in response.json()["detail"]

    assert auth_client.get(
        "/api/playlists", params={"mode_id": target_id}
    ).json() == []
    target_dir = Path(os.environ["MODES_DIR"]) / target_id
    assert list((target_dir / "soundboards").glob("*.yaml")) == []
    assert list((target_dir / "presets").glob("*.yaml")) == []
    assert list((target_dir / "cues").glob("*.yaml")) == []
    manifest = yaml.safe_load((target_dir / "manifest.yaml").read_text(encoding="utf-8"))
    assert manifest["playlist_categories"] == []
    assert manifest["interrupts"] == []
