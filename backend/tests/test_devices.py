"""Device registry HTTP surface — the operator's manually-curated, file-backed
device list and the audio-output designation that gates activation."""
from __future__ import annotations

import json
import os
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from tests.conftest import reset_sync_singletons


@pytest.fixture(autouse=True)
def _reset_sync_state():
    reset_sync_singletons()
    yield
    reset_sync_singletons()


def _register(ws, name: str, client_id: str) -> None:
    ws.send_json({"type": "register", "name": name, "client_id": client_id})


def test_list_requires_auth(client: TestClient) -> None:
    assert client.get("/api/devices").status_code == 401


def test_connecting_does_not_auto_save(auth_client: TestClient) -> None:
    """Remembering a device is manual — connecting alone never adds it."""
    with auth_client.websocket_connect("/api/ws") as ws:
        ws.receive_json()  # snapshot
        _register(ws, "Phone", "phone-1")
        ws.receive_json()  # register broadcast

        assert auth_client.get("/api/devices").json() == []


def test_put_saves_device(auth_client: TestClient) -> None:
    with auth_client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        _register(ws, "TV", "tv-client")
        ws.receive_json()

        r = auth_client.put("/api/devices/tv-client", json={"name": "Living Room TV"})
        assert r.status_code == 200, r.text
        body = r.json()
        assert body["name"] == "Living Room TV"
        assert body["is_output"] is False
        assert body["connected"] is True
        ws.receive_json()  # save broadcast

        rows = auth_client.get("/api/devices").json()
        assert any(x["client_id"] == "tv-client" for x in rows)


def test_put_is_output_enables_activation(auth_client: TestClient) -> None:
    with auth_client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        _register(ws, "TV", "tv-client")
        ws.receive_json()

        # Not saved/designated → activation rejected.
        ws.send_json({"type": "set_active_outputs", "device_ids": ["tv-client"]})
        assert ws.receive_json()["type"] == "error"

        # Save as a designated output.
        r = auth_client.put(
            "/api/devices/tv-client", json={"name": "TV", "is_output": True}
        )
        assert r.status_code == 200
        assert r.json()["is_output"] is True
        ws.receive_json()  # save broadcast

        ws.send_json({"type": "set_active_outputs", "device_ids": ["tv-client"]})
        msg = ws.receive_json()
        assert msg["type"] == "state_changed"
        assert msg["state"]["active_output_device_ids"] == ["tv-client"]


def test_put_is_output_off_removes_active(auth_client: TestClient) -> None:
    with auth_client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        _register(ws, "TV", "tv-client")
        ws.receive_json()
        auth_client.put("/api/devices/tv-client", json={"name": "TV", "is_output": True})
        ws.receive_json()  # save broadcast
        ws.send_json({"type": "set_active_outputs", "device_ids": ["tv-client"]})
        assert ws.receive_json()["state"]["active_output_device_ids"] == ["tv-client"]

        # Flip output off → dropped from the live active set immediately.
        auth_client.put("/api/devices/tv-client", json={"name": "TV", "is_output": False})
        msg = ws.receive_json()
        assert "tv-client" not in msg["state"]["active_output_device_ids"]


def test_delete_device(auth_client: TestClient) -> None:
    with auth_client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        _register(ws, "TV", "tv-client")
        ws.receive_json()
        auth_client.put("/api/devices/tv-client", json={"name": "TV"})
        ws.receive_json()  # save broadcast

        assert auth_client.delete("/api/devices/tv-client").status_code == 204
        rows = auth_client.get("/api/devices").json()
        assert all(x["client_id"] != "tv-client" for x in rows)


def test_delete_unknown_device_404(auth_client: TestClient) -> None:
    assert auth_client.delete("/api/devices/nope").status_code == 404


def test_saved_to_standalone_file(auth_client: TestClient) -> None:
    """The list is a real JSON file in the data dir — separate from app.db —
    so it survives a reinstall and an app.db wipe."""
    with auth_client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        _register(ws, "TV", "tv-client")
        ws.receive_json()
        auth_client.put(
            "/api/devices/tv-client",
            json={"name": "Living Room TV", "is_output": True},
        )
        ws.receive_json()

    path = Path(os.environ["DEVICES_FILE"])
    assert path.is_file()
    data = json.loads(path.read_text(encoding="utf-8"))
    assert data["tv-client"]["name"] == "Living Room TV"
    assert data["tv-client"]["is_output"] is True


def test_designation_survives_a_store_reload(auth_client: TestClient) -> None:
    """A fresh store instance (i.e. a server restart) reads the same file and
    sees the designation — it's truly persistent, not in-memory only."""
    from app.devices.store import DeviceStore

    with auth_client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        _register(ws, "TV", "tv-client")
        ws.receive_json()
        auth_client.put("/api/devices/tv-client", json={"name": "TV", "is_output": True})
        ws.receive_json()

    fresh = DeviceStore()
    fresh.load()
    assert fresh.is_output("tv-client") is True
