"""Sync layer: WebSocket auth, snapshot, register, broadcast, action validation.

Covers the foundation. Playback-specific actions (queue, ambient, interrupts,
SFX, scenes) and their tests come in subsequent commits.
"""
from __future__ import annotations

from collections.abc import Iterator
from contextlib import contextmanager

import pytest
from fastapi.testclient import TestClient


@pytest.fixture(autouse=True)
def _reset_sync_state():
    """The state machine, device registry, and connection manager are
    process-wide singletons; reset them between sync tests so leftovers
    from a prior test don't (a) cause writes that match prior values to
    no-op into a hung receive_json or (b) carry stale device entries into
    the next test's snapshot."""
    from app.core.db import SessionLocal
    from app.models.playback_state import PlaybackState
    from app.sync.connection import manager
    from app.sync.devices import registry
    from app.sync.state import machine

    def _reset() -> None:
        machine.reset_for_tests()
        registry.reset_for_tests()
        manager.reset_for_tests()
        with SessionLocal() as db:
            row = db.get(PlaybackState, 1)
            if row is not None:
                row.state_json = {}
                db.commit()

    _reset()
    yield
    _reset()


def _login(client: TestClient) -> None:
    from sqlalchemy import select

    from app.core.db import SessionLocal
    from app.core.security import hash_password
    from app.models.user import User

    USERNAME = "tester"
    PASSWORD = "test-password"

    with SessionLocal() as db:
        existing = db.scalar(select(User).where(User.username == USERNAME))
        if existing is None:
            db.add(User(username=USERNAME, password_hash=hash_password(PASSWORD)))
            db.commit()

    response = client.post(
        "/api/auth/login", json={"username": USERNAME, "password": PASSWORD}
    )
    assert response.status_code == 200, response.text


@contextmanager
def _ws_authed(client: TestClient) -> Iterator:
    _login(client)
    with client.websocket_connect("/api/ws") as ws:
        yield ws


# --- auth -------------------------------------------------------------------


def test_ws_rejects_without_cookie(client: TestClient) -> None:
    from starlette.websockets import WebSocketDisconnect

    with (
        pytest.raises(WebSocketDisconnect) as ei,
        client.websocket_connect("/api/ws"),
    ):
        pass
    assert ei.value.code == 4401


def test_ws_accepts_with_cookie_and_sends_snapshot(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        msg = ws.receive_json()
        assert msg["type"] == "state_snapshot"
        assert "your_device_id" in msg
        assert msg["your_device_id"].startswith("dev-")
        state = msg["state"]
        assert "revision" in state
        assert state["is_playing"] is False
        assert isinstance(state["volume"], int | float)
        assert state["connected_devices"] == []  # not registered yet


# --- register ---------------------------------------------------------------


def test_register_appears_in_state_for_other_clients(client: TestClient) -> None:
    _login(client)
    with client.websocket_connect("/api/ws") as a, client.websocket_connect("/api/ws") as b:
        a.receive_json()  # snapshot for A
        b.receive_json()  # snapshot for B

        a.send_json(
            {"type": "register", "name": "Laptop", "capabilities": ["controls"]}
        )

        # Both A and B should see a state_changed reflecting A's registration.
        msg_a = a.receive_json()
        msg_b = b.receive_json()
        for msg in (msg_a, msg_b):
            assert msg["type"] == "state_changed"
            devices = msg["state"]["connected_devices"]
            assert any(
                d["name"] == "Laptop" and "controls" in d["capabilities"]
                for d in devices
            )


# --- volume / pause / resume ------------------------------------------------


def test_set_volume_broadcasts_new_state(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        snapshot = ws.receive_json()
        prev_revision = snapshot["state"]["revision"]

        ws.send_json({"type": "set_volume", "volume": 0.42})
        msg = ws.receive_json()
        assert msg["type"] == "state_changed"
        assert msg["state"]["volume"] == 0.42
        assert msg["state"]["revision"] == prev_revision + 1


def test_set_volume_no_broadcast_when_unchanged(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        snapshot = ws.receive_json()
        current_volume = snapshot["state"]["volume"]

        ws.send_json({"type": "set_volume", "volume": current_volume})
        # No state_changed message expected. We can't easily assert "nothing
        # arrives" without timeouts, so instead we send a follow-up volume
        # change and assert the next message is the *new* volume.
        ws.send_json({"type": "set_volume", "volume": 0.123})
        msg = ws.receive_json()
        assert msg["state"]["volume"] == 0.123


def test_pause_resume_toggles_is_playing(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()  # snapshot

        ws.send_json({"type": "resume"})
        assert ws.receive_json()["state"]["is_playing"] is True

        ws.send_json({"type": "pause"})
        assert ws.receive_json()["state"]["is_playing"] is False


# --- mode validation -------------------------------------------------------


def test_set_active_mode_validates(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()  # snapshot
        ws.send_json({"type": "set_active_mode", "mode_id": "nope"})
        msg = ws.receive_json()
        assert msg["type"] == "error"
        assert "unknown mode" in msg["detail"]


def test_set_active_mode_accepts_loaded_mode(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()  # snapshot
        ws.send_json({"type": "set_active_mode", "mode_id": "dnd"})
        msg = ws.receive_json()
        assert msg["type"] == "state_changed"
        assert msg["state"]["active_mode_id"] == "dnd"


def test_set_active_mode_persists_across_reconnect(client: TestClient) -> None:
    _login(client)
    with client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        ws.send_json({"type": "set_active_mode", "mode_id": "dnd"})
        ws.receive_json()  # acknowledge

    # Reconnect — snapshot should reflect the persisted mode.
    with client.websocket_connect("/api/ws") as ws2:
        snapshot = ws2.receive_json()
        assert snapshot["state"]["active_mode_id"] == "dnd"


# --- active outputs ---------------------------------------------------------


def test_set_active_outputs_rejects_unregistered_device(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()  # snapshot
        ws.send_json({"type": "set_active_outputs", "device_ids": ["dev-bogus"]})
        msg = ws.receive_json()
        assert msg["type"] == "error"


def test_set_active_outputs_rejects_device_without_audio_output(
    client: TestClient,
) -> None:
    _login(client)
    with client.websocket_connect("/api/ws") as a, client.websocket_connect("/api/ws") as b:
        snap_a = a.receive_json()
        b.receive_json()
        # A registers as controls-only, no audio_output capability.
        a.send_json(
            {"type": "register", "name": "A", "capabilities": ["controls"]}
        )
        a.receive_json()  # state_changed
        b.receive_json()  # state_changed (reg propagates)

        b.send_json(
            {
                "type": "set_active_outputs",
                "device_ids": [snap_a["your_device_id"]],
            }
        )
        msg = b.receive_json()
        assert msg["type"] == "error"


def test_set_active_outputs_accepts_audio_output_device(client: TestClient) -> None:
    _login(client)
    with client.websocket_connect("/api/ws") as a, client.websocket_connect("/api/ws") as b:
        snap_a = a.receive_json()
        b.receive_json()
        a.send_json(
            {"type": "register", "name": "TV", "capabilities": ["audio_output"]}
        )
        a.receive_json()
        b.receive_json()

        b.send_json(
            {
                "type": "set_active_outputs",
                "device_ids": [snap_a["your_device_id"]],
            }
        )
        # Both A and B receive the state_changed.
        msg_a = a.receive_json()
        msg_b = b.receive_json()
        for msg in (msg_a, msg_b):
            assert msg["type"] == "state_changed"
            assert msg["state"]["active_output_device_ids"] == [snap_a["your_device_id"]]


# --- position reports -------------------------------------------------------


def test_position_report_requires_audio_output(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json(
            {"type": "register", "name": "Phone", "capabilities": ["controls"]}
        )
        ws.receive_json()  # state_changed for register

        ws.send_json({"type": "position_report", "position_ms": 1000})
        msg = ws.receive_json()
        assert msg["type"] == "error"


def test_position_report_does_not_broadcast(client: TestClient) -> None:
    _login(client)
    with client.websocket_connect("/api/ws") as a, client.websocket_connect("/api/ws") as b:
        a.receive_json()
        b.receive_json()
        a.send_json(
            {"type": "register", "name": "TV", "capabilities": ["audio_output"]}
        )
        a.receive_json()
        b.receive_json()

        a.send_json({"type": "position_report", "position_ms": 5000})
        # No state_changed should arrive on B as a result. We verify by sending
        # a state-mutating action from A and checking B's next message is THAT
        # change, not a position-report broadcast.
        a.send_json({"type": "set_volume", "volume": 0.999})
        msg_b = b.receive_json()
        assert msg_b["type"] == "state_changed"
        assert msg_b["state"]["volume"] == 0.999


# --- malformed input --------------------------------------------------------


def test_unknown_action_type_returns_error(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "blast_off", "rocket": "v2"})
        msg = ws.receive_json()
        assert msg["type"] == "error"


def test_malformed_payload_returns_error(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "set_volume"})  # missing required 'volume'
        msg = ws.receive_json()
        assert msg["type"] == "error"


# --- ambient lane ----------------------------------------------------------


def test_ambient_play_track(client: TestClient, seeded_track_id: int) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "ambient_play_track", "track_id": seeded_track_id})
        msg = ws.receive_json()
        assert msg["type"] == "state_changed"
        amb = msg["state"]["ambient"]
        assert amb["current_track_id"] == seeded_track_id
        assert amb["queue"] == []
        assert amb["position_ms"] == 0
        assert msg["state"]["is_playing"] is True


def test_ambient_play_track_validates_track_id(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "ambient_play_track", "track_id": 999999})
        msg = ws.receive_json()
        assert msg["type"] == "error"


def test_ambient_set_queue(
    client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "ambient_set_queue", "track_ids": extra_seeded_track_ids})
        msg = ws.receive_json()
        assert msg["state"]["ambient"]["queue"] == extra_seeded_track_ids


def test_ambient_enqueue_appends_and_inserts(
    client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    a, b, c = extra_seeded_track_ids
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "ambient_enqueue", "track_id": a})
        ws.receive_json()
        ws.send_json({"type": "ambient_enqueue", "track_id": c})
        ws.receive_json()
        # Insert b at position 1.
        ws.send_json({"type": "ambient_enqueue", "track_id": b, "position": 1})
        msg = ws.receive_json()
        assert msg["state"]["ambient"]["queue"] == [a, b, c]


def test_ambient_clear_queue(
    client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "ambient_set_queue", "track_ids": extra_seeded_track_ids})
        ws.receive_json()
        ws.send_json({"type": "ambient_clear_queue"})
        msg = ws.receive_json()
        assert msg["state"]["ambient"]["queue"] == []


def test_ambient_skip_next_advances_through_queue(
    client: TestClient, seeded_track_id: int, extra_seeded_track_ids: list[int]
) -> None:
    a, b, _c = extra_seeded_track_ids
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "ambient_play_track", "track_id": seeded_track_id})
        ws.receive_json()
        ws.send_json({"type": "ambient_set_queue", "track_ids": [a, b]})
        ws.receive_json()

        ws.send_json({"type": "ambient_skip_next"})
        msg = ws.receive_json()
        amb = msg["state"]["ambient"]
        assert amb["current_track_id"] == a
        assert amb["queue"] == [b]
        assert amb["history"] == [seeded_track_id]
        assert amb["position_ms"] == 0


def test_ambient_skip_next_at_end_of_queue_loop_off(
    client: TestClient, seeded_track_id: int
) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "ambient_play_track", "track_id": seeded_track_id})
        ws.receive_json()
        ws.send_json({"type": "ambient_skip_next"})
        msg = ws.receive_json()
        amb = msg["state"]["ambient"]
        assert amb["current_track_id"] is None


def test_ambient_skip_next_loop_track_replays(
    client: TestClient, seeded_track_id: int
) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "ambient_play_track", "track_id": seeded_track_id})
        ws.receive_json()
        ws.send_json({"type": "ambient_seek", "position_ms": 30000})
        ws.receive_json()
        ws.send_json({"type": "ambient_set_loop", "loop": "track"})
        ws.receive_json()
        ws.send_json({"type": "ambient_skip_next"})
        msg = ws.receive_json()
        amb = msg["state"]["ambient"]
        assert amb["current_track_id"] == seeded_track_id  # unchanged
        assert amb["position_ms"] == 0  # reset


def test_ambient_skip_next_loop_queue_wraps(
    client: TestClient, seeded_track_id: int, extra_seeded_track_ids: list[int]
) -> None:
    a = extra_seeded_track_ids[0]
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "ambient_play_track", "track_id": seeded_track_id})
        ws.receive_json()
        ws.send_json({"type": "ambient_set_queue", "track_ids": [a]})
        ws.receive_json()
        ws.send_json({"type": "ambient_set_loop", "loop": "queue"})
        ws.receive_json()

        # Skip once: current=a, queue=[], history=[seeded].
        ws.send_json({"type": "ambient_skip_next"})
        ws.receive_json()
        # Skip again at end of queue, loop=queue should wrap.
        ws.send_json({"type": "ambient_skip_next"})
        msg = ws.receive_json()
        amb = msg["state"]["ambient"]
        assert amb["current_track_id"] == seeded_track_id
        assert amb["queue"] == [a]
        assert amb["history"] == []


def test_ambient_skip_prev_with_history(
    client: TestClient, seeded_track_id: int, extra_seeded_track_ids: list[int]
) -> None:
    a = extra_seeded_track_ids[0]
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "ambient_play_track", "track_id": seeded_track_id})
        ws.receive_json()
        ws.send_json({"type": "ambient_set_queue", "track_ids": [a]})
        ws.receive_json()
        ws.send_json({"type": "ambient_skip_next"})
        ws.receive_json()  # now current=a, history=[seeded]

        ws.send_json({"type": "ambient_skip_prev"})
        msg = ws.receive_json()
        amb = msg["state"]["ambient"]
        assert amb["current_track_id"] == seeded_track_id
        assert amb["queue"] == [a]
        assert amb["history"] == []


def test_ambient_skip_prev_no_history_seeks_to_start(
    client: TestClient, seeded_track_id: int
) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "ambient_play_track", "track_id": seeded_track_id})
        ws.receive_json()
        ws.send_json({"type": "ambient_seek", "position_ms": 12345})
        ws.receive_json()
        ws.send_json({"type": "ambient_skip_prev"})
        msg = ws.receive_json()
        assert msg["state"]["ambient"]["current_track_id"] == seeded_track_id
        assert msg["state"]["ambient"]["position_ms"] == 0


def test_ambient_seek(client: TestClient, seeded_track_id: int) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "ambient_play_track", "track_id": seeded_track_id})
        ws.receive_json()
        ws.send_json({"type": "ambient_seek", "position_ms": 42000})
        msg = ws.receive_json()
        assert msg["state"]["ambient"]["position_ms"] == 42000


def test_ambient_set_loop(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        for mode in ("queue", "track", "off"):
            ws.send_json({"type": "ambient_set_loop", "loop": mode})
            msg = ws.receive_json()
            assert msg["state"]["ambient"]["loop"] == mode


def test_ambient_stop_clears_lane(
    client: TestClient, seeded_track_id: int, extra_seeded_track_ids: list[int]
) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "ambient_play_track", "track_id": seeded_track_id})
        ws.receive_json()
        ws.send_json({"type": "ambient_set_queue", "track_ids": extra_seeded_track_ids})
        ws.receive_json()
        ws.send_json({"type": "ambient_stop"})
        msg = ws.receive_json()
        amb = msg["state"]["ambient"]
        assert amb["current_track_id"] is None
        assert amb["queue"] == []
        assert amb["history"] == []
        assert amb["position_ms"] == 0


def test_ambient_play_playlist_manual(
    auth_client, client: TestClient, seeded_track_id: int, extra_seeded_track_ids: list[int]
) -> None:
    # Create a manual playlist with several tracks via the HTTP API.
    pid = auth_client.post(
        "/api/playlists", json={"name": "AmbTest", "source": "manual"}
    ).json()["id"]
    track_ids = [seeded_track_id, *extra_seeded_track_ids]
    for bid in track_ids:
        auth_client.post(f"/api/playlists/{pid}/tracks", json={"track_id": bid})

    with client.websocket_connect("/api/ws") as ws:
        ws.receive_json()  # snapshot
        ws.send_json(
            {"type": "ambient_play_playlist", "playlist_id": pid, "start_index": 1}
        )
        msg = ws.receive_json()
        amb = msg["state"]["ambient"]
        assert amb["current_track_id"] == track_ids[1]
        assert amb["queue"] == track_ids[2:]
        assert amb["history"] == track_ids[:1]
        assert msg["state"]["is_playing"] is True


def test_ambient_play_playlist_404(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "ambient_play_playlist", "playlist_id": 99999})
        msg = ws.receive_json()
        assert msg["type"] == "error"
        assert "playlist not found" in msg["detail"]


# --- mode-scoped session settings -----------------------------------------


def test_set_active_soundboard_requires_active_mode(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "set_active_soundboard", "soundboard_id": "tavern"})
        msg = ws.receive_json()
        assert msg["type"] == "error"
        assert "no active mode" in msg["detail"]


def test_set_active_soundboard_validates_against_active_mode(
    client: TestClient,
) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "set_active_mode", "mode_id": "dnd"})
        ws.receive_json()
        ws.send_json({"type": "set_active_soundboard", "soundboard_id": "nope"})
        msg = ws.receive_json()
        assert msg["type"] == "error"


def test_set_active_soundboard_accepts_known_soundboard(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "set_active_mode", "mode_id": "dnd"})
        ws.receive_json()
        ws.send_json({"type": "set_active_soundboard", "soundboard_id": "tavern"})
        msg = ws.receive_json()
        assert msg["state"]["active_soundboard_id"] == "tavern"


def test_set_active_soundboard_can_clear(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "set_active_mode", "mode_id": "dnd"})
        ws.receive_json()
        ws.send_json({"type": "set_active_soundboard", "soundboard_id": "tavern"})
        ws.receive_json()
        ws.send_json({"type": "set_active_soundboard", "soundboard_id": None})
        msg = ws.receive_json()
        assert msg["state"]["active_soundboard_id"] is None


def test_set_active_presets_validates(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "set_active_presets", "preset_ids": ["nope"]})
        msg = ws.receive_json()
        assert msg["type"] == "error"


def test_set_active_presets_dedupes(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json(
            {"type": "set_active_presets", "preset_ids": ["cave", "radio-vintage", "cave"]}
        )
        msg = ws.receive_json()
        assert msg["state"]["active_preset_ids"] == ["cave", "radio-vintage"]


def test_set_crossfade(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "set_crossfade", "crossfade_ms": 2500})
        msg = ws.receive_json()
        assert msg["state"]["crossfade_ms"] == 2500
        assert msg["state"]["crossfade_type"] == "linear"  # unchanged


def test_set_crossfade_with_type(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json(
            {"type": "set_crossfade", "crossfade_ms": 1500, "crossfade_type": "equal_power"}
        )
        msg = ws.receive_json()
        assert msg["state"]["crossfade_ms"] == 1500
        assert msg["state"]["crossfade_type"] == "equal_power"


def test_set_crossfade_rejects_unknown_type(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json(
            {"type": "set_crossfade", "crossfade_ms": 1000, "crossfade_type": "wobble"}
        )
        msg = ws.receive_json()
        assert msg["type"] == "error"


# --- HTTP-WS state machine integration ------------------------------------


def test_http_set_active_mode_broadcasts_to_ws_clients(
    auth_client, client: TestClient
) -> None:
    """The drift bug fix: HTTP /api/modes/active PUT goes through the sync
    state machine, so connected WS clients see the change immediately."""
    with client.websocket_connect("/api/ws") as ws:
        ws.receive_json()  # snapshot

        auth_client.put("/api/modes/active", json={"mode_id": "dnd"})

        msg = ws.receive_json()
        assert msg["type"] == "state_changed"
        assert msg["state"]["active_mode_id"] == "dnd"


def test_http_set_active_presets_broadcasts_to_ws_clients(
    auth_client, client: TestClient
) -> None:
    with client.websocket_connect("/api/ws") as ws:
        ws.receive_json()

        auth_client.put("/api/presets/active", json={"preset_ids": ["cave"]})

        msg = ws.receive_json()
        assert msg["type"] == "state_changed"
        assert msg["state"]["active_preset_ids"] == ["cave"]


def test_position_report_stamps_ambient_position(
    client: TestClient, seeded_track_id: int
) -> None:
    _login(client)
    with client.websocket_connect("/api/ws") as a, client.websocket_connect("/api/ws") as b:
        a.receive_json()
        b.receive_json()
        a.send_json({"type": "register", "name": "TV", "capabilities": ["audio_output"]})
        a.receive_json()
        b.receive_json()

        b.send_json({"type": "ambient_play_track", "track_id": seeded_track_id})
        a.receive_json()
        b.receive_json()

        a.send_json({"type": "position_report", "position_ms": 12345})
        # No broadcast expected. Trigger a broadcast by changing volume.
        b.send_json({"type": "set_volume", "volume": 0.55})
        a.receive_json()
        msg = b.receive_json()
        assert msg["state"]["ambient"]["position_ms"] == 12345


# --- interrupt lane -------------------------------------------------------


def test_fire_interrupt_track(
    client: TestClient, seeded_track_id: int, extra_seeded_track_ids: list[int]
) -> None:
    interrupt_id = extra_seeded_track_ids[0]
    with _ws_authed(client) as ws:
        ws.receive_json()
        # Set ambient first.
        ws.send_json({"type": "ambient_play_track", "track_id": seeded_track_id})
        ws.receive_json()
        ws.send_json({"type": "ambient_seek", "position_ms": 30000})
        ws.receive_json()

        ws.send_json({"type": "fire_interrupt_track", "track_id": interrupt_id})
        msg = ws.receive_json()
        assert msg["type"] == "state_changed"
        intr = msg["state"]["interrupt"]
        assert intr is not None
        assert intr["current_track_id"] == interrupt_id
        assert intr["queue"] == []
        assert intr["return_to_ambient"] is True
        assert msg["state"]["is_playing"] is True
        # Ambient state preserved.
        assert msg["state"]["ambient"]["current_track_id"] == seeded_track_id
        assert msg["state"]["ambient"]["position_ms"] == 30000


def test_fire_interrupt_track_validates_track_id(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "fire_interrupt_track", "track_id": 999999})
        msg = ws.receive_json()
        assert msg["type"] == "error"


def test_fire_interrupt_replaces_existing_interrupt(
    client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    a, b, _c = extra_seeded_track_ids
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "fire_interrupt_track", "track_id": a})
        ws.receive_json()
        ws.send_json({"type": "fire_interrupt_track", "track_id": b})
        msg = ws.receive_json()
        assert msg["state"]["interrupt"]["current_track_id"] == b


def test_fire_interrupt_playlist_loads_tracks(
    auth_client, client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    pid = auth_client.post(
        "/api/playlists", json={"name": "InterruptTest", "source": "manual"}
    ).json()["id"]
    for bid in extra_seeded_track_ids:
        auth_client.post(f"/api/playlists/{pid}/tracks", json={"track_id": bid})

    with client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        ws.send_json({"type": "fire_interrupt_playlist", "playlist_id": pid})
        msg = ws.receive_json()
        intr = msg["state"]["interrupt"]
        assert intr["current_track_id"] == extra_seeded_track_ids[0]
        assert intr["queue"] == extra_seeded_track_ids[1:]


def test_fire_interrupt_playlist_404(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "fire_interrupt_playlist", "playlist_id": 99999})
        msg = ws.receive_json()
        assert msg["type"] == "error"


def test_interrupt_skip_next_advances_within_queue(
    auth_client, client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    pid = auth_client.post(
        "/api/playlists", json={"name": "InterruptSkipTest", "source": "manual"}
    ).json()["id"]
    for bid in extra_seeded_track_ids:
        auth_client.post(f"/api/playlists/{pid}/tracks", json={"track_id": bid})

    with client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        ws.send_json({"type": "fire_interrupt_playlist", "playlist_id": pid})
        ws.receive_json()
        ws.send_json({"type": "interrupt_skip_next"})
        msg = ws.receive_json()
        intr = msg["state"]["interrupt"]
        assert intr["current_track_id"] == extra_seeded_track_ids[1]
        assert intr["queue"] == extra_seeded_track_ids[2:]
        assert intr["position_ms"] == 0


def test_interrupt_skip_next_at_end_returns_to_ambient(
    client: TestClient, seeded_track_id: int, extra_seeded_track_ids: list[int]
) -> None:
    interrupt_id = extra_seeded_track_ids[0]
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "ambient_play_track", "track_id": seeded_track_id})
        ws.receive_json()
        ws.send_json({"type": "ambient_seek", "position_ms": 45000})
        ws.receive_json()
        ws.send_json({"type": "fire_interrupt_track", "track_id": interrupt_id})
        ws.receive_json()
        ws.send_json({"type": "interrupt_skip_next"})  # auto-completes
        msg = ws.receive_json()
        assert msg["state"]["interrupt"] is None
        # Ambient lane intact, position preserved.
        assert msg["state"]["ambient"]["current_track_id"] == seeded_track_id
        assert msg["state"]["ambient"]["position_ms"] == 45000
        assert msg["state"]["is_playing"] is True


def test_interrupt_skip_next_at_end_no_return_stops_playback(
    client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    interrupt_id = extra_seeded_track_ids[0]
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json(
            {
                "type": "fire_interrupt_track",
                "track_id": interrupt_id,
                "return_to_ambient": False,
            }
        )
        ws.receive_json()
        ws.send_json({"type": "interrupt_skip_next"})  # auto-completes
        msg = ws.receive_json()
        assert msg["state"]["interrupt"] is None
        assert msg["state"]["is_playing"] is False


def test_interrupt_seek(
    client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    interrupt_id = extra_seeded_track_ids[0]
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "fire_interrupt_track", "track_id": interrupt_id})
        ws.receive_json()
        ws.send_json({"type": "interrupt_seek", "position_ms": 8000})
        msg = ws.receive_json()
        assert msg["state"]["interrupt"]["position_ms"] == 8000


def test_cancel_interrupt_returns_to_ambient(
    client: TestClient, seeded_track_id: int, extra_seeded_track_ids: list[int]
) -> None:
    interrupt_id = extra_seeded_track_ids[0]
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "ambient_play_track", "track_id": seeded_track_id})
        ws.receive_json()
        ws.send_json({"type": "ambient_seek", "position_ms": 12000})
        ws.receive_json()
        ws.send_json({"type": "fire_interrupt_track", "track_id": interrupt_id})
        ws.receive_json()
        ws.send_json({"type": "cancel_interrupt"})
        msg = ws.receive_json()
        assert msg["state"]["interrupt"] is None
        assert msg["state"]["ambient"]["position_ms"] == 12000
        assert msg["state"]["is_playing"] is True


def test_cancel_interrupt_no_return_stops_playback(
    client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    interrupt_id = extra_seeded_track_ids[0]
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json(
            {
                "type": "fire_interrupt_track",
                "track_id": interrupt_id,
                "return_to_ambient": False,
            }
        )
        ws.receive_json()
        ws.send_json({"type": "cancel_interrupt"})
        msg = ws.receive_json()
        assert msg["state"]["interrupt"] is None
        assert msg["state"]["is_playing"] is False


def test_position_report_stamps_interrupt_when_active(
    client: TestClient, seeded_track_id: int, extra_seeded_track_ids: list[int]
) -> None:
    """Single-client: same client both reports position and triggers a
    state-changing action so we can read back the broadcast deterministically."""
    interrupt_id = extra_seeded_track_ids[0]
    with _ws_authed(client) as ws:
        ws.receive_json()  # snapshot
        ws.send_json({"type": "register", "name": "TV", "capabilities": ["audio_output"]})
        ws.receive_json()

        ws.send_json({"type": "ambient_play_track", "track_id": seeded_track_id})
        ws.receive_json()
        ws.send_json({"type": "ambient_seek", "position_ms": 25000})
        ws.receive_json()
        ws.send_json({"type": "fire_interrupt_track", "track_id": interrupt_id})
        ws.receive_json()

        # position_report while interrupt is active should stamp the
        # interrupt lane, not ambient.
        ws.send_json({"type": "position_report", "position_ms": 4000})
        # No broadcast for that. Trigger a broadcast and read back state.
        ws.send_json({"type": "set_volume", "volume": 0.77})
        msg = ws.receive_json()
        assert msg["state"]["interrupt"]["position_ms"] == 4000
        # Ambient position preserved as the resume point.
        assert msg["state"]["ambient"]["position_ms"] == 25000


def test_interrupt_actions_are_noops_when_no_interrupt(client: TestClient) -> None:
    """skip_next, seek, cancel during no-interrupt state don't broadcast."""
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "interrupt_skip_next"})
        ws.send_json({"type": "interrupt_seek", "position_ms": 1000})
        ws.send_json({"type": "cancel_interrupt"})
        # No broadcasts. Trigger one with set_volume to check the queue is clean.
        ws.send_json({"type": "set_volume", "volume": 0.111})
        msg = ws.receive_json()
        assert msg["state"]["volume"] == 0.111


# --- SFX firing -----------------------------------------------------------


def test_fire_sfx_requires_active_mode(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json(
            {
                "type": "fire_sfx",
                "soundboard_id": "tavern",
                "item_path": "dnd/door.ogg",
            }
        )
        msg = ws.receive_json()
        assert msg["type"] == "error"
        assert "no active mode" in msg["detail"]


def test_fire_sfx_validates_soundboard(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "set_active_mode", "mode_id": "dnd"})
        ws.receive_json()
        ws.send_json(
            {"type": "fire_sfx", "soundboard_id": "nope", "item_path": "x.ogg"}
        )
        msg = ws.receive_json()
        assert msg["type"] == "error"
        assert "unknown soundboard" in msg["detail"]


def test_fire_sfx_validates_item_path(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "set_active_mode", "mode_id": "dnd"})
        ws.receive_json()
        ws.send_json(
            {
                "type": "fire_sfx",
                "soundboard_id": "tavern",
                "item_path": "nope.ogg",
            }
        )
        msg = ws.receive_json()
        assert msg["type"] == "error"
        assert "not in soundboard" in msg["detail"]


# --- scene activation -----------------------------------------------------


def _make_tavern_playlist(auth_client, track_ids: list[int]) -> int:
    """Create a manual playlist named 'tavern-music' in dnd mode for the
    test scene to resolve to. Returns the playlist id."""
    pid = auth_client.post(
        "/api/playlists",
        json={"name": "tavern-music", "source": "manual", "mode_id": "dnd"},
    ).json()["id"]
    for bid in track_ids:
        auth_client.post(f"/api/playlists/{pid}/tracks", json={"track_id": bid})
    return pid


def test_activate_scene_requires_active_mode(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "activate_scene", "scene_id": "tavern"})
        msg = ws.receive_json()
        assert msg["type"] == "error"
        assert "no active mode" in msg["detail"]


def test_activate_scene_validates_scene_id(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "set_active_mode", "mode_id": "dnd"})
        ws.receive_json()
        ws.send_json({"type": "activate_scene", "scene_id": "nope"})
        msg = ws.receive_json()
        assert msg["type"] == "error"
        assert "unknown scene" in msg["detail"]


def test_activate_scene_validates_playlist_reference(client: TestClient) -> None:
    """The fixture scene references playlist 'tavern-music' which doesn't
    exist by default — error surfaces clearly instead of crashing."""
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "set_active_mode", "mode_id": "dnd"})
        ws.receive_json()
        ws.send_json({"type": "activate_scene", "scene_id": "tavern"})
        msg = ws.receive_json()
        assert msg["type"] == "error"
        assert "no playlist named" in msg["detail"]


def test_activate_scene_composite_apply(
    auth_client, client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    _make_tavern_playlist(auth_client, extra_seeded_track_ids)

    with client.websocket_connect("/api/ws") as ws:
        ws.receive_json()  # snapshot
        ws.send_json({"type": "set_active_mode", "mode_id": "dnd"})
        state_changed = ws.receive_json()
        assert state_changed["state"]["active_mode_id"] == "dnd"

        ws.send_json({"type": "activate_scene", "scene_id": "tavern"})
        # Single state_changed for the composite mutation, then a
        # scene_activated event.
        state_changed = ws.receive_json()
        assert state_changed["type"] == "state_changed"
        s = state_changed["state"]
        assert s["active_scene_id"] == "tavern"
        assert s["active_preset_ids"] == ["radio-vintage"]
        assert s["crossfade_ms"] == 2500
        assert s["ambient"]["current_track_id"] == extra_seeded_track_ids[0]
        assert s["ambient"]["queue"] == extra_seeded_track_ids[1:]
        assert s["is_playing"] is True

        scene_event = ws.receive_json()
        assert scene_event["type"] == "scene_activated"
        assert scene_event["scene_id"] == "tavern"
        assert scene_event["mode_id"] == "dnd"
        assert scene_event["scene"]["name"] == "Stonehill Inn"
        assert scene_event["scene"]["presets"] == ["radio-vintage"]


def test_deactivate_scene_clears_active_scene_id_and_emits_event(
    auth_client, client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    _make_tavern_playlist(auth_client, extra_seeded_track_ids)

    with client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        ws.send_json({"type": "set_active_mode", "mode_id": "dnd"})
        ws.receive_json()
        ws.send_json({"type": "activate_scene", "scene_id": "tavern"})
        ws.receive_json()  # state_changed
        ws.receive_json()  # scene_activated

        ws.send_json({"type": "deactivate_scene"})
        state_changed = ws.receive_json()
        assert state_changed["type"] == "state_changed"
        assert state_changed["state"]["active_scene_id"] is None
        # State that the scene applied (presets, ambient, crossfade) sticks —
        # only the marker is cleared.
        assert state_changed["state"]["active_preset_ids"] == ["radio-vintage"]
        assert state_changed["state"]["crossfade_ms"] == 2500

        deact_event = ws.receive_json()
        assert deact_event["type"] == "scene_deactivated"
        assert deact_event["scene_id"] == "tavern"


def test_deactivate_scene_noop_when_none_active(client: TestClient) -> None:
    with _ws_authed(client) as ws:
        ws.receive_json()
        ws.send_json({"type": "deactivate_scene"})
        # No broadcasts. Trigger one to verify.
        ws.send_json({"type": "set_volume", "volume": 0.333})
        msg = ws.receive_json()
        assert msg["state"]["volume"] == 0.333


def test_fire_sfx_broadcasts_to_audio_output_devices(client: TestClient) -> None:
    """Fire-and-forget: the SFX event reaches audio_output devices but the
    sender (controller) does not get the event echoed back."""
    _login(client)
    with client.websocket_connect("/api/ws") as controller, client.websocket_connect(
        "/api/ws"
    ) as output:
        controller.receive_json()  # snapshot
        output.receive_json()  # snapshot
        controller.send_json(
            {"type": "register", "name": "Phone", "capabilities": ["controls"]}
        )
        controller.receive_json()  # state_changed (register)
        output.receive_json()
        output.send_json(
            {"type": "register", "name": "TV", "capabilities": ["audio_output"]}
        )
        controller.receive_json()
        output.receive_json()

        controller.send_json({"type": "set_active_mode", "mode_id": "dnd"})
        controller.receive_json()  # state_changed
        output.receive_json()

        controller.send_json(
            {
                "type": "fire_sfx",
                "soundboard_id": "tavern",
                "item_path": "dnd/door.ogg",
                "volume": 0.5,
            }
        )
        # Output (audio_output) gets the sfx_fired event.
        msg = output.receive_json()
        assert msg["type"] == "sfx_fired"
        assert msg["soundboard_id"] == "tavern"
        assert msg["item_path"] == "dnd/door.ogg"
        assert msg["volume"] == 0.5

        # Controller did NOT receive sfx_fired. Verify by sending a state-
        # changing action and checking the next message is THAT, not sfx_fired.
        controller.send_json({"type": "set_volume", "volume": 0.42})
        next_msg = controller.receive_json()
        assert next_msg["type"] == "state_changed"
        assert next_msg["state"]["volume"] == 0.42
