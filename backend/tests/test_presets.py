"""EQ presets — now mode-scoped (modes/<mode>/presets/), CRUD on the modes API."""
import os
from pathlib import Path

import pytest
from fastapi.testclient import TestClient


@pytest.fixture(autouse=True)
def _reset_sync_state():
    """Reset the process-wide player state between tests — these exercise WS
    actions, and stale state (e.g. active_mode_id) would make a same-value
    write no-op into a hung receive_json()."""
    from tests.conftest import reset_sync_singletons

    reset_sync_singletons()
    yield
    reset_sync_singletons()


def test_presets_appear_in_mode_detail(auth_client: TestClient) -> None:
    detail = auth_client.get("/api/modes/dnd").json()
    assert "cave" in detail["presets"]
    assert "radio-vintage" in detail["presets"]
    assert detail["presets"]["cave"]["effects"][0]["type"] == "reverb"


def test_create_preset_writes_yaml(auth_client: TestClient) -> None:
    r = auth_client.post(
        "/api/modes/dnd/presets",
        json={
            "id": "newp",
            "name": "Brand New",
            "description": "test",
            "effects": [{"type": "lowpass", "frequency": 800}],
        },
    )
    assert r.status_code == 201, r.text
    body = r.json()
    assert body["id"] == "newp"
    assert body["effects"][0]["type"] == "lowpass"

    presets_dir = Path(os.environ["MODES_DIR"]) / "dnd" / "presets"
    assert (presets_dir / "newp.yaml").is_file()
    assert "newp" in auth_client.get("/api/modes/dnd").json()["presets"]


def test_create_preset_graphic_eq_bands_roundtrip(auth_client: TestClient) -> None:
    """A graphic-EQ effect carries a nested `bands` list (freq + gain). The
    loose `extra=allow` effect spec must preserve it through save → reload."""
    bands = [
        {"frequency": 32, "gain": 4.5},
        {"frequency": 1000, "gain": -3.0},
        {"frequency": 16000, "gain": 6.0},
    ]
    r = auth_client.post(
        "/api/modes/dnd/presets",
        json={"id": "eqp", "name": "Smiley", "effects": [{"type": "eq", "bands": bands}]},
    )
    assert r.status_code == 201, r.text
    eff = r.json()["effects"][0]
    assert eff["type"] == "eq"
    assert eff["bands"] == bands

    # Survives a fresh load from disk too.
    detail = auth_client.get("/api/modes/dnd").json()
    assert detail["presets"]["eqp"]["effects"][0]["bands"] == bands


def test_create_preset_rejects_unknown_effect_type(auth_client: TestClient) -> None:
    r = auth_client.post(
        "/api/modes/dnd/presets",
        json={"id": "badp", "name": "Bad", "effects": [{"type": "nonsense"}]},
    )
    assert r.status_code == 400
    presets_dir = Path(os.environ["MODES_DIR"]) / "dnd" / "presets"
    assert not (presets_dir / "badp.yaml").exists()


def test_create_preset_rejects_invalid_id(auth_client: TestClient) -> None:
    for bad in ["With Space", "../escape", "UPPER"]:
        r = auth_client.post("/api/modes/dnd/presets", json={"id": bad, "name": "X"})
        assert r.status_code == 400, bad


def test_create_preset_conflict(auth_client: TestClient) -> None:
    auth_client.post("/api/modes/dnd/presets", json={"id": "dupp", "name": "First"})
    r = auth_client.post("/api/modes/dnd/presets", json={"id": "dupp", "name": "Second"})
    assert r.status_code == 409


def test_create_preset_unknown_mode(auth_client: TestClient) -> None:
    r = auth_client.post("/api/modes/nope/presets", json={"id": "x", "name": "X"})
    assert r.status_code == 404


def test_update_preset_replaces_effects(auth_client: TestClient) -> None:
    auth_client.post(
        "/api/modes/dnd/presets",
        json={"id": "updp", "name": "U", "effects": [{"type": "lowpass", "frequency": 800}]},
    )
    r = auth_client.put(
        "/api/modes/dnd/presets/updp",
        json={"name": "U", "effects": [{"type": "highpass", "frequency": 200}]},
    )
    assert r.status_code == 200
    assert r.json()["effects"][0]["type"] == "highpass"


def test_delete_preset_removes_file(auth_client: TestClient) -> None:
    auth_client.post("/api/modes/dnd/presets", json={"id": "delp", "name": "Doomed"})
    assert auth_client.delete("/api/modes/dnd/presets/delp").status_code == 204
    presets_dir = Path(os.environ["MODES_DIR"]) / "dnd" / "presets"
    assert not (presets_dir / "delp.yaml").exists()
    assert "delp" not in auth_client.get("/api/modes/dnd").json()["presets"]


def test_preset_crossfade_roundtrip_and_apply(auth_client: TestClient) -> None:
    r = auth_client.post(
        "/api/modes/dnd/presets",
        json={
            "id": "moody",
            "name": "Moody",
            "effects": [{"type": "lowpass", "frequency": 600}],
            # Accepted as an ignored legacy field; presets no longer mutate output volume.
            "volume": 0.5,
            "crossfade_ms": 3000,
        },
    )
    assert r.status_code == 201, r.text
    assert "volume" not in r.json()
    assert r.json()["crossfade_ms"] == 3000

    with auth_client.websocket_connect("/api/ws") as ws:
        ws.receive_json()  # snapshot
        ws.send_json({"type": "set_active_mode", "mode_id": "dnd"})
        ws.receive_json()
        ws.send_json({"type": "set_active_presets", "preset_ids": ["moody"]})
        msg = ws.receive_json()
        assert msg["state"]["active_preset_ids"] == ["moody"]
        assert msg["state"]["volume"] == 1.0
        assert msg["state"]["crossfade_ms"] == 3000


def test_preset_crossfade_override_last_active_wins(auth_client: TestClient) -> None:
    auth_client.post(
        "/api/modes/dnd/presets",
        json={"id": "slow", "name": "Slow", "effects": [], "crossfade_ms": 9000},
    )
    auth_client.post(
        "/api/modes/dnd/presets",
        json={"id": "quick", "name": "Quick", "effects": [], "crossfade_ms": 200},
    )
    with auth_client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        ws.send_json({"type": "set_active_mode", "mode_id": "dnd"})
        ws.receive_json()
        ws.send_json({"type": "set_active_presets", "preset_ids": ["slow", "quick"]})
        msg = ws.receive_json()
        assert msg["state"]["crossfade_ms"] == 200  # last in the active list wins


def test_set_active_presets_requires_active_mode(auth_client: TestClient) -> None:
    # No active mode → no presets resolve → any id is unknown.
    with auth_client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        ws.send_json({"type": "set_active_presets", "preset_ids": ["cave"]})
        msg = ws.receive_json()
        assert msg["type"] == "error"
