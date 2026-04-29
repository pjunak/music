"""Modes API: list, detail, active, reload, theme."""
import os
from pathlib import Path

from fastapi.testclient import TestClient


def test_list_requires_auth(client: TestClient) -> None:
    assert client.get("/api/modes").status_code == 401


def test_list_returns_loaded_modes(auth_client: TestClient) -> None:
    r = auth_client.get("/api/modes")
    assert r.status_code == 200
    body = r.json()
    ids = {m["id"] for m in body}
    assert "dnd" in ids
    dnd = next(m for m in body if m["id"] == "dnd")
    assert dnd["name"] == "Test DnD Mode"
    assert dnd["panels"] == ["now-playing"]
    assert dnd["has_theme"] is True
    assert dnd["default_crossfade_ms"] == 1500
    assert dnd["default_soundboard"] == "tavern"


def test_get_mode_includes_soundboards_and_scenes(auth_client: TestClient) -> None:
    r = auth_client.get("/api/modes/dnd")
    assert r.status_code == 200
    body = r.json()
    assert body["id"] == "dnd"
    assert set(body["soundboards"].keys()) == {"tavern", "dungeon"}
    tavern = body["soundboards"]["tavern"]
    assert tavern["categories"][0]["id"] == "doors"
    assert tavern["categories"][0]["items"][0]["hotkey"] == "d"
    assert "tavern" in body["scenes"]
    scene = body["scenes"]["tavern"]
    assert scene["name"] == "Stonehill Inn"
    assert scene["ambient"]["playlist"] == "tavern-music"
    assert scene["presets"] == ["radio-vintage"]


def test_get_unknown_mode_404(auth_client: TestClient) -> None:
    assert auth_client.get("/api/modes/nope").status_code == 404


def test_active_mode_initially_null(auth_client: TestClient) -> None:
    r = auth_client.get("/api/modes/active")
    assert r.status_code == 200
    assert r.json() == {"mode_id": None}


def test_set_active_mode_persists(auth_client: TestClient) -> None:
    r = auth_client.put("/api/modes/active", json={"mode_id": "dnd"})
    assert r.status_code == 200
    assert r.json() == {"mode_id": "dnd"}
    assert auth_client.get("/api/modes/active").json() == {"mode_id": "dnd"}


def test_set_active_mode_validates(auth_client: TestClient) -> None:
    assert auth_client.put("/api/modes/active", json={"mode_id": "nope"}).status_code == 400


def test_set_active_mode_can_clear(auth_client: TestClient) -> None:
    auth_client.put("/api/modes/active", json={"mode_id": "dnd"})
    r = auth_client.put("/api/modes/active", json={"mode_id": None})
    assert r.status_code == 200
    assert auth_client.get("/api/modes/active").json() == {"mode_id": None}


def test_theme_css_served(auth_client: TestClient) -> None:
    r = auth_client.get("/api/modes/dnd/theme.css")
    assert r.status_code == 200
    assert "text/css" in r.headers["content-type"]
    assert "data-mode='dnd'" in r.text or 'data-mode="dnd"' in r.text


def test_theme_css_404_for_unknown_mode(auth_client: TestClient) -> None:
    assert auth_client.get("/api/modes/nope/theme.css").status_code == 404


def test_reload_picks_up_new_mode(auth_client: TestClient) -> None:
    modes_dir = Path(os.environ["MODES_DIR"])
    new_dir = modes_dir / "cyberpunk"
    new_dir.mkdir(exist_ok=True)
    (new_dir / "manifest.yaml").write_text(
        "id: cyberpunk\nname: Cyberpunk\npanels: []\n", encoding="utf-8"
    )
    try:
        r = auth_client.post("/api/modes/reload")
        assert r.status_code == 200
        body = r.json()
        assert "cyberpunk" in body["loaded"]
        assert body["errors"] == {}

        ids = {m["id"] for m in auth_client.get("/api/modes").json()}
        assert "cyberpunk" in ids
    finally:
        import shutil

        shutil.rmtree(new_dir)
        auth_client.post("/api/modes/reload")


def test_reload_surfaces_broken_manifests(auth_client: TestClient) -> None:
    modes_dir = Path(os.environ["MODES_DIR"])
    bad_dir = modes_dir / "broken"
    bad_dir.mkdir(exist_ok=True)
    (bad_dir / "manifest.yaml").write_text(
        "id: not-broken\nname: Mismatch\n",
        encoding="utf-8",
    )
    try:
        r = auth_client.post("/api/modes/reload")
        assert r.status_code == 200
        body = r.json()
        assert "broken" in body["errors"]
        assert "broken" not in body["loaded"]
    finally:
        import shutil

        shutil.rmtree(bad_dir)
        auth_client.post("/api/modes/reload")


# --- scaffolding ---------------------------------------------------------


def test_create_mode_scaffolds_dir_and_manifest(auth_client: TestClient) -> None:
    r = auth_client.post(
        "/api/modes", json={"id": "newmode", "name": "Brand New"}
    )
    assert r.status_code == 201
    body = r.json()
    assert body["id"] == "newmode"
    assert body["name"] == "Brand New"

    modes_dir = Path(os.environ["MODES_DIR"])
    assert (modes_dir / "newmode" / "manifest.yaml").is_file()
    assert (modes_dir / "newmode" / "soundboards").is_dir()
    assert (modes_dir / "newmode" / "scenes").is_dir()

    listed = {m["id"] for m in auth_client.get("/api/modes").json()}
    assert "newmode" in listed


def test_create_mode_rejects_invalid_id(auth_client: TestClient) -> None:
    for bad in ["With Space", "../escape", "UPPER", "-leadingdash"]:
        r = auth_client.post("/api/modes", json={"id": bad, "name": "X"})
        assert r.status_code == 400, bad


def test_create_mode_conflict(auth_client: TestClient) -> None:
    auth_client.post("/api/modes", json={"id": "conflict", "name": "First"})
    r = auth_client.post("/api/modes", json={"id": "conflict", "name": "Second"})
    assert r.status_code == 409


def test_delete_mode_removes_dir(auth_client: TestClient) -> None:
    auth_client.post("/api/modes", json={"id": "doomed", "name": "Bye"})
    r = auth_client.delete("/api/modes/doomed")
    assert r.status_code == 204
    modes_dir = Path(os.environ["MODES_DIR"])
    assert not (modes_dir / "doomed").exists()
    listed = {m["id"] for m in auth_client.get("/api/modes").json()}
    assert "doomed" not in listed


def test_delete_unknown_mode_404(auth_client: TestClient) -> None:
    assert auth_client.delete("/api/modes/never-existed").status_code == 404


def test_create_soundboard_writes_yaml(auth_client: TestClient) -> None:
    auth_client.post("/api/modes", json={"id": "sbtest", "name": "SB Host"})
    r = auth_client.post(
        "/api/modes/sbtest/soundboards", json={"id": "tavern", "name": "Tavern"}
    )
    assert r.status_code == 201
    assert r.json()["id"] == "tavern"

    modes_dir = Path(os.environ["MODES_DIR"])
    assert (modes_dir / "sbtest" / "soundboards" / "tavern.yaml").is_file()

    detail = auth_client.get("/api/modes/sbtest").json()
    assert "tavern" in detail["soundboards"]


def test_delete_soundboard(auth_client: TestClient) -> None:
    auth_client.post("/api/modes", json={"id": "sbdel", "name": "SB Del"})
    auth_client.post("/api/modes/sbdel/soundboards", json={"id": "x"})
    r = auth_client.delete("/api/modes/sbdel/soundboards/x")
    assert r.status_code == 204
    detail = auth_client.get("/api/modes/sbdel").json()
    assert "x" not in detail["soundboards"]


def test_create_scene(auth_client: TestClient) -> None:
    auth_client.post("/api/modes", json={"id": "scenetest", "name": "Scene Host"})
    r = auth_client.post(
        "/api/modes/scenetest/scenes",
        json={"id": "tavern", "name": "Stonehill Inn", "description": "Quiet"},
    )
    assert r.status_code == 201

    modes_dir = Path(os.environ["MODES_DIR"])
    assert (modes_dir / "scenetest" / "scenes" / "tavern.yaml").is_file()

    detail = auth_client.get("/api/modes/scenetest").json()
    assert "tavern" in detail["scenes"]


def test_delete_scene(auth_client: TestClient) -> None:
    auth_client.post("/api/modes", json={"id": "scenedel", "name": "Scene Del"})
    auth_client.post(
        "/api/modes/scenedel/scenes", json={"id": "x", "name": "X"}
    )
    r = auth_client.delete("/api/modes/scenedel/scenes/x")
    assert r.status_code == 204
    detail = auth_client.get("/api/modes/scenedel").json()
    assert "x" not in detail["scenes"]

