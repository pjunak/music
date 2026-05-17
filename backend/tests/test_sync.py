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


def test_ws_accepts_guest_connection_read_only(client: TestClient) -> None:
    """Without a cookie the socket connects in guest mode: it gets the
    state snapshot like any other client, but mutating actions are
    rejected with an `error` message instead of being applied."""
    with client.websocket_connect("/api/ws") as ws:
        snap = ws.receive_json()
        assert snap["type"] == "state_snapshot"

        ws.send_json({"type": "set_volume", "volume": 0.5})
        err = ws.receive_json()
        assert err["type"] == "error"
        assert "guest" in err["detail"].lower()


def test_ws_session_expiry_mid_connection_downgrades_to_guest(
    client: TestClient,
) -> None:
    """Cookie that's valid at WS upgrade but expires mid-connection should
    cause the next mutation to be rejected with a "session expired" error.
    Without this, a long-lived tab whose cookie expired could keep
    mutating state indefinitely."""
    import time
    from datetime import UTC, datetime, timedelta

    from sqlalchemy import update

    from app.core.db import SessionLocal
    from app.models.auth_session import AuthSession

    _login(client)
    token = client.cookies.get("music_session")
    assert token is not None

    # Shorten the freshly-minted session's expiry to 0.5s from now. At
    # WS-upgrade time, `_authenticate` captures this expiry into a
    # connection-local variable. The per-action check compares
    # `datetime.now(UTC)` against the captured value — sleeping past
    # the boundary exercises that path.
    soon = datetime.now(UTC) + timedelta(milliseconds=500)
    with SessionLocal() as db:
        db.execute(
            update(AuthSession)
            .where(AuthSession.token == token)
            .values(expires_at=soon)
        )
        db.commit()

    with client.websocket_connect("/api/ws") as ws:
        ws.receive_json()  # snapshot — connection authenticated normally

        # Wait past the captured expiry. 0.8s leaves plenty of margin
        # over the 0.5s deadline without inflating the suite runtime.
        time.sleep(0.8)

        # First mutation after expiry: should hit the new per-action
        # re-check and surface "session expired".
        ws.send_json({"type": "set_volume", "volume": 0.5})
        err = ws.receive_json()
        assert err["type"] == "error"
        assert "expired" in err["detail"].lower()

        # Subsequent mutations take the standard guest-rejection path —
        # the connection is now downgraded for the rest of its life.
        ws.send_json({"type": "set_volume", "volume": 0.7})
        err2 = ws.receive_json()
        assert err2["type"] == "error"
        assert "guest" in err2["detail"].lower()


def test_ws_guest_can_register_to_appear_in_outputs(client: TestClient) -> None:
    """Register is the one mutation a guest is allowed — so a logged-out
    Player tab on a TV can show up in the operator's Outputs picker."""
    with client.websocket_connect("/api/ws") as guest:
        guest.receive_json()  # snapshot
        guest.send_json(
            {
                "type": "register",
                "name": "TV",
                "capabilities": ["audio_output"],
            }
        )
        # Register is broadcast as a state_changed; receive it.
        msg = guest.receive_json()
        assert msg["type"] == "state_changed"
        names = [d["name"] for d in msg["state"]["connected_devices"]]
        assert "TV" in names


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


def test_set_active_outputs_does_not_re_validate_existing_ids(
    client: TestClient,
) -> None:
    """Stale device ids that are already in active_output_device_ids must
    not block the operator's next toggle. We only validate ids being
    *added* to the list."""
    _login(client)
    with client.websocket_connect("/api/ws") as a, client.websocket_connect("/api/ws") as b:
        snap_a = a.receive_json()
        b.receive_json()
        a.send_json(
            {"type": "register", "name": "A", "capabilities": ["audio_output"]}
        )
        a.receive_json()
        b.receive_json()
        b.send_json(
            {"type": "register", "name": "B", "capabilities": ["audio_output"]}
        )
        a.receive_json()
        b.receive_json()

        # Set A as the only active output.
        b.send_json(
            {"type": "set_active_outputs", "device_ids": [snap_a["your_device_id"]]}
        )
        a.receive_json()
        b.receive_json()

    # `a` socket closes; its device id is now stale in active_output_device_ids
    # (the disconnect-prune state_changed will land on `b` before this block ends).
    # `b` then logs in fresh and tries to add itself — the request will include
    # both the stale id (from current state) and the new id (b). The server must
    # not reject because of the stale one.

    with client.websocket_connect("/api/ws") as fresh:
        snap_fresh = fresh.receive_json()
        fresh.send_json(
            {"type": "register", "name": "Fresh", "capabilities": ["audio_output"]}
        )
        fresh.receive_json()
        # Whatever's already in active_output_device_ids stays; we just add ourselves.
        existing = list(snap_fresh["state"]["active_output_device_ids"])
        fresh.send_json(
            {
                "type": "set_active_outputs",
                "device_ids": [*existing, snap_fresh["your_device_id"]],
            }
        )
        # Either we get a state_changed (success) — but never an error.
        msg = fresh.receive_json()
        assert msg["type"] == "state_changed"
        assert snap_fresh["your_device_id"] in msg["state"]["active_output_device_ids"]


def test_state_load_prunes_dangling_track_ids(
    auth_client: TestClient,
    seeded_track_id: int,
) -> None:
    """A `current_track_id` referencing a track that's no longer in the
    DB must be cleared on state machine reload. Otherwise the audio
    engine fetches `/api/library/tracks/{stale}/stream` and gets 404.
    Same rule for queue / history / interrupt."""
    from app.core.db import SessionLocal
    from app.domain import playback_state as playback_state_domain
    from app.sync.state import StateMachine

    # Seed a known-bad state directly into the DB.
    with SessionLocal() as db:
        playback_state_domain.update_state(
            db,
            ambient={
                "current_track_id": 999_999,  # doesn't exist
                "queue": [seeded_track_id, 999_998],  # mix valid + invalid
                "history": [999_997],
                "position_ms": 0,
                "loop": "off",
            },
            interrupt={
                "current_track_id": 999_996,
                "queue": [],
                "position_ms": 0,
                "return_to_ambient": True,
                "fade_in_ms": 0,
                "fade_out_ms": 0,
            },
            active_mode_id="ghost-mode",
            active_soundboard_id="ghost-sb",
            active_scene_id="ghost-scene",
            active_preset_ids=["cave", "nope-preset"],
            active_output_device_ids=["dev-stale-1", "dev-stale-2"],
        )

    machine = StateMachine()
    import asyncio

    asyncio.run(machine.load(SessionLocal))
    snap = asyncio.run(machine.snapshot())

    assert snap.ambient.current_track_id is None
    assert snap.ambient.queue == [seeded_track_id]
    assert snap.ambient.history == []
    assert snap.interrupt is None
    assert snap.active_mode_id is None
    assert snap.active_soundboard_id is None
    assert snap.active_scene_id is None
    assert "cave" in snap.active_preset_ids
    assert "nope-preset" not in snap.active_preset_ids
    assert snap.active_output_device_ids == []


def test_state_load_prunes_dangling_pre_scene_snapshot(
    auth_client: TestClient,
    seeded_track_id: int,
) -> None:
    """The scene-revert snapshot stores a captured ambient lane. Track
    rows referenced there can disappear too — same dangling-ref hazard as
    the live ambient field, so the loader must prune both in lockstep."""
    from app.core.db import SessionLocal
    from app.domain import playback_state as playback_state_domain
    from app.sync.state import StateMachine

    with SessionLocal() as db:
        playback_state_domain.update_state(
            db,
            pre_scene_state={
                "ambient": {
                    "current_track_id": 999_001,
                    "queue": [seeded_track_id, 999_002],
                    "history": [999_003],
                    "position_ms": 12000,
                    "loop": "off",
                },
                "crossfade_ms": 1500,
                "active_preset_ids": ["cave", "phantom-preset"],
            },
        )

    machine = StateMachine()
    import asyncio

    asyncio.run(machine.load(SessionLocal))
    snap = asyncio.run(machine.snapshot())
    assert snap.pre_scene_state is not None
    assert snap.pre_scene_state.ambient is not None
    assert snap.pre_scene_state.ambient.current_track_id is None
    assert snap.pre_scene_state.ambient.queue == [seeded_track_id]
    assert snap.pre_scene_state.ambient.history == []
    assert snap.pre_scene_state.crossfade_ms == 1500
    assert "phantom-preset" not in snap.pre_scene_state.active_preset_ids
    assert "cave" in snap.pre_scene_state.active_preset_ids


def test_state_load_after_track_deletion_clears_active_track(
    auth_client: TestClient, client: TestClient
) -> None:
    """Regression for the audio-playback bug: a track playing in ambient
    that gets deleted via the library API would leave a stale id behind
    in playback_state. On the next state load (server restart), the audio
    engine would fetch its stream URL and surface as SRC_NOT_SUPPORTED.
    The load-time prune defends against that."""
    import asyncio
    import io
    import struct

    from app.core.db import SessionLocal
    from app.sync.state import StateMachine

    def _wav() -> bytes:
        sr = 8000
        pcm = b"\x00\x00" * 4000
        h = b"RIFF" + struct.pack("<I", 36 + len(pcm)) + b"WAVE"
        h += b"fmt " + struct.pack("<I", 16)
        h += struct.pack("<H", 1) + struct.pack("<H", 1)
        h += struct.pack("<I", sr) + struct.pack("<I", sr * 2)
        h += struct.pack("<H", 2) + struct.pack("<H", 16)
        h += b"data" + struct.pack("<I", len(pcm))
        return h + pcm

    upload = auth_client.post(
        "/api/library/upload",
        files=[("files", ("doomed-playback.wav", _wav(), "audio/wav"))],
        params={"dest": "DoomedPlay"},
    ).json()
    track_id = upload["saved"][0]["id"]

    # Start playing it via WS so the persisted state references it.
    with client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        ws.send_json({"type": "ambient_play_track", "track_id": track_id})
        ws.receive_json()

    # User deletes the file from the library while we're "off".
    delete_resp = auth_client.delete(f"/api/library/tracks/{track_id}")
    assert delete_resp.status_code == 204

    # Reload as if the server just restarted.
    machine = StateMachine()
    asyncio.run(machine.load(SessionLocal))
    snap = asyncio.run(machine.snapshot())
    assert snap.ambient.current_track_id is None
    _ = io  # keep import for potential future test growth


def test_remove_active_output_mutator() -> None:
    """Unit-test the mutator the WS disconnect path uses to prune stale
    device ids. (A full WS-disconnect integration test had timing issues
    with the TestClient that weren't worth the test's value.)"""
    from app.sync.protocol import PlayerState
    from app.sync.state import remove_active_output

    initial = PlayerState(active_output_device_ids=["dev-keep", "dev-drop"])
    mutated = remove_active_output("dev-drop")(initial)
    assert mutated.active_output_device_ids == ["dev-keep"]

    # No-op when the id isn't present — returns the same instance so the
    # state machine knows to skip persistence + broadcast.
    same = remove_active_output("dev-not-present")(initial)
    assert same is initial


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


def test_deactivate_scene_reverts_overwritten_state_and_emits_event(
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
        s = state_changed["state"]
        assert s["active_scene_id"] is None
        # Fields the scene overwrote (presets, ambient, crossfade) revert
        # to the values captured at activation time.
        assert s["active_preset_ids"] == []
        assert s["crossfade_ms"] == 0
        assert s["ambient"]["current_track_id"] is None
        assert s["pre_scene_state"] is None

        deact_event = ws.receive_json()
        assert deact_event["type"] == "scene_deactivated"
        assert deact_event["scene_id"] == "tavern"


def test_deactivate_scene_preserves_unrelated_state(
    auth_client, client: TestClient, seeded_track_id: int, extra_seeded_track_ids: list[int]
) -> None:
    """User changes that happen *after* a scene activates (e.g. setting
    volume, picking a different soundboard) are not reverted on
    deactivation — only the fields the scene itself overwrote are."""
    _make_tavern_playlist(auth_client, extra_seeded_track_ids)

    with client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        ws.send_json({"type": "set_active_mode", "mode_id": "dnd"})
        ws.receive_json()
        ws.send_json({"type": "activate_scene", "scene_id": "tavern"})
        ws.receive_json()
        ws.receive_json()
        ws.send_json({"type": "set_volume", "volume": 0.31})
        ws.receive_json()
        ws.send_json({"type": "set_active_soundboard", "soundboard_id": "tavern"})
        ws.receive_json()

        ws.send_json({"type": "deactivate_scene"})
        s = ws.receive_json()["state"]
        assert s["volume"] == 0.31
        assert s["active_soundboard_id"] == "tavern"


def test_chained_scene_deactivate_unwinds_to_original(
    auth_client, client: TestClient, seeded_track_id: int, extra_seeded_track_ids: list[int]
) -> None:
    """Activating a second scene while one is already active must not
    overwrite the original snapshot — deactivating goes back to the
    state before the *first* scene, not the intermediate one."""
    _make_tavern_playlist(auth_client, extra_seeded_track_ids)

    with client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        ws.send_json({"type": "set_active_presets", "preset_ids": ["cave"]})
        ws.receive_json()
        ws.send_json({"type": "set_crossfade", "crossfade_ms": 500})
        ws.receive_json()
        ws.send_json({"type": "set_active_mode", "mode_id": "dnd"})
        ws.receive_json()

        ws.send_json({"type": "activate_scene", "scene_id": "tavern"})
        ws.receive_json()
        ws.receive_json()
        # Re-activating the same scene replaces the active scene marker
        # but the snapshot from the first activation is preserved.
        ws.send_json({"type": "activate_scene", "scene_id": "tavern"})
        ws.receive_json()
        ws.receive_json()

        ws.send_json({"type": "deactivate_scene"})
        s = ws.receive_json()["state"]
        assert s["active_preset_ids"] == ["cave"]
        assert s["crossfade_ms"] == 500


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


# --- HTTP polling fallback (/api/sync/state) -------------------------------


def test_http_state_returns_snapshot_for_guest(client: TestClient) -> None:
    """The HTTP fallback mirrors the WS endpoint's guest-friendly auth
    contract — a logged-out TV bookmark can read state and play whatever
    the controller queued up. This is the whole point of the endpoint:
    smart-TV browsers that can't establish a wss:// handshake (cert
    re-validation refuses the user's page-level exception) hit this via
    XHR instead. Symmetry with `test_ws_accepts_guest_connection_read_only`."""
    response = client.get("/api/sync/state")
    assert response.status_code == 200, response.text
    payload = response.json()
    # Same shape as the WS state_snapshot.state — schema_revision and
    # the volume default prove we got a real PlayerState, not an error
    # page rewritten to JSON.
    assert "revision" in payload
    assert "volume" in payload
    assert "ambient" in payload


def test_http_state_returns_snapshot_for_authed_user(client: TestClient) -> None:
    """Same endpoint, with a session cookie. Should return the same shape
    as the guest path — OptionalUser doesn't distinguish in the response."""
    _login(client)
    response = client.get("/api/sync/state")
    assert response.status_code == 200, response.text
    payload = response.json()
    assert "revision" in payload
    assert "ambient" in payload


def test_http_state_reflects_ws_mutations(
    client: TestClient, seeded_track_id: int
) -> None:
    """The HTTP endpoint reads the same in-memory StateMachine the WS
    writes to, so a mutation pushed over WS is immediately visible via
    GET /api/sync/state. This is what makes the polling fallback work:
    the TV polls every 2s and sees state changes driven by the controller
    on another device."""
    with _ws_authed(client) as ws:
        ws.receive_json()  # initial snapshot
        ws.send_json({"type": "ambient_play_track", "track_id": seeded_track_id})
        ws.receive_json()  # state_changed broadcast

        # Read via HTTP — should see the same current_track_id.
        response = client.get("/api/sync/state")
        assert response.status_code == 200
        payload = response.json()
        assert payload["ambient"]["current_track_id"] == seeded_track_id
        assert payload["is_playing"] is True
