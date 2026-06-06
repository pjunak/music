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


# --- scaffolding ---------------------------------------------------------


def test_create_preset_writes_yaml(auth_client: TestClient) -> None:
    r = auth_client.post(
        "/api/presets",
        json={
            "id": "newp",
            "name": "Brand New",
            "description": "test",
            "effects": [{"type": "lowpass", "frequency": 800}],
        },
    )
    assert r.status_code == 201
    body = r.json()
    assert body["id"] == "newp"
    assert body["effects"][0]["type"] == "lowpass"

    presets_dir = Path(os.environ["PRESETS_DIR"])
    assert (presets_dir / "newp.yaml").is_file()

    listed = {p["id"] for p in auth_client.get("/api/presets").json()}
    assert "newp" in listed


def test_create_preset_rolls_back_on_unknown_effect_type(
    auth_client: TestClient,
) -> None:
    r = auth_client.post(
        "/api/presets",
        json={
            "id": "badp",
            "name": "Bad",
            "effects": [{"type": "nonsense"}],
        },
    )
    assert r.status_code == 400
    presets_dir = Path(os.environ["PRESETS_DIR"])
    # File rolled back so the operator can fix and retry.
    assert not (presets_dir / "badp.yaml").exists()


def test_create_preset_rejects_invalid_id(auth_client: TestClient) -> None:
    for bad in ["With Space", "../escape", "UPPER"]:
        r = auth_client.post("/api/presets", json={"id": bad, "name": "X"})
        assert r.status_code == 400, bad


def test_create_preset_conflict(auth_client: TestClient) -> None:
    auth_client.post("/api/presets", json={"id": "dupp", "name": "First"})
    r = auth_client.post("/api/presets", json={"id": "dupp", "name": "Second"})
    assert r.status_code == 409


def test_update_preset_replaces_effects(auth_client: TestClient) -> None:
    auth_client.post(
        "/api/presets",
        json={
            "id": "updp",
            "name": "U",
            "effects": [{"type": "lowpass", "frequency": 800}],
        },
    )
    r = auth_client.put(
        "/api/presets/updp",
        json={"effects": [{"type": "highpass", "frequency": 200}]},
    )
    assert r.status_code == 200
    body = r.json()
    assert body["effects"][0]["type"] == "highpass"


def test_preset_volume_crossfade_roundtrip_and_apply(auth_client: TestClient) -> None:
    # A preset can optionally pin master volume + crossfade ("mood" overrides).
    r = auth_client.post(
        "/api/presets",
        json={
            "id": "moody",
            "name": "Moody",
            "effects": [{"type": "lowpass", "frequency": 600}],
            "volume": 0.5,
            "crossfade_ms": 3000,
        },
    )
    assert r.status_code == 201, r.text
    body = r.json()
    assert body["volume"] == 0.5
    assert body["crossfade_ms"] == 3000

    # GET round-trips the overrides from disk.
    got = auth_client.get("/api/presets/moody").json()
    assert got["volume"] == 0.5
    assert got["crossfade_ms"] == 3000

    # Activating the preset applies the overrides to the shared player state.
    with auth_client.websocket_connect("/api/ws") as ws:
        ws.receive_json()  # snapshot
        ws.send_json({"type": "set_active_presets", "preset_ids": ["moody"]})
        msg = ws.receive_json()
        assert msg["state"]["active_preset_ids"] == ["moody"]
        assert msg["state"]["volume"] == 0.5
        assert msg["state"]["crossfade_ms"] == 3000


def test_preset_overrides_last_active_wins(auth_client: TestClient) -> None:
    auth_client.post(
        "/api/presets",
        json={"id": "loud", "name": "Loud", "effects": [], "volume": 0.9},
    )
    auth_client.post(
        "/api/presets",
        json={"id": "soft", "name": "Soft", "effects": [], "volume": 0.2},
    )
    with auth_client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        ws.send_json({"type": "set_active_presets", "preset_ids": ["loud", "soft"]})
        msg = ws.receive_json()
        assert msg["state"]["volume"] == 0.2  # last in the active list wins


def test_preset_update_can_clear_overrides(auth_client: TestClient) -> None:
    auth_client.post(
        "/api/presets",
        json={"id": "clearme", "name": "C", "effects": [], "volume": 0.4},
    )
    # Re-saving the full form with volume omitted (null) clears it.
    r = auth_client.put("/api/presets/clearme", json={"name": "C", "effects": []})
    assert r.status_code == 200
    assert r.json()["volume"] is None


def test_delete_preset_removes_file_and_prunes_active(auth_client: TestClient) -> None:
    auth_client.post(
        "/api/presets",
        json={
            "id": "delp",
            "name": "Doomed",
            "effects": [{"type": "lowpass", "frequency": 800}],
        },
    )
    auth_client.put("/api/presets/active", json={"preset_ids": ["cave", "delp"]})

    r = auth_client.delete("/api/presets/delp")
    assert r.status_code == 204

    presets_dir = Path(os.environ["PRESETS_DIR"])
    assert not (presets_dir / "delp.yaml").exists()

    active = auth_client.get("/api/presets/active").json()
    assert "delp" not in active["preset_ids"]
