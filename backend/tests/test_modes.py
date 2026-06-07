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


def test_get_mode_includes_soundboards(auth_client: TestClient) -> None:
    r = auth_client.get("/api/modes/dnd")
    assert r.status_code == 200
    body = r.json()
    assert body["id"] == "dnd"
    assert set(body["soundboards"].keys()) == {"tavern", "dungeon"}
    tavern = body["soundboards"]["tavern"]
    assert tavern["categories"][0]["id"] == "doors"
    assert tavern["categories"][0]["items"][0]["hotkey"] == "d"


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
    assert (modes_dir / "newmode" / "cues").is_dir()
    assert (modes_dir / "newmode" / "presets").is_dir()

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


# --- soundboard editor (categories + items) -----------------------------


def _bootstrap_soundboard(auth_client: TestClient, mode_id: str = "sbed") -> None:
    auth_client.post("/api/modes", json={"id": mode_id, "name": "SB Editor"})
    auth_client.post(f"/api/modes/{mode_id}/soundboards", json={"id": "tavern"})


def test_add_category(auth_client: TestClient) -> None:
    _bootstrap_soundboard(auth_client)
    r = auth_client.post(
        "/api/modes/sbed/soundboards/tavern/categories",
        json={"id": "doors", "name": "Doors"},
    )
    assert r.status_code == 201
    body = r.json()
    cats = {c["id"]: c for c in body["categories"]}
    assert "doors" in cats
    assert cats["doors"]["name"] == "Doors"


def test_add_category_conflict(auth_client: TestClient) -> None:
    _bootstrap_soundboard(auth_client, "sbed_conflict")
    auth_client.post(
        "/api/modes/sbed_conflict/soundboards/tavern/categories",
        json={"id": "doors", "name": "Doors"},
    )
    r = auth_client.post(
        "/api/modes/sbed_conflict/soundboards/tavern/categories",
        json={"id": "doors", "name": "Other"},
    )
    assert r.status_code == 409


def test_delete_category(auth_client: TestClient) -> None:
    _bootstrap_soundboard(auth_client, "sbed_delcat")
    auth_client.post(
        "/api/modes/sbed_delcat/soundboards/tavern/categories",
        json={"id": "ambience", "name": "Ambience"},
    )
    r = auth_client.delete(
        "/api/modes/sbed_delcat/soundboards/tavern/categories/ambience"
    )
    assert r.status_code == 200
    body = r.json()
    assert all(c["id"] != "ambience" for c in body["categories"])


def test_add_item(auth_client: TestClient) -> None:
    _bootstrap_soundboard(auth_client, "sbed_item")
    auth_client.post(
        "/api/modes/sbed_item/soundboards/tavern/categories",
        json={"id": "doors", "name": "Doors"},
    )
    r = auth_client.post(
        "/api/modes/sbed_item/soundboards/tavern/categories/doors/items",
        json={"file": "dnd/door.ogg", "name": "Door slam", "hotkey": "d"},
    )
    assert r.status_code == 201
    body = r.json()
    items = next(c for c in body["categories"] if c["id"] == "doors")["items"]
    assert len(items) == 1
    assert items[0]["file"] == "dnd/door.ogg"
    assert items[0]["name"] == "Door slam"
    assert items[0]["hotkey"] == "d"


def test_add_item_unknown_category_404(auth_client: TestClient) -> None:
    _bootstrap_soundboard(auth_client, "sbed_unknown")
    r = auth_client.post(
        "/api/modes/sbed_unknown/soundboards/tavern/categories/nope/items",
        json={"file": "x.ogg", "name": "x"},
    )
    assert r.status_code == 404


def test_update_item(auth_client: TestClient) -> None:
    _bootstrap_soundboard(auth_client, "sbed_upd")
    auth_client.post(
        "/api/modes/sbed_upd/soundboards/tavern/categories",
        json={"id": "doors", "name": "Doors"},
    )
    auth_client.post(
        "/api/modes/sbed_upd/soundboards/tavern/categories/doors/items",
        json={"file": "dnd/door.ogg", "name": "Door slam"},
    )
    r = auth_client.patch(
        "/api/modes/sbed_upd/soundboards/tavern/categories/doors/items/0",
        json={"name": "Slamming door", "hotkey": "s"},
    )
    assert r.status_code == 200
    items = next(c for c in r.json()["categories"] if c["id"] == "doors")["items"]
    assert items[0]["name"] == "Slamming door"
    assert items[0]["hotkey"] == "s"


def test_delete_item(auth_client: TestClient) -> None:
    _bootstrap_soundboard(auth_client, "sbed_del")
    auth_client.post(
        "/api/modes/sbed_del/soundboards/tavern/categories",
        json={"id": "doors", "name": "Doors"},
    )
    for n in range(3):
        auth_client.post(
            "/api/modes/sbed_del/soundboards/tavern/categories/doors/items",
            json={"file": f"dnd/door{n}.ogg", "name": f"Door {n}"},
        )
    r = auth_client.delete(
        "/api/modes/sbed_del/soundboards/tavern/categories/doors/items/1"
    )
    assert r.status_code == 200
    items = next(c for c in r.json()["categories"] if c["id"] == "doors")["items"]
    assert [it["name"] for it in items] == ["Door 0", "Door 2"]


def test_delete_item_index_out_of_range(auth_client: TestClient) -> None:
    _bootstrap_soundboard(auth_client, "sbed_oor")
    auth_client.post(
        "/api/modes/sbed_oor/soundboards/tavern/categories",
        json={"id": "doors", "name": "Doors"},
    )
    r = auth_client.delete(
        "/api/modes/sbed_oor/soundboards/tavern/categories/doors/items/5"
    )
    assert r.status_code == 404


def test_added_item_is_referenced_by_sfx_endpoint(
    auth_client: TestClient,
) -> None:
    """Adding an item to a soundboard should make `/api/sfx/file?path=` pass
    the reference check for that file (assuming the file exists on disk)."""
    _bootstrap_soundboard(auth_client, "sbed_ref")
    auth_client.post(
        "/api/modes/sbed_ref/soundboards/tavern/categories",
        json={"id": "doors", "name": "Doors"},
    )
    auth_client.post(
        "/api/modes/sbed_ref/soundboards/tavern/categories/doors/items",
        json={"file": "dnd/door.ogg", "name": "Door slam"},
    )
    # conftest seeds dnd/door.ogg into SFX_LIBRARY_DIR.
    r = auth_client.get("/api/sfx/file", params={"path": "dnd/door.ogg"})
    assert r.status_code == 200



# --- cues ---------------------------------------------------------------


def test_cue_crud_roundtrip(auth_client: TestClient) -> None:
    # Create a cue with a preset, playlist ref, start point, and a loop.
    r = auth_client.post(
        "/api/modes/dnd/cues",
        json={
            "id": "kraken",
            "name": "Kraken Fight",
            "preset": "cave",
            "playlist": "Fight",
            "start_index": 2,
            "start_ms": 90000,
            "sfx": [{"soundboard": "tavern", "item": "dnd/door.ogg"}],
            "loops": [
                {
                    "soundboard": "tavern",
                    "item": "dnd/door.ogg",
                    "interval_s": 45,
                    "volume": 0.5,
                }
            ],
        },
    )
    assert r.status_code == 201, r.text
    body = r.json()
    assert body["id"] == "kraken"
    assert body["preset"] == "cave"
    assert body["start_ms"] == 90000
    assert body["loops"][0]["interval_s"] == 45

    # Appears in mode detail.
    detail = auth_client.get("/api/modes/dnd").json()
    assert "kraken" in detail["cues"]

    # Update is a full replace.
    r = auth_client.put(
        "/api/modes/dnd/cues/kraken",
        json={"name": "Kraken!", "loops": []},
    )
    assert r.status_code == 200
    assert r.json()["name"] == "Kraken!"
    assert r.json()["loops"] == []

    # Delete.
    assert auth_client.delete("/api/modes/dnd/cues/kraken").status_code == 204
    assert "kraken" not in auth_client.get("/api/modes/dnd").json()["cues"]


def test_cue_create_conflict(auth_client: TestClient) -> None:
    auth_client.post("/api/modes/dnd/cues", json={"id": "dup", "name": "A"})
    r = auth_client.post("/api/modes/dnd/cues", json={"id": "dup", "name": "B"})
    assert r.status_code == 409


def test_cue_create_rejects_bad_slug(auth_client: TestClient) -> None:
    r = auth_client.post("/api/modes/dnd/cues", json={"id": "Bad Slug", "name": "X"})
    assert r.status_code == 400


def test_rename_mode(auth_client: TestClient) -> None:
    r = auth_client.patch("/api/modes/dnd", json={"name": "D&D Reborn"})
    assert r.status_code == 200, r.text
    assert r.json()["name"] == "D&D Reborn"
    listed = {m["id"]: m["name"] for m in auth_client.get("/api/modes").json()}
    assert listed["dnd"] == "D&D Reborn"


def test_rename_mode_404(auth_client: TestClient) -> None:
    assert auth_client.patch("/api/modes/nope", json={"name": "X"}).status_code == 404
