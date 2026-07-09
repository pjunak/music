"""Server-side track lifecycle: end-of-track advancement.

Historically the queue only advanced when a *client* observed its `<audio>`
element end and sent `ambient_skip_next` — which silently failed whenever the
playing output couldn't send it (guest compat-mode devices, the headless
appliance, a dropped WebSocket send) and double-fired when several outputs
played at once. This watchdog makes the server own that job: it watches the
materialized playback clock and applies the same advance mutators the WS
handlers use once the active lane's track runs past its known length. It also
ends interrupts that no browser is playing.

Client `ended` events remain the low-latency fast path; their skips carry a
`from_track_id` idempotency token, and the advancer passes the track id it
verified under the lock — so however many advance attempts race, exactly one
wins (see `expected_track_id` in `state.ambient_skip_next`).

Tracks with unknown length (no readable duration tag) can't be scheduled;
they stay client-advance-only and are logged once.
"""
from __future__ import annotations

import asyncio
import contextlib
import logging

from starlette.concurrency import run_in_threadpool

from app.core.db import SessionLocal
from app.library import index as library_index
from app.models.track import Track
from app.sync import state as state_module
from app.sync.protocol import PlayerState

logger = logging.getLogger(__name__)

# How far past the nominal track end we wait before advancing. Covers tag
# duration imprecision and lets a healthy client's `ended`-driven skip win the
# race (it fires at the true end, before us), keeping the advancer a backstop.
GRACE_S = 0.75

_LENGTH_CACHE_MAX = 4096


async def resolve_follow_next() -> int | None:
    """For follow ("Continue") mode: the next library track after the one
    currently playing, or None when follow doesn't apply right now — a
    different loop mode is set, the queue still has tracks (a normal advance
    handles those), or nothing is playing. Snapshots the live state so the
    caller can pass the successor into the pure mutator."""
    snap = await state_module.machine.snapshot()
    amb = snap.ambient
    if amb.loop != "follow" or amb.queue or amb.current_track_id is None:
        return None
    current_id = amb.current_track_id

    def _work() -> int | None:
        db = SessionLocal()
        try:
            track = db.get(Track, current_id)
            if track is None:
                return None
            return library_index.next_track_id_after(db, track.path)
        finally:
            db.close()

    return await run_in_threadpool(_work)


class Advancer:
    def __init__(self) -> None:
        self._wake = asyncio.Event()
        self._task: asyncio.Task | None = None
        self._loop: asyncio.AbstractEventLoop | None = None
        self._length_cache: dict[int, float | None] = {}
        self._warned_unknown_length: set[int] = set()

    # -- lifecycle -----------------------------------------------------------

    def start(self) -> None:
        """Begin watching. Must be called from a running event loop (app
        lifespan). Idempotent. The wake event is recreated per start because
        asyncio primitives bind to the loop they first wait on — a restarted
        lifespan (tests) runs on a fresh loop."""
        if self._task is not None:
            return
        self._wake = asyncio.Event()
        self._loop = asyncio.get_running_loop()
        state_module.machine.on_applied = self.poke
        library_index.on_index_changed = self.invalidate_lengths
        self._task = self._loop.create_task(self._run(), name="track-advancer")
        logger.info("track advancer started (grace=%.2fs)", GRACE_S)

    async def stop(self) -> None:
        if self._task is None:
            return
        state_module.machine.on_applied = None
        library_index.on_index_changed = None
        self._task.cancel()
        with contextlib.suppress(asyncio.CancelledError):
            await self._task
        self._task = None
        self._loop = None

    def poke(self) -> None:
        """State changed — recompute the deadline. Called (via the state
        machine's on_applied hook) after every mutation, including silent
        position reports."""
        self._wake.set()

    def invalidate_lengths(self) -> None:
        """Library index changed — cached `Track.length_s` values may be
        stale (a rescan re-reads a replaced file's duration under the same
        track id). Drop the cache and replan. Index writes run in worker
        threads, so the wake is marshalled onto the advancer's loop instead
        of touching the (thread-unsafe) Event directly."""
        self._length_cache.clear()
        self._warned_unknown_length.clear()
        loop = self._loop
        if loop is not None:
            loop.call_soon_threadsafe(self._wake.set)

    # -- scheduling ----------------------------------------------------------

    async def _track_length_s(self, track_id: int) -> float | None:
        if track_id in self._length_cache:
            return self._length_cache[track_id]

        def _work() -> float | None:
            db = SessionLocal()
            try:
                track = db.get(Track, track_id)
                if track is None or not track.length_s or track.length_s <= 0:
                    return None
                return float(track.length_s)
            finally:
                db.close()

        length = await run_in_threadpool(_work)
        if len(self._length_cache) >= _LENGTH_CACHE_MAX:
            self._length_cache.clear()
        self._length_cache[track_id] = length
        if length is None and track_id not in self._warned_unknown_length:
            self._warned_unknown_length.add(track_id)
            logger.warning(
                "track %d has no readable duration — server-side advance "
                "disabled for it (client `ended` events still advance)",
                track_id,
            )
        return length

    async def _plan(self, snap: PlayerState) -> tuple[float, str, int] | None:
        """Earliest upcoming end among ticking lanes, as
        (remaining_s, lane, track_id). None = nothing to schedule (idle, paused,
        or only unknown-length tracks). `snap` must be a materialized snapshot —
        a lane with a non-None anchor is ticking and its position_ms is current."""
        candidates: list[tuple[float, str, int]] = []
        interrupt = snap.interrupt
        if interrupt is not None and interrupt.position_anchored_at is not None:
            length = await self._track_length_s(interrupt.current_track_id)
            if length is not None:
                remaining = length - interrupt.position_ms / 1000.0 + GRACE_S
                candidates.append((remaining, "interrupt", interrupt.current_track_id))
        ambient = snap.ambient
        if (
            ambient.current_track_id is not None
            and ambient.position_anchored_at is not None
        ):
            length = await self._track_length_s(ambient.current_track_id)
            if length is not None:
                remaining = length - ambient.position_ms / 1000.0 + GRACE_S
                candidates.append((remaining, "ambient", ambient.current_track_id))
        if not candidates:
            return None
        return min(candidates, key=lambda c: c[0])

    async def _fire(self, lane: str, track_id: int) -> None:
        # Late import: commit_and_broadcast lives in the package __init__,
        # which imports this module — importing at call time keeps the single
        # mutate-funnel without a cycle.
        from app.sync import commit_and_broadcast

        if lane == "interrupt":
            await commit_and_broadcast(
                state_module.interrupt_skip_next(expected_track_id=track_id)
            )
        else:
            follow_next_id = await resolve_follow_next()
            await commit_and_broadcast(
                state_module.ambient_skip_next(
                    follow_next_id, expected_track_id=track_id
                )
            )
        logger.info("advanced %s lane past track %d (end of track)", lane, track_id)

    async def _run(self) -> None:
        while True:
            try:
                await self._run_once()
            except asyncio.CancelledError:
                raise
            except Exception:
                logger.exception("track advancer iteration failed; continuing")
                await asyncio.sleep(1.0)

    async def _run_once(self) -> None:
        # Clear BEFORE snapshotting: a mutation that lands after the snapshot
        # sets the event again and the wait below returns immediately, so no
        # state change can slip through unseen.
        self._wake.clear()
        snap = await state_module.machine.snapshot()
        plan = await self._plan(snap)
        if plan is None:
            await self._wake.wait()
            return
        remaining, lane, track_id = plan
        if remaining > 0:
            try:
                await asyncio.wait_for(self._wake.wait(), timeout=remaining)
                return  # state changed — recompute from a fresh snapshot
            except TimeoutError:
                pass
        # Deadline reached with no state change in between. Re-verify against
        # a fresh snapshot (a report may have just rewound the clock slightly)
        # and only advance if this track is genuinely done.
        snap2 = await state_module.machine.snapshot()
        plan2 = await self._plan(snap2)
        if plan2 is None:
            return
        remaining2, lane2, track2 = plan2
        if lane2 != lane or track2 != track_id or remaining2 > 0.05:
            return
        await self._fire(lane2, track2)


# Module-level singleton, started/stopped by the app lifespan.
advancer = Advancer()
