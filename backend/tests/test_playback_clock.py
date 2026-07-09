"""Server-side playback clock, position_epoch, skip idempotency, and the
end-of-track advancer (docs/playback-sync-overhaul.md phases 1-2)."""
from __future__ import annotations

import asyncio
import time
from typing import Any

import pytest
from fastapi.testclient import TestClient

# App imports happen lazily (inside fixtures/helpers): pytest imports test
# modules at collection time, BEFORE the session-scoped env fixture in
# conftest points settings at the tmp dir — a module-level `from app...`
# import would build Settings against the real environment and fail.


class _Lazy:
    """Attribute-lazy façade over the app modules (import-on-first-use)."""

    def __getattr__(self, name: str):
        # Collection hooks (anyio's pytest plugin) getattr() every module-
        # level object for dunders like __test__. Importing app modules that
        # early would bind app.core.db's engine before the test env exists —
        # bail out before the import, not after.
        if name.startswith("__"):
            raise AttributeError(name)
        from app.sync import protocol, state

        for mod in (state, protocol):
            if hasattr(mod, name):
                value = getattr(mod, name)
                setattr(self, name, value)
                return value
        raise AttributeError(name)


L = _Lazy()


@pytest.fixture(autouse=True)
def _reset_sync_state():
    from tests.conftest import reset_sync_singletons

    reset_sync_singletons()
    yield
    reset_sync_singletons()


def _playing_state(position_ms: int = 10_000, anchored_at: float | None = None) -> Any:
    return L.PlayerState(
        is_playing=True,
        ambient=L.AmbientState(
            current_track_id=1,
            queue=[2],
            position_ms=position_ms,
            position_anchored_at=anchored_at,
        ),
    )


# --- materialization ---------------------------------------------------------


def test_materialize_advances_a_ticking_lane() -> None:
    t0 = time.time()
    state = _playing_state(position_ms=10_000, anchored_at=t0)
    out = L.materialize_positions(state, now=t0 + 5.0)
    assert out.ambient.position_ms == 15_000
    assert out.ambient.position_anchored_at == t0 + 5.0
    # The input state is untouched (materialization is a read-path copy).
    assert state.ambient.position_ms == 10_000


def test_materialize_leaves_a_frozen_lane_alone() -> None:
    state = _playing_state(position_ms=10_000, anchored_at=None)
    out = L.materialize_positions(state, now=time.time() + 60)
    assert out is state


def test_pause_freezes_clock_and_resume_restarts_it() -> None:
    t0 = time.time()
    state = _playing_state(position_ms=10_000, anchored_at=t0 - 2.0)
    paused = L.set_is_playing(False)(state)
    # ~2s of ticking folded into the base; clock stopped; NOT a seek.
    assert paused.ambient.position_anchored_at is None
    assert abs(paused.ambient.position_ms - 12_000) < 500
    assert paused.position_epoch == state.position_epoch

    resumed = L.set_is_playing(True)(paused)
    assert resumed.ambient.position_anchored_at is not None
    assert resumed.ambient.position_ms == paused.ambient.position_ms
    assert resumed.position_epoch == state.position_epoch


def test_report_position_rebases_without_epoch_bump() -> None:
    t0 = time.time()
    state = _playing_state(position_ms=10_000, anchored_at=t0 - 30.0)
    out = L.report_position("dev", 39_500)(state)
    assert out.ambient.position_ms == 39_500
    assert out.ambient.position_anchored_at is not None  # still ticking
    assert out.position_epoch == state.position_epoch


# --- position_epoch bump matrix ----------------------------------------------


def test_deliberate_moves_bump_epoch_and_bookkeeping_does_not() -> None:
    state = _playing_state()

    bumps = [
        L.ambient_play_track(1),
        L.ambient_seek(5_000),
        L.ambient_skip_next(),
        L.ambient_stop(),
        L.fire_interrupt_track(2, True, 0, 0),
    ]
    for mut in bumps:
        out = mut(state)
        assert out.position_epoch == state.position_epoch + 1, mut

    no_bumps = [
        L.set_volume(0.5),
        L.set_is_playing(False),
        L.report_position("dev", 11_000),
    ]
    for mut in no_bumps:
        out = mut(state)
        assert out.position_epoch == state.position_epoch, mut


def test_interrupt_lifecycle_bumps_epoch_each_transition() -> None:
    state = _playing_state()
    fired = L.fire_interrupt_track(2, True, 0, 0)(state)
    ended = L.cancel_interrupt()(fired)
    assert fired.position_epoch == state.position_epoch + 1
    assert ended.position_epoch == fired.position_epoch + 1
    assert ended.interrupt is None


# --- interrupt / ambient clock interaction -------------------------------------


def test_pausing_interrupt_freezes_ambient_and_end_resumes_it() -> None:
    t0 = time.time()
    state = _playing_state(position_ms=30_000, anchored_at=t0 - 1.0)
    fired = L.fire_interrupt_track(2, True, 0, 0, duck_to=None)(state)
    # Ambient frozen at its materialized position; interrupt ticking from 0.
    assert fired.ambient.position_anchored_at is None
    assert abs(fired.ambient.position_ms - 31_000) < 500
    assert fired.interrupt is not None
    assert fired.interrupt.position_anchored_at is not None

    ended = L.cancel_interrupt()(fired)
    assert ended.interrupt is None
    # Ambient clock restarts from the preserved base.
    assert ended.ambient.position_anchored_at is not None
    assert ended.ambient.position_ms == fired.ambient.position_ms


def test_ducking_interrupt_keeps_ambient_ticking() -> None:
    t0 = time.time()
    state = _playing_state(position_ms=30_000, anchored_at=t0)
    fired = L.fire_interrupt_track(2, True, 0, 0, duck_to=0.3)(state)
    assert fired.ambient.position_anchored_at is not None  # still ticking


# --- skip idempotency ----------------------------------------------------------


def test_skip_with_stale_token_is_a_no_op() -> None:
    state = _playing_state()  # current=1, queue=[2]
    first = L.ambient_skip_next(expected_track_id=1)(state)
    assert first.ambient.current_track_id == 2

    # A second output's duplicate `ended` for track 1 arrives late.
    second = L.ambient_skip_next(expected_track_id=1)(first)
    assert second is first  # dropped — no double-advance


def test_duplicate_end_of_queue_skips_cannot_stop_playback() -> None:
    """The old bug: with two outputs and loop off, the duplicate skip at the
    final queue entry ran 'queue empty → stop' and killed playback although
    the queue had a song left."""
    state = L.PlayerState(
        is_playing=True,
        ambient=L.AmbientState(current_track_id=1, queue=[2], loop="off"),
    )
    first = L.ambient_skip_next(expected_track_id=1)(state)
    assert first.ambient.current_track_id == 2
    assert first.is_playing is True

    second = L.ambient_skip_next(expected_track_id=1)(first)
    assert second is first
    assert second.is_playing is True  # track 2 actually gets to play


def test_manual_skip_without_token_always_applies() -> None:
    state = _playing_state()
    out = L.ambient_skip_next()(state)
    assert out.ambient.current_track_id == 2


# --- boot behavior --------------------------------------------------------------


def test_boot_never_auto_resumes(client: TestClient, seeded_track_id: int) -> None:
    """A restart freezes the world: is_playing off, interrupt cleared, ambient
    track + position preserved (base only, anchor cleared)."""
    from app.core.db import SessionLocal
    from app.domain import playback_state as playback_state_domain
    from app.sync.state import StateMachine

    with SessionLocal() as db:
        playback_state_domain.update_state(
            db,
            is_playing=True,
            position_epoch=7,
            ambient={
                "current_track_id": seeded_track_id,
                "queue": [],
                "history": [],
                "position_ms": 42_000,
                "position_anchored_at": time.time() - 3600,  # stale by an hour
                "loop": "off",
                "shuffle": "off",
                "source_playlist_id": None,
            },
            interrupt={
                "current_track_id": seeded_track_id,
                "queue": [],
                "position_ms": 5_000,
                "position_anchored_at": time.time(),
                "return_to_ambient": True,
                "fade_in_ms": 0,
                "fade_out_ms": 0,
                "duck_to": None,
            },
        )

    machine = StateMachine()
    asyncio.run(machine.load(SessionLocal))
    snap = asyncio.run(machine.snapshot())
    assert snap.is_playing is False
    assert snap.interrupt is None
    assert snap.ambient.current_track_id == seeded_track_id
    # Stored base survives; the hour of downtime does NOT get materialized in.
    assert snap.ambient.position_ms == 42_000
    assert snap.ambient.position_anchored_at is None


# --- guest telemetry -------------------------------------------------------------


def test_guest_active_output_may_report_position(
    client: TestClient, seeded_track_id: int
) -> None:
    """A guest socket (compat-mode device) that the operator toggled on as an
    output may send position reports — and they land in the ambient lane."""
    from tests.test_sync import _login

    _login(client)
    with client.websocket_connect("/api/ws") as operator:
        operator.receive_json()  # snapshot
        # WS auth happens at upgrade time, so the already-open operator socket
        # stays authenticated; clearing the jar makes the NEXT connect a guest.
        client.cookies.clear()
        with client.websocket_connect("/api/ws") as guest:
            guest.receive_json()  # snapshot
            guest.send_json(
                {"type": "register", "name": "Old TV", "client_id": "tv-guest"}
            )
            guest.receive_json()  # register broadcast
            operator.receive_json()

            operator.send_json(
                {"type": "ambient_play_track", "track_id": seeded_track_id}
            )
            operator.receive_json()
            guest.receive_json()

            operator.send_json(
                {"type": "set_active_outputs", "device_ids": ["tv-guest"]}
            )
            operator.receive_json()
            guest.receive_json()

            guest.send_json({"type": "position_report", "position_ms": 123_000})
            # Reports are silent; force a broadcast to read the state back.
            operator.send_json({"type": "set_volume", "volume": 0.42})
            msg = operator.receive_json()
            assert msg["type"] == "state_changed"
            report = msg["state"]["last_position_report"]
            assert report is not None and report["device_id"] == "tv-guest"
            assert abs(msg["state"]["ambient"]["position_ms"] - 123_000) < 1000

            # Reading the guest's frames must show no rejection.
            frame = guest.receive_json()
            assert frame["type"] != "error"


def test_guest_snapshot_hides_other_devices(
    client: TestClient, seeded_track_id: int
) -> None:
    """A guest must not learn other devices' client_ids or the global active-
    output set. Those ids are the capability tokens that gate output activation
    and position reports; leaking them (the connect snapshot did) let an
    anonymous socket re-register under an active output's id and hijack the
    shared playback clock. The guest still sees ITS OWN active membership so it
    can decide whether to play."""
    from tests.test_sync import _login

    _login(client)
    with client.websocket_connect("/api/ws") as operator:
        operator.receive_json()  # snapshot (authed → full)
        client.cookies.clear()  # every subsequent connect is a guest
        with client.websocket_connect("/api/ws") as tv:
            tv.receive_json()  # redacted snapshot
            tv.send_json(
                {"type": "register", "name": "Real TV", "client_id": "secret-tv-id"}
            )
            tv.receive_json()
            operator.receive_json()

            # The already-open operator socket stayed authed (WS auth is at
            # upgrade). Activate the TV as an output.
            operator.send_json(
                {"type": "set_active_outputs", "device_ids": ["secret-tv-id"]}
            )
            op_frame = operator.receive_json()
            tv_frame = tv.receive_json()

            # Operator (authed) sees the full picture.
            assert "secret-tv-id" in op_frame["state"]["active_output_device_ids"]
            assert any(
                d["client_id"] == "secret-tv-id"
                for d in op_frame["state"]["connected_devices"]
            )

            # The TV guest sees ONLY its own membership and no device roster.
            assert tv_frame["state"]["active_output_device_ids"] == ["secret-tv-id"]
            assert tv_frame["state"]["connected_devices"] == []

            # A fresh attacker guest learns nothing it could spoof.
            with client.websocket_connect("/api/ws") as attacker:
                snap = attacker.receive_json()
                assert snap["type"] == "state_snapshot"
                st = snap["state"]
                assert st["connected_devices"] == []
                assert st["active_output_device_ids"] == []


def test_guest_still_cannot_mutate(client: TestClient) -> None:
    with client.websocket_connect("/api/ws") as guest:
        guest.receive_json()
        guest.send_json({"type": "pause"})
        err = guest.receive_json()
        assert err["type"] == "error"
        assert "guest" in err["detail"].lower()


# --- snapshot endpoint carries a live clock ---------------------------------------


def test_polled_state_positions_advance_between_polls(
    client: TestClient, seeded_track_id: int
) -> None:
    from tests.test_sync import _login

    _login(client)
    with client.websocket_connect("/api/ws") as ws:
        ws.receive_json()
        ws.send_json({"type": "ambient_play_track", "track_id": seeded_track_id})
        ws.receive_json()

        first = client.get("/api/sync/state").json()["ambient"]["position_ms"]
        time.sleep(0.15)
        second = client.get("/api/sync/state").json()["ambient"]["position_ms"]
        assert second > first  # the clock ticks with no position reports at all


# --- advancer ---------------------------------------------------------------------


def test_advancer_plan_targets_the_soonest_ending_lane() -> None:
    from app.sync.advancer import GRACE_S, Advancer

    adv = Advancer()

    async def fake_length(track_id: int) -> float | None:
        return {1: 100.0, 2: 30.0}.get(track_id)

    adv._track_length_s = fake_length  # type: ignore[method-assign]

    now = time.time()
    snap = L.PlayerState(
        is_playing=True,
        ambient=L.AmbientState(
            current_track_id=1, position_ms=95_000, position_anchored_at=now
        ),
        interrupt=L.InterruptState(
            current_track_id=2, position_ms=29_000, position_anchored_at=now
        ),
    )
    plan = asyncio.run(adv._plan(snap))
    assert plan is not None
    remaining, lane, track_id = plan
    # Interrupt ends in ~1s, ambient in ~5s — interrupt wins.
    assert lane == "interrupt" and track_id == 2
    assert abs(remaining - (1.0 + GRACE_S)) < 0.3


def test_advancer_plan_skips_frozen_and_unknown_length_lanes() -> None:
    from app.sync.advancer import Advancer

    adv = Advancer()

    async def fake_length(track_id: int) -> float | None:
        return None  # unknown duration

    adv._track_length_s = fake_length  # type: ignore[method-assign]

    frozen = L.PlayerState(
        is_playing=False,
        ambient=L.AmbientState(
            current_track_id=1, position_ms=5_000, position_anchored_at=None
        ),
    )
    assert asyncio.run(adv._plan(frozen)) is None


def test_advancer_bounds_unknown_length_interrupt() -> None:
    """An unknown-length interrupt must still get a (bounded) deadline so it
    can't block ambient from resuming forever when no client sends the skip.
    Ambient with unknown length stays unscheduled — that blocks nothing."""
    from app.sync.advancer import (
        GRACE_S,
        INTERRUPT_UNKNOWN_LENGTH_MAX_S,
        Advancer,
    )

    adv = Advancer()

    async def no_length(track_id: int) -> float | None:
        return None

    adv._track_length_s = no_length  # type: ignore[method-assign]

    now = time.time()
    snap = L.PlayerState(
        is_playing=True,
        ambient=L.AmbientState(
            current_track_id=1, position_ms=0, position_anchored_at=None
        ),
        interrupt=L.InterruptState(
            current_track_id=2, position_ms=0, position_anchored_at=now
        ),
    )
    plan = asyncio.run(adv._plan(snap))
    assert plan is not None
    remaining, lane, track_id = plan
    assert lane == "interrupt" and track_id == 2
    assert 0 < remaining <= INTERRUPT_UNKNOWN_LENGTH_MAX_S + GRACE_S + 0.1


async def test_index_writes_invalidate_advancer_length_cache(
    client: TestClient, db_session
) -> None:
    """A rescan can change a track's duration under the same track id (file
    replaced in place), so any index write must flush the advancer's length
    cache — otherwise it keeps firing on the stale deadline until restart.
    start() registers the hook; index writes run in worker threads, so the
    notification is exercised off-loop here too."""
    from starlette.concurrency import run_in_threadpool

    from app.library import index as library_index
    from app.sync.advancer import Advancer

    adv = Advancer()
    adv.start()
    try:
        assert library_index.on_index_changed is not None
        adv._length_cache[123] = 45.0
        adv._warned_unknown_length.add(123)
        gen_before = adv._cache_generation
        # A real (no-op) index write from the threadpool, like an upload does.
        await run_in_threadpool(library_index.scan_paths, db_session, [])
        # Invalidation is marshalled onto the loop (thread-safe), so yield a
        # loop turn for the scheduled callback to run before asserting.
        await asyncio.sleep(0)
        assert adv._length_cache == {}
        assert adv._warned_unknown_length == set()
        assert adv._cache_generation > gen_before
    finally:
        await adv.stop()
    # stop() must deregister — a stopped advancer shouldn't outlive its hook.
    assert library_index.on_index_changed is None


def test_advancer_advances_the_queue_end_to_end(monkeypatch) -> None:
    """Full-stack: with the advancer on, a playing 0.5s track advances to the
    next queued track with NO client sending ambient_skip_next."""
    from app.core import config
    from app.sync import advancer as advancer_module
    from tests.test_sync import _login

    monkeypatch.setenv("ADVANCER_ENABLED", "1")
    monkeypatch.setattr(advancer_module, "GRACE_S", 0.15)
    config.get_settings.cache_clear()
    try:
        from app.main import app

        with TestClient(app) as c:
            _login(c)
            # Look the two seeded tracks up (0.5s silent WAVs).
            from sqlalchemy import select

            from app.core.db import SessionLocal
            from app.models.track import Track

            with SessionLocal() as db:
                a = db.scalar(select(Track).where(Track.path == "Demo/test-song.wav"))
                assert a is not None
                track_a = a.id

            with c.websocket_connect("/api/ws") as ws:
                ws.receive_json()
                ws.send_json({"type": "ambient_play_track", "track_id": track_a})
                ws.receive_json()
                ws.send_json({"type": "ambient_set_loop", "loop": "track"})
                ws.receive_json()

            # The track is 0.5s; grace 0.15s → the advancer should apply the
            # loop:track restart (an epoch bump) within a couple of seconds,
            # with no connected output at all. Poll the HTTP snapshot.
            deadline = time.time() + 5.0
            baseline_epoch = c.get("/api/sync/state").json()["position_epoch"]
            advanced = False
            while time.time() < deadline:
                snap = c.get("/api/sync/state").json()
                if snap["position_epoch"] > baseline_epoch:
                    advanced = True
                    assert snap["ambient"]["current_track_id"] == track_a
                    assert snap["is_playing"] is True
                    break
                time.sleep(0.1)
            assert advanced, "advancer never fired"
    finally:
        config.get_settings.cache_clear()
