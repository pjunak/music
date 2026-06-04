"""Device registry HTTP surface — the operator's visible, editable device
list and the manual audio-output designation that gates activation."""
from __future__ import annotations

import asyncio

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


def test_device_appears_after_register_not_output(auth_client: TestClient) -> None:
    with auth_client.websocket_connect("/api/ws") as ws:
        ws.receive_json()  # snapshot
        _register(ws, "Phone", "phone-1")
        ws.receive_json()  # state_changed

        rows = auth_client.get("/api/devices").json()
        entry = next(r for r in rows if r["client_id"] == "phone-1")
        assert entry["name"] == "Phone"
        assert entry["is_output"] is False  # default — never inferred
        assert entry["connected"] is True


def test_patch_rename(auth_client: TestClient) -> None:
    with auth_client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        _register(ws, "Old", "c1")
        ws.receive_json()

        r = auth_client.patch("/api/devices/c1", json={"name": "Living Room TV"})
        assert r.status_code == 200, r.text
        assert r.json()["name"] == "Living Room TV"
        ws.receive_json()  # rename broadcast

        rows = auth_client.get("/api/devices").json()
        assert next(x for x in rows if x["client_id"] == "c1")["name"] == "Living Room TV"


def test_patch_is_output_enables_activation(auth_client: TestClient) -> None:
    with auth_client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        _register(ws, "TV", "tv-client")
        ws.receive_json()

        # Not designated yet → activation rejected.
        ws.send_json({"type": "set_active_outputs", "device_ids": ["tv-client"]})
        assert ws.receive_json()["type"] == "error"

        # Designate via the device list.
        r = auth_client.patch("/api/devices/tv-client", json={"is_output": True})
        assert r.status_code == 200
        assert r.json()["is_output"] is True
        ws.receive_json()  # turn-on broadcast

        # Now activation succeeds.
        ws.send_json({"type": "set_active_outputs", "device_ids": ["tv-client"]})
        msg = ws.receive_json()
        assert msg["type"] == "state_changed"
        assert msg["state"]["active_output_device_ids"] == ["tv-client"]


def test_patch_is_output_off_removes_active(auth_client: TestClient) -> None:
    with auth_client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        _register(ws, "TV", "tv-client")
        ws.receive_json()
        auth_client.patch("/api/devices/tv-client", json={"is_output": True})
        ws.receive_json()  # turn-on broadcast
        ws.send_json({"type": "set_active_outputs", "device_ids": ["tv-client"]})
        assert ws.receive_json()["state"]["active_output_device_ids"] == ["tv-client"]

        # De-designate while live-active → immediately dropped from outputs.
        auth_client.patch("/api/devices/tv-client", json={"is_output": False})
        msg = ws.receive_json()
        assert "tv-client" not in msg["state"]["active_output_device_ids"]


def test_delete_device(auth_client: TestClient) -> None:
    with auth_client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        _register(ws, "Temp", "c1")
        ws.receive_json()

        assert auth_client.delete("/api/devices/c1").status_code == 204
        rows = auth_client.get("/api/devices").json()
        assert all(r["client_id"] != "c1" for r in rows)


def test_patch_unknown_device_404(auth_client: TestClient) -> None:
    assert auth_client.patch("/api/devices/nope", json={"name": "x"}).status_code == 404


def test_delete_unknown_device_404(auth_client: TestClient) -> None:
    assert auth_client.delete("/api/devices/nope").status_code == 404


def test_designation_persists_across_reload(auth_client: TestClient) -> None:
    """The output designation lives in the persistent table, so it survives a
    server restart — while active outputs are wiped on boot (fully-manual)."""
    from app.core.db import SessionLocal
    from app.sync.state import StateMachine

    with auth_client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        _register(ws, "TV", "tv-client")
        ws.receive_json()
        auth_client.patch("/api/devices/tv-client", json={"is_output": True})
        ws.receive_json()  # broadcast

    machine = StateMachine()
    asyncio.run(machine.load(SessionLocal))
    snap = asyncio.run(machine.snapshot())
    assert snap.active_output_device_ids == []  # wiped on boot

    rows = auth_client.get("/api/devices").json()
    assert next(r for r in rows if r["client_id"] == "tv-client")["is_output"] is True
