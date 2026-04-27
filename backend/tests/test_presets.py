"""Preset loader + API: list, active, reload, unknown-preset validation."""
import os
from pathlib import Path

from fastapi.testclient import TestClient


def test_list_requires_auth(client: TestClient) -> None:
    assert client.get("/api/presets").status_code == 401


def test_list_returns_loaded_presets(auth_client: TestClient) -> None:
    r = auth_client.get("/api/presets")
    assert r.status_code == 200
    ids = {p["id"] for p in r.json()}
    assert {"cave", "radio-vintage"} <= ids


def test_get_one(auth_client: TestClient) -> None:
    r = auth_client.get("/api/presets/cave")
    assert r.status_code == 200
    body = r.json()
    assert body["id"] == "cave"
    assert body["effects"][0]["type"] == "reverb"


def test_get_unknown_404(auth_client: TestClient) -> None:
    assert auth_client.get("/api/presets/nope").status_code == 404


def test_active_initially_empty(auth_client: TestClient) -> None:
    r = auth_client.get("/api/presets/active")
    assert r.status_code == 200
    assert r.json() == {"preset_ids": []}


def test_set_active_persists(auth_client: TestClient) -> None:
    r = auth_client.put("/api/presets/active", json={"preset_ids": ["cave"]})
    assert r.status_code == 200
    assert r.json() == {"preset_ids": ["cave"]}
    assert auth_client.get("/api/presets/active").json() == {"preset_ids": ["cave"]}


def test_set_active_deduplicates(auth_client: TestClient) -> None:
    r = auth_client.put(
        "/api/presets/active",
        json={"preset_ids": ["cave", "radio-vintage", "cave"]},
    )
    assert r.json() == {"preset_ids": ["cave", "radio-vintage"]}


def test_set_active_rejects_unknown_preset(auth_client: TestClient) -> None:
    r = auth_client.put("/api/presets/active", json={"preset_ids": ["nope"]})
    assert r.status_code == 400


def test_set_active_can_clear(auth_client: TestClient) -> None:
    auth_client.put("/api/presets/active", json={"preset_ids": ["cave"]})
    r = auth_client.put("/api/presets/active", json={"preset_ids": []})
    assert r.status_code == 200
    assert auth_client.get("/api/presets/active").json() == {"preset_ids": []}


def test_reload_picks_up_new_preset(auth_client: TestClient) -> None:
    presets_dir = Path(os.environ["PRESETS_DIR"])
    new_path = presets_dir / "church.yaml"
    new_path.write_text(
        "id: church\nname: Church\neffects:\n  - type: reverb\n    wet: 0.7\n",
        encoding="utf-8",
    )
    try:
        r = auth_client.post("/api/presets/reload")
        assert r.status_code == 200
        assert "church" in r.json()["loaded"]
        assert r.json()["errors"] == {}
        ids = {p["id"] for p in auth_client.get("/api/presets").json()}
        assert "church" in ids
    finally:
        new_path.unlink(missing_ok=True)
        auth_client.post("/api/presets/reload")


def test_reload_reports_broken_preset(auth_client: TestClient) -> None:
    presets_dir = Path(os.environ["PRESETS_DIR"])
    broken = presets_dir / "broken.yaml"
    broken.write_text(
        "id: broken\nname: Bad\neffects:\n  - type: nonsense\n",
        encoding="utf-8",
    )
    try:
        r = auth_client.post("/api/presets/reload")
        assert r.status_code == 200
        assert "broken" in r.json()["errors"]
        assert "broken" not in r.json()["loaded"]
    finally:
        broken.unlink(missing_ok=True)
        auth_client.post("/api/presets/reload")


def test_reload_prunes_stale_active_ids(auth_client: TestClient) -> None:
    presets_dir = Path(os.environ["PRESETS_DIR"])
    temp_path = presets_dir / "temporary.yaml"
    temp_path.write_text(
        "id: temporary\nname: Temp\neffects:\n  - type: delay\n    time: 0.2\n",
        encoding="utf-8",
    )
    try:
        auth_client.post("/api/presets/reload")
        auth_client.put(
            "/api/presets/active", json={"preset_ids": ["cave", "temporary"]}
        )
        # Now remove the preset and reload — "temporary" should be pruned.
        temp_path.unlink()
        auth_client.post("/api/presets/reload")
        r = auth_client.get("/api/presets/active")
        assert r.json() == {"preset_ids": ["cave"]}
    finally:
        temp_path.unlink(missing_ok=True)
        auth_client.put("/api/presets/active", json={"preset_ids": []})
        auth_client.post("/api/presets/reload")
