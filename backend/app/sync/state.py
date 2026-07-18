"""Server-side state machine for sync.

Holds the canonical PlayerState in memory; persists to playback_state row on
every state-mutating change. Mutations are serialized via an asyncio.Lock so
actions can't interleave. Position reports are silent — they update state but
don't broadcast (clients dead-reckon from the most recent report).

Mutators are pure functions `(state) -> state`. They return the same state
instance to signal "no change" (state machine then skips persistence and
broadcast).

Playback clock
--------------
The server is the authoritative clock. Each lane stores a BASE position
(`position_ms`) stamped at `position_anchored_at` (epoch seconds); a non-None
anchor means the lane is ticking and its true position is
`base + (now - anchor)`. Reads (snapshot / broadcast) materialize that sum
into `position_ms` via `materialize_positions`, so every state a client sees
carries a current position — with or without position reports. Reports are
just drift corrections (they re-stamp base + anchor).

Deliberate position moves (play / seek / skip / loop restart / interrupt
transitions) additionally bump `PlayerState.position_epoch`. Clients seek the
active lane iff the epoch changed and never compare positions — see
docs/playback-sync-overhaul.md.
"""
from __future__ import annotations

import asyncio
import logging
import random
import time
from typing import Any

from sqlalchemy.orm import Session
from starlette.concurrency import run_in_threadpool

from app.devices.store import device_store
from app.domain import playback_state as playback_state_domain
from app.sync.devices import registry
from app.sync.protocol import (
    AmbientState,
    InterruptState,
    LoopingSfx,
    LoopMode,
    PlayerState,
    PositionReport,
    ShuffleMode,
)

logger = logging.getLogger(__name__)


# --- playback clock helpers --------------------------------------------------


def _now() -> float:
    return time.time()


def _materialize_lane(lane: Any, now: float) -> Any:
    """A copy of the lane with `position_ms` advanced to `now` (anchor
    re-stamped), or the lane itself when its clock is stopped."""
    if lane is None or lane.position_anchored_at is None:
        return lane
    elapsed_ms = int((now - lane.position_anchored_at) * 1000)
    if elapsed_ms <= 0:
        return lane
    return lane.model_copy(
        update={
            "position_ms": lane.position_ms + elapsed_ms,
            "position_anchored_at": now,
        }
    )


def materialize_positions(state: PlayerState, now: float | None = None) -> PlayerState:
    """Advance every ticking lane's `position_ms` to `now`. Pure. Applied on
    the READ path only (snapshot / broadcast) — the stored state keeps the raw
    base+anchor pair so repeated reads don't compound rounding."""
    ts = _now() if now is None else now
    ambient = _materialize_lane(state.ambient, ts)
    interrupt = _materialize_lane(state.interrupt, ts)
    if ambient is state.ambient and interrupt is state.interrupt:
        return state
    return state.model_copy(update={"ambient": ambient, "interrupt": interrupt})


def _freeze_lane(lane: Any, now: float) -> Any:
    """Stop a lane's clock: fold the elapsed time into the base and clear the
    anchor. No-op for an already-frozen lane."""
    if lane is None or lane.position_anchored_at is None:
        return lane
    elapsed_ms = max(0, int((now - lane.position_anchored_at) * 1000))
    return lane.model_copy(
        update={
            "position_ms": lane.position_ms + elapsed_ms,
            "position_anchored_at": None,
        }
    )


# How often silent position reports are flushed to the DB. In-memory state
# is always current; the DB copy only matters for resume-after-restart, so a
# crash loses at most this many seconds of playback position.
_EPHEMERAL_PERSIST_INTERVAL_S = 5.0


def _ambient_anchor(
    is_playing: bool, interrupt: InterruptState | None, now: float
) -> float | None:
    """The anchor a *repositioned* ambient lane should carry: ticking iff
    playback is on and no pausing interrupt sits on top (a ducking interrupt —
    duck_to set — keeps ambient audibly playing, so its clock runs too)."""
    if is_playing and (interrupt is None or interrupt.duck_to is not None):
        return now
    return None


class StateMachine:
    def __init__(self) -> None:
        self._state = PlayerState()
        self._lock = asyncio.Lock()
        self._last_ephemeral_persist = 0.0
        self._loaded = False
        # Fired after every applied mutation (including silent position
        # reports). Late-bound by the track advancer so it can recompute its
        # end-of-track deadline; None outside app lifespan (tests, CLI).
        self.on_applied: Any = None

    async def load(self, db_factory: Any) -> None:
        """Hydrate state from the playback_state DB row, then prune dangling
        references so the runtime never tries to act on tracks / modes /
        presets that disappeared between sessions (e.g. a track row that
        scan_full removed because its file went away — without this pass
        the audio engine would request a 404 stream URL and surface as
        SRC_NOT_SUPPORTED). Persists the cleaned state immediately so the
        next boot starts from the same trimmed snapshot."""

        def _load_and_clean() -> dict:
            db: Session = db_factory()
            try:
                raw = playback_state_domain.get_state(db)
                cleaned = _prune_dangling_state(raw, db)
                if cleaned != raw:
                    playback_state_domain.update_state(db, **cleaned)
                return cleaned
            finally:
                db.close()

        raw = await run_in_threadpool(_load_and_clean)
        merged: dict[str, Any] = self._state.model_dump()
        for key, value in raw.items():
            if key in merged:
                merged[key] = value
        self._state = PlayerState.model_validate(merged)
        self._loaded = True
        logger.info("state machine loaded (revision=%s)", self._state.revision)

    async def snapshot(self) -> PlayerState:
        async with self._lock:
            return materialize_positions(
                self._refresh_devices(self._state)
            ).model_copy(deep=True)

    def reset_for_tests(self) -> None:
        """Wipe the in-memory state. For test isolation only — never call
        in production. Doesn't touch the DB; tests use a throwaway DB anyway.

        Also recreates the mutation lock: asyncio.Lock binds itself to the
        running event loop on its first *contended* acquire, and every
        TestClient runs its own loop — a lock that saw contention in one test
        would wedge (or RuntimeError) every later test. Production has a
        single loop for the process lifetime, so this can't happen there."""
        self._state = PlayerState()
        self._lock = asyncio.Lock()
        self._last_ephemeral_persist = 0.0

    def _refresh_devices(self, state: PlayerState) -> PlayerState:
        """Stamp current device registry into the state. Not persisted —
        connected devices are ephemeral."""
        return state.model_copy(update={"connected_devices": registry.all_infos()})

    async def apply(
        self,
        mutator: Any,
        db_factory: Any,
        *,
        broadcast: bool = True,
    ) -> tuple[PlayerState, bool]:
        """Run `mutator(state) -> new_state` under the lock, persist, and
        return (new_state, did_change). When `broadcast=False`, the change
        is still persisted but the caller should skip broadcasting (used for
        position reports)."""
        async with self._lock:
            new_state = mutator(self._state)
            if new_state is self._state:
                return (
                    materialize_positions(self._refresh_devices(new_state)),
                    False,
                )

            if broadcast:
                new_state = new_state.model_copy(update={"revision": self._state.revision + 1})

            self._state = new_state
            # Position reports (broadcast=False) arrive every second from
            # every active output; the in-memory state is what live clients
            # and mutators read, so the DB copy — which only matters for
            # resuming after a restart — is throttled instead of committed
            # (full model_dump + SQLite write, under this lock) per tick.
            if broadcast:
                await self._persist(db_factory, new_state)
            else:
                now = time.monotonic()
                if now - self._last_ephemeral_persist >= _EPHEMERAL_PERSIST_INTERVAL_S:
                    self._last_ephemeral_persist = now
                    await self._persist(db_factory, new_state)
            if self.on_applied is not None:
                self.on_applied()
            return (
                materialize_positions(self._refresh_devices(new_state)),
                True,
            )

    async def _persist(self, db_factory: Any, state: PlayerState) -> None:
        payload = state.model_dump(mode="json")

        def _write() -> None:
            db: Session = db_factory()
            try:
                playback_state_domain.update_state(db, **payload)
            finally:
                db.close()

        await run_in_threadpool(_write)


def _prune_dangling_state(raw: dict[str, Any], db: Session) -> dict[str, Any]:
    """Drop references inside `raw` to tracks / modes / presets /
    soundboards that no longer exist. Returns a new dict; caller decides
    whether to persist it.

    Pruning rules:
      - `ambient.current_track_id`, `ambient.queue`, `ambient.history`,
        `interrupt.current_track_id`, `interrupt.queue`: keep only ids
        that still resolve to a row in `tracks`.
      - `active_mode_id`: cleared if the mode is no longer loaded.
      - `active_soundboard_id`: cleared if no active mode or soundboard
        isn't part of it.
      - `active_preset_ids`: filtered to only the active mode's presets.
      - `active_output_device_ids`: cleared on boot. Activation is fully
        manual — there is no auto-claim or auto-resume; the operator
        re-activates a designated device when they want sound. (The persistent
        output *designation* lives in DEVICES_FILE, untouched here.)
    """
    from app.models.track import Track  # local to keep cyclic-import risk down
    from app.modes import loader as modes_loader

    out = dict(raw)

    # Volume model migration: older state stored a global master plus sparse
    # per-device trims. Fold those into absolute per-device levels and pin the
    # legacy master field to unity so old clients do not attenuate twice.
    def _unit_interval(value: Any, fallback: float = 1.0) -> float:
        if isinstance(value, bool) or not isinstance(value, int | float):
            return fallback
        return max(0.0, min(1.0, float(value)))

    legacy_master = _unit_interval(out.get("volume"))
    had_absolute_model = "default_device_volume" in out
    default_volume = _unit_interval(out.get("default_device_volume"))
    raw_volumes = out.get("device_volumes")
    volumes = {
        str(device_id): _unit_interval(value)
        for device_id, value in (raw_volumes.items() if isinstance(raw_volumes, dict) else [])
    }
    if not had_absolute_model:
        default_volume = legacy_master
        volumes = {
            device_id: _unit_interval(legacy_master * trim)
            for device_id, trim in volumes.items()
        }
    elif abs(legacy_master - 1.0) > 1e-9:
        # A pre-upgrade client may have written the legacy field between deploy
        # and restart. Preserve its group-volume intent without reviving a
        # second runtime gain.
        default_volume = _unit_interval(default_volume * legacy_master)
        volumes = {
            device_id: _unit_interval(level * legacy_master)
            for device_id, level in volumes.items()
        }

    known_device_ids = set(volumes)
    active_ids = out.get("active_output_device_ids")
    if isinstance(active_ids, list):
        known_device_ids.update(str(device_id) for device_id in active_ids)
    known_device_ids.update(str(device["client_id"]) for device in device_store.list())
    for device_id in known_device_ids:
        volumes.setdefault(device_id, default_volume)
    out["volume"] = 1.0
    out["default_device_volume"] = default_volume
    out["device_volumes"] = volumes

    valid_track_ids = {row[0] for row in db.query(Track.id).all()}

    def _filter_track_id(value: Any) -> Any:
        if isinstance(value, int) and value in valid_track_ids:
            return value
        return None

    def _filter_track_list(value: Any) -> list[int]:
        if not isinstance(value, list):
            return []
        return [v for v in value if isinstance(v, int) and v in valid_track_ids]

    ambient = dict(out.get("ambient") or {})
    if ambient:
        ambient["current_track_id"] = _filter_track_id(ambient.get("current_track_id"))
        ambient["queue"] = _filter_track_list(ambient.get("queue"))
        ambient["history"] = _filter_track_list(ambient.get("history"))
        # The clock did not tick while the server was down — the persisted
        # anchor is meaningless now. Boot frozen at the stored base position.
        ambient["position_anchored_at"] = None
        # "weighted" was removed from ShuffleMode in 2026-07 (it always drew
        # uniformly); a state persisted before that must not fail validation.
        if ambient.get("shuffle") == "weighted":
            ambient["shuffle"] = "random"
        out["ambient"] = ambient

    # An interrupt is transient by definition — it never survives a restart
    # (same no-auto-resume rule as active outputs and looping SFX below).
    # Ambient's resume point is preserved because its position was frozen
    # while the interrupt ran.
    out["interrupt"] = None

    loaded_modes = modes_loader.all_modes()
    active_mode_id = out.get("active_mode_id")
    if active_mode_id is not None and active_mode_id not in loaded_modes:
        active_mode_id = None
    out["active_mode_id"] = active_mode_id

    active_mode = loaded_modes.get(active_mode_id) if active_mode_id else None

    active_soundboard_id = out.get("active_soundboard_id")
    if active_soundboard_id is not None and (
        active_mode is None or active_soundboard_id not in active_mode.soundboards
    ):
        out["active_soundboard_id"] = None

    # Presets are per-mode now, so filter active ids against the active mode's
    # presets (none survive when no mode is active).
    loaded_presets = active_mode.presets if active_mode is not None else {}
    active_preset_ids = out.get("active_preset_ids") or []
    if isinstance(active_preset_ids, list):
        out["active_preset_ids"] = [
            p for p in active_preset_ids if isinstance(p, str) and p in loaded_presets
        ]

    # Wipe active outputs on boot. Activation is fully manual — there is no
    # auto-claim and no auto-resume across a restart. The persistent output
    # *designation* lives in DEVICES_FILE (app/devices/store.py) and is
    # untouched; the operator re-activates a designated device when they want
    # sound. (The client_ids here would otherwise be a stale snapshot of who
    # was live.)
    out["active_output_device_ids"] = []

    # Wipe looping SFX on boot for the same reason: the server-side timers that
    # drive them don't survive a restart, so a persisted entry would show in the
    # LOOPS panel with no timer behind it. Session-only, no auto-resume.
    out["looping_sfx"] = []

    # Playback never auto-resumes across a restart: active outputs are wiped
    # (above), so nothing would sound anyway — and with the server-side
    # advancer, a phantom "playing" state would silently chew through the
    # queue in an empty room. The ambient lane keeps its track + position;
    # the operator presses Play when they're back.
    out["is_playing"] = False

    return out


# Module-level singleton.
machine = StateMachine()


# --- Top-level mutators ----------------------------------------------------


def set_volume(volume: float) -> Any:
    """Compatibility group-volume action for older clients.

    Scale every absolute device level relative to the previous default. New
    clients never send this action; they update a specific device directly.
    """
    target = max(0.0, min(1.0, volume))

    def _mut(state: PlayerState) -> PlayerState:
        # Legacy connections are shown the largest absolute level as their
        # master, which represents every device with a trim in 0..1. Scale
        # against that same displayed value when set_volume comes back.
        previous = max(
            [state.default_device_volume, *state.device_volumes.values()],
            default=state.default_device_volume,
        )
        if abs(previous - target) < 1e-9:
            return state
        if previous > 1e-9:
            scaled = {
                device_id: max(0.0, min(1.0, level * target / previous))
                for device_id, level in state.device_volumes.items()
            }
        else:
            scaled = {device_id: target for device_id in state.device_volumes}
        return state.model_copy(
            update={
                "volume": 1.0,
                "default_device_volume": target,
                "device_volumes": scaled,
            }
        )

    return _mut


def set_is_playing(playing: bool) -> Any:
    """Pause / resume. Stops or restarts the ambient clock; the interrupt
    lane (if any) keeps ticking — clients don't pause interrupts on the
    is_playing flag. No position_epoch bump: pause/resume isn't a seek, the
    client element already sits at the right position."""

    def _mut(state: PlayerState) -> PlayerState:
        if state.is_playing == playing:
            return state
        now = _now()
        ambient = state.ambient
        if not playing:
            ambient = _freeze_lane(ambient, now)
        elif ambient.current_track_id is not None:
            anchor = _ambient_anchor(True, state.interrupt, now)
            if anchor is not None:
                ambient = ambient.model_copy(update={"position_anchored_at": anchor})
        return state.model_copy(update={"is_playing": playing, "ambient": ambient})

    return _mut


def set_active_mode(mode_id: str | None) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        if state.active_mode_id == mode_id:
            return state
        # Presets + soundboard are per-mode — switching modes clears them so
        # stale ids from the previous mode don't linger (they'd no longer
        # resolve). Ambient/queue keep playing across the switch.
        return state.model_copy(
            update={
                "active_mode_id": mode_id,
                "active_preset_ids": [],
                "active_soundboard_id": None,
            }
        )

    return _mut


def set_active_outputs(device_ids: list[str]) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        if list(state.active_output_device_ids) == list(device_ids):
            return state
        return state.model_copy(update={"active_output_device_ids": list(device_ids)})

    return _mut


def add_active_output(device_id: str) -> Any:
    """Append a single device id to `active_output_device_ids`. No-op if the id
    is already present. Used by the WS register path to auto-activate a
    default-on device without a read-modify-write on a stale snapshot (which
    races a concurrent register)."""

    def _mut(state: PlayerState) -> PlayerState:
        if device_id in state.active_output_device_ids:
            return state
        return state.model_copy(
            update={
                "active_output_device_ids": [
                    *state.active_output_device_ids,
                    device_id,
                ]
            }
        )

    return _mut


def remove_active_output(device_id: str) -> Any:
    """Drop a single device id from `active_output_device_ids`. No-op if
    the id isn't present. Used by the WS disconnect path to keep the list
    free of stale device ids."""

    def _mut(state: PlayerState) -> PlayerState:
        if device_id not in state.active_output_device_ids:
            return state
        next_ids = [d for d in state.active_output_device_ids if d != device_id]
        return state.model_copy(update={"active_output_device_ids": next_ids})

    return _mut


def register_device(device_id: str, *, activate: bool) -> Any:
    """Materialize one device's canonical volume and optionally activate it.

    Registration is the point at which a previously unseen client_id becomes a
    concrete output installation. Keeping this atomic avoids separate volume
    and activation broadcasts during reconnect.
    """

    def _mut(state: PlayerState) -> PlayerState:
        update: dict[str, Any] = {}
        if device_id not in state.device_volumes:
            update["device_volumes"] = {
                **state.device_volumes,
                device_id: state.default_device_volume,
            }
        if activate and device_id not in state.active_output_device_ids:
            update["active_output_device_ids"] = [
                *state.active_output_device_ids,
                device_id,
            ]
        if not update:
            return state
        return state.model_copy(update=update)

    return _mut


def set_device_volume(device_id: str, volume: float) -> Any:
    """Set one device's canonical absolute software volume."""

    v = max(0.0, min(1.0, volume))

    def _mut(state: PlayerState) -> PlayerState:
        cur = dict(state.device_volumes)
        if cur.get(device_id, state.default_device_volume) == v and device_id in cur:
            return state
        cur[device_id] = v
        return state.model_copy(update={"device_volumes": cur})

    return _mut


def set_active_soundboard(soundboard_id: str | None) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        if state.active_soundboard_id == soundboard_id:
            return state
        return state.model_copy(update={"active_soundboard_id": soundboard_id})

    return _mut


def set_active_presets(
    preset_ids: list[str],
    *,
    crossfade_ms: int | None = None,
) -> Any:
    """Set the active preset list and optional crossfade override."""

    def _mut(state: PlayerState) -> PlayerState:
        # De-duplicate while preserving order.
        deduped: list[str] = []
        for p in preset_ids:
            if p not in deduped:
                deduped.append(p)
        update: dict[str, Any] = {}
        if list(state.active_preset_ids) != deduped:
            update["active_preset_ids"] = deduped
        if crossfade_ms is not None and crossfade_ms != state.crossfade_ms:
            update["crossfade_ms"] = crossfade_ms
        if not update:
            return state
        return state.model_copy(update=update)

    return _mut


def start_loop(loop: LoopingSfx) -> Any:
    """Add (or replace, by id) a looping SFX entry. The actual interval timer
    is owned by `sync/loops.py` — this only records it in the broadcast state
    so every LOOPS panel reflects it."""

    def _mut(state: PlayerState) -> PlayerState:
        kept = [entry for entry in state.looping_sfx if entry.id != loop.id]
        return state.model_copy(update={"looping_sfx": [*kept, loop]})

    return _mut


def stop_loop(loop_id: str) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        kept = [entry for entry in state.looping_sfx if entry.id != loop_id]
        if len(kept) == len(state.looping_sfx):
            return state
        return state.model_copy(update={"looping_sfx": kept})

    return _mut


def set_crossfade(crossfade_ms: int, crossfade_type: str | None) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        new_ms = crossfade_ms
        new_type = crossfade_type if crossfade_type is not None else state.crossfade_type
        if state.crossfade_ms == new_ms and state.crossfade_type == new_type:
            return state
        return state.model_copy(
            update={"crossfade_ms": new_ms, "crossfade_type": new_type}
        )

    return _mut


def report_position(device_id: str, position_ms: int) -> Any:
    """Stamp telemetry. Updates whichever lane is active. Under the server
    clock this is a drift *correction*: it re-bases the ticking lane at the
    reported position (anchor re-stamped to now). A frozen lane keeps its
    stopped clock — only the base moves. Never bumps position_epoch."""

    def _mut(state: PlayerState) -> PlayerState:
        now = _now()
        report = PositionReport(
            device_id=device_id, position_ms=position_ms, reported_at=now
        )
        if state.interrupt is not None:
            interrupt = state.interrupt.model_copy(
                update={
                    "position_ms": position_ms,
                    "position_anchored_at": (
                        now if state.interrupt.position_anchored_at is not None else None
                    ),
                }
            )
            return state.model_copy(
                update={"last_position_report": report, "interrupt": interrupt}
            )
        ambient = state.ambient.model_copy(
            update={
                "position_ms": position_ms,
                "position_anchored_at": (
                    now if state.ambient.position_anchored_at is not None else None
                ),
            }
        )
        return state.model_copy(
            update={"last_position_report": report, "ambient": ambient}
        )

    return _mut


# --- Ambient lane mutators -------------------------------------------------


def _replace_ambient(state: PlayerState, ambient: AmbientState) -> PlayerState:
    return state.model_copy(update={"ambient": ambient})


def ambient_play_track(track_id: int) -> Any:
    """Play this track now. Replaces current; clears queue and history.
    Implicitly sets is_playing=true."""

    def _mut(state: PlayerState) -> PlayerState:
        now = _now()
        new_ambient = AmbientState(
            current_track_id=track_id,
            queue=[],
            history=[],
            position_ms=0,
            position_anchored_at=_ambient_anchor(True, state.interrupt, now),
            loop=state.ambient.loop,
            shuffle=state.ambient.shuffle,
        )
        return state.model_copy(
            update={
                "ambient": new_ambient,
                "is_playing": True,
                "position_epoch": state.position_epoch + 1,
            }
        )

    return _mut


def ambient_set_queue(track_ids: list[int]) -> Any:
    """Replace the queue. Doesn't touch current."""

    def _mut(state: PlayerState) -> PlayerState:
        if list(state.ambient.queue) == list(track_ids):
            return state
        # Explicit queue replacement = diverged from any source playlist.
        return _replace_ambient(
            state,
            state.ambient.model_copy(
                update={"queue": list(track_ids), "source_playlist_id": None}
            ),
        )

    return _mut


def ambient_enqueue(track_id: int, position: int | None) -> Any:
    """Add a track to the queue at `position` (default: append)."""

    def _mut(state: PlayerState) -> PlayerState:
        new_queue = list(state.ambient.queue)
        idx = len(new_queue) if position is None else max(0, min(position, len(new_queue)))
        new_queue.insert(idx, track_id)
        return _replace_ambient(state, state.ambient.model_copy(update={"queue": new_queue}))

    return _mut


def ambient_clear_queue() -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        if not state.ambient.queue:
            return state
        return _replace_ambient(state, state.ambient.model_copy(update={"queue": []}))

    return _mut


def ambient_skip_next(
    follow_next_id: int | None = None, *, expected_track_id: int | None = None
) -> Any:
    """Advance to next track. Behavior at end of queue depends on loop mode.

    `follow_next_id` is the next library track the *handler* resolved for
    follow mode (the mutator can't touch the DB). It's only consulted when the
    queue is empty and `loop == "follow"`; None means no successor available,
    so follow degrades to idle.

    `expected_track_id` is the idempotency token from
    AmbientSkipNextAction.from_track_id (also passed by the server-side
    advancer): when set and the lane has already moved past that track, the
    skip is a no-op — a duplicate `ended` from a second output (or a race
    with the advancer) can no longer double-advance or stop the queue."""

    def _mut(state: PlayerState) -> PlayerState:
        amb = state.ambient
        if (
            expected_track_id is not None
            and amb.current_track_id != expected_track_id
        ):
            return state  # someone already advanced past that track
        if amb.current_track_id is None and not amb.queue:
            return state  # nothing to skip from

        now = _now()
        epoch = state.position_epoch + 1
        anchor = _ambient_anchor(state.is_playing, state.interrupt, now)

        if amb.loop == "track" and amb.current_track_id is not None:
            # Restart current.
            return state.model_copy(
                update={
                    "ambient": amb.model_copy(
                        update={"position_ms": 0, "position_anchored_at": anchor}
                    ),
                    "position_epoch": epoch,
                }
            )

        if amb.queue:
            # Normal advance. Shuffle pulls a random entry instead of the
            # head; "off" keeps deterministic queue order.
            new_history = list(amb.history)
            if amb.current_track_id is not None:
                new_history.append(amb.current_track_id)
            pick = random.randrange(len(amb.queue)) if amb.shuffle != "off" else 0
            new_current = amb.queue[pick]
            new_queue = [t for i, t in enumerate(amb.queue) if i != pick]
            return state.model_copy(
                update={
                    "ambient": amb.model_copy(
                        update={
                            "current_track_id": new_current,
                            "queue": new_queue,
                            "history": new_history,
                            "position_ms": 0,
                            "position_anchored_at": anchor,
                        }
                    ),
                    "position_epoch": epoch,
                }
            )

        # Queue empty.
        if amb.loop == "queue" and (amb.history or amb.current_track_id is not None):
            # Wrap around: rebuild queue from history + current; play first.
            # `current` alone (single-track lane, empty history) is enough —
            # repeat-queue then restarts the one track, like repeat-track.
            full = [*amb.history]
            if amb.current_track_id is not None:
                full.append(amb.current_track_id)
            new_current = full[0]
            new_queue = full[1:]
            return state.model_copy(
                update={
                    "ambient": amb.model_copy(
                        update={
                            "current_track_id": new_current,
                            "queue": new_queue,
                            "history": [],
                            "position_ms": 0,
                            "position_anchored_at": anchor,
                        }
                    ),
                    "position_epoch": epoch,
                }
            )

        if amb.loop == "follow" and follow_next_id is not None:
            # Continue into the library: the just-finished track joins history
            # and the handler-resolved successor becomes current. Queue stays
            # empty — follow advances one library track at a time, re-resolving
            # from the new current on the next skip. Keeps is_playing as-is.
            new_history = list(amb.history)
            if amb.current_track_id is not None:
                new_history.append(amb.current_track_id)
            return state.model_copy(
                update={
                    "ambient": amb.model_copy(
                        update={
                            "current_track_id": follow_next_id,
                            "queue": [],
                            "history": new_history,
                            "position_ms": 0,
                            "position_anchored_at": anchor,
                        }
                    ),
                    "position_epoch": epoch,
                }
            )

        # Loop off (or follow with no successor), end of queue: clear current
        # AND stop. Leaving is_playing=true here is what let clients dead-reckon
        # the position clock upward against an empty lane ("Nothing playing"
        # ticking up).
        return state.model_copy(
            update={
                "ambient": amb.model_copy(
                    update={
                        "current_track_id": None,
                        "position_ms": 0,
                        "position_anchored_at": None,
                    }
                ),
                "is_playing": False,
                "position_epoch": epoch,
            }
        )

    return _mut


def ambient_skip_prev() -> Any:
    """Go back to previous track if history exists; otherwise restart current."""

    def _mut(state: PlayerState) -> PlayerState:
        amb = state.ambient
        now = _now()
        epoch = state.position_epoch + 1
        anchor = _ambient_anchor(state.is_playing, state.interrupt, now)
        if not amb.history:
            if amb.current_track_id is None:
                return state
            # Restart current from the top. (No "already at 0" short-circuit:
            # the stored base is stale under the ticking clock, so equality
            # against it says nothing about the real position.)
            return state.model_copy(
                update={
                    "ambient": amb.model_copy(
                        update={"position_ms": 0, "position_anchored_at": anchor}
                    ),
                    "position_epoch": epoch,
                }
            )

        # Move current back to queue head, pop history.
        new_queue = list(amb.queue)
        if amb.current_track_id is not None:
            new_queue.insert(0, amb.current_track_id)
        new_history = list(amb.history)
        new_current = new_history.pop()
        return state.model_copy(
            update={
                "ambient": amb.model_copy(
                    update={
                        "current_track_id": new_current,
                        "queue": new_queue,
                        "history": new_history,
                        "position_ms": 0,
                        "position_anchored_at": anchor,
                    }
                ),
                "position_epoch": epoch,
            }
        )

    return _mut


def ambient_seek(position_ms: int) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        if state.ambient.current_track_id is None:
            return state  # nothing loaded — a position would be meaningless
        now = _now()
        return state.model_copy(
            update={
                "ambient": state.ambient.model_copy(
                    update={
                        "position_ms": position_ms,
                        "position_anchored_at": _ambient_anchor(
                            state.is_playing, state.interrupt, now
                        ),
                    }
                ),
                "position_epoch": state.position_epoch + 1,
            }
        )

    return _mut


def ambient_set_loop(mode: LoopMode) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        if state.ambient.loop == mode:
            return state
        return _replace_ambient(state, state.ambient.model_copy(update={"loop": mode}))

    return _mut


def ambient_set_shuffle(mode: ShuffleMode) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        if state.ambient.shuffle == mode:
            return state
        return _replace_ambient(
            state, state.ambient.model_copy(update={"shuffle": mode})
        )

    return _mut


def ambient_stop() -> Any:
    """Clear ambient lane entirely. Doesn't change is_playing — that's
    a separate action."""

    def _mut(state: PlayerState) -> PlayerState:
        if (
            state.ambient.current_track_id is None
            and not state.ambient.queue
            and not state.ambient.history
            and state.ambient.position_ms == 0
        ):
            return state
        return state.model_copy(
            update={
                "ambient": AmbientState(
                    loop=state.ambient.loop, shuffle=state.ambient.shuffle
                ),
                "position_epoch": state.position_epoch + 1,
            }
        )

    return _mut


def ambient_play_playlist(
    track_ids: list[int], start_index: int, source_playlist_id: int | None = None
) -> Any:
    """Load a pre-resolved track list into ambient and start at start_index.
    Implicitly sets is_playing=true. `source_playlist_id` is stamped on the lane
    so the Console can show which playlist is driving."""

    def _mut(state: PlayerState) -> PlayerState:
        if not track_ids:
            return state
        idx = max(0, min(start_index, len(track_ids) - 1))
        new_ambient = AmbientState(
            current_track_id=track_ids[idx],
            queue=list(track_ids[idx + 1 :]),
            history=list(track_ids[:idx]),
            position_ms=0,
            position_anchored_at=_ambient_anchor(True, state.interrupt, _now()),
            loop=state.ambient.loop,
            shuffle=state.ambient.shuffle,
            source_playlist_id=source_playlist_id,
        )
        return state.model_copy(
            update={
                "ambient": new_ambient,
                "is_playing": True,
                "position_epoch": state.position_epoch + 1,
            }
        )

    return _mut


# --- Interrupt lane mutators -----------------------------------------------


def _fire_interrupt(
    state: PlayerState,
    track_ids: list[int],
    return_to_ambient: bool,
    fade_in_ms: int,
    fade_out_ms: int,
    duck_to: float | None,
) -> PlayerState:
    """Shared body of the two fire_interrupt_* mutators. A pausing interrupt
    (duck_to=None) freezes the ambient clock so the resume point is preserved;
    a ducking one leaves ambient ticking (it keeps playing audibly)."""
    now = _now()
    new_interrupt = InterruptState(
        current_track_id=track_ids[0],
        queue=list(track_ids[1:]),
        position_ms=0,
        position_anchored_at=now,
        return_to_ambient=return_to_ambient,
        fade_in_ms=fade_in_ms,
        fade_out_ms=fade_out_ms,
        duck_to=duck_to,
    )
    ambient = state.ambient
    if duck_to is None:
        ambient = _freeze_lane(ambient, now)
    return state.model_copy(
        update={
            "interrupt": new_interrupt,
            "ambient": ambient,
            "is_playing": True,
            "position_epoch": state.position_epoch + 1,
        }
    )


def fire_interrupt_track(
    track_id: int,
    return_to_ambient: bool,
    fade_in_ms: int,
    fade_out_ms: int,
    duck_to: float | None = None,
) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        return _fire_interrupt(
            state, [track_id], return_to_ambient, fade_in_ms, fade_out_ms, duck_to
        )

    return _mut


def fire_interrupt_playlist(
    track_ids: list[int],
    return_to_ambient: bool,
    fade_in_ms: int,
    fade_out_ms: int,
    duck_to: float | None = None,
) -> Any:
    """Load a pre-resolved track list as an interrupt; first becomes current."""

    def _mut(state: PlayerState) -> PlayerState:
        if not track_ids:
            return state
        return _fire_interrupt(
            state, track_ids, return_to_ambient, fade_in_ms, fade_out_ms, duck_to
        )

    return _mut


def _end_interrupt(state: PlayerState) -> PlayerState:
    """Clear the interrupt lane. If return_to_ambient=true, ambient takes back
    over — its clock restarts from the base a pausing interrupt froze (a
    ducking one never stopped it). Otherwise stop playback."""
    if state.interrupt is None:
        return state
    now = _now()
    return_to_ambient = state.interrupt.return_to_ambient
    updates: dict = {
        "interrupt": None,
        "position_epoch": state.position_epoch + 1,
    }
    ambient = state.ambient
    if not return_to_ambient:
        updates["is_playing"] = False
        ambient = _freeze_lane(ambient, now)
    elif (
        state.is_playing
        and ambient.current_track_id is not None
        and ambient.position_anchored_at is None
    ):
        ambient = ambient.model_copy(update={"position_anchored_at": now})
    if ambient is not state.ambient:
        updates["ambient"] = ambient
    return state.model_copy(update=updates)


def interrupt_skip_next(*, expected_track_id: int | None = None) -> Any:
    """Advance to next interrupt track. If queue empty, auto-completes.
    `expected_track_id` carries the same idempotency contract as
    ambient_skip_next."""

    def _mut(state: PlayerState) -> PlayerState:
        if state.interrupt is None:
            return state
        if (
            expected_track_id is not None
            and state.interrupt.current_track_id != expected_track_id
        ):
            return state
        if state.interrupt.queue:
            new_current = state.interrupt.queue[0]
            new_queue = state.interrupt.queue[1:]
            new_interrupt = state.interrupt.model_copy(
                update={
                    "current_track_id": new_current,
                    "queue": new_queue,
                    "position_ms": 0,
                    "position_anchored_at": _now(),
                }
            )
            return state.model_copy(
                update={
                    "interrupt": new_interrupt,
                    "position_epoch": state.position_epoch + 1,
                }
            )
        return _end_interrupt(state)

    return _mut


def interrupt_seek(position_ms: int) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        if state.interrupt is None:
            return state
        new_interrupt = state.interrupt.model_copy(
            update={"position_ms": position_ms, "position_anchored_at": _now()}
        )
        return state.model_copy(
            update={
                "interrupt": new_interrupt,
                "position_epoch": state.position_epoch + 1,
            }
        )

    return _mut


def cancel_interrupt() -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        if state.interrupt is None:
            return state
        return _end_interrupt(state)

    return _mut


