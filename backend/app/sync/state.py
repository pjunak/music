"""Server-side state machine for sync.

Holds the canonical PlayerState in memory; persists to playback_state row on
every state-mutating change. Mutations are serialized via an asyncio.Lock so
actions can't interleave. Position reports are silent — they update state but
don't broadcast (clients dead-reckon from the most recent report).

Mutators are pure functions `(state) -> state`. They return the same state
instance to signal "no change" (state machine then skips persistence and
broadcast).
"""
from __future__ import annotations

import asyncio
import logging
import random
import time
from typing import Any

from sqlalchemy.orm import Session
from starlette.concurrency import run_in_threadpool

from app.domain import playback_state as playback_state_domain
from app.sync.devices import registry
from app.sync.protocol import (
    AmbientState,
    InterruptState,
    LoopMode,
    PlayerState,
    PositionReport,
    ScenePreviousState,
    ShuffleMode,
)

logger = logging.getLogger(__name__)


class StateMachine:
    def __init__(self) -> None:
        self._state = PlayerState()
        self._lock = asyncio.Lock()
        self._loaded = False

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
            return self._refresh_devices(self._state).model_copy(deep=True)

    def reset_for_tests(self) -> None:
        """Wipe the in-memory state. For test isolation only — never call
        in production. Doesn't touch the DB; tests use a throwaway DB anyway.
        """
        self._state = PlayerState()

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
                return (self._refresh_devices(new_state), False)

            if broadcast:
                new_state = new_state.model_copy(update={"revision": self._state.revision + 1})

            self._state = new_state
            await self._persist(db_factory, new_state)
            return (self._refresh_devices(new_state), True)

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
    """Drop references inside `raw` to tracks / modes / presets / scenes /
    soundboards that no longer exist. Returns a new dict; caller decides
    whether to persist it.

    Pruning rules:
      - `ambient.current_track_id`, `ambient.queue`, `ambient.history`,
        `interrupt.current_track_id`, `interrupt.queue`: keep only ids
        that still resolve to a row in `tracks`.
      - `active_mode_id`: cleared if the mode is no longer loaded.
      - `active_soundboard_id`: cleared if no active mode or soundboard
        isn't part of it.
      - `active_scene_id`: same scoping rule.
      - `active_preset_ids`: filtered to only loaded presets.
      - `active_output_device_ids`: cleared. Device ids are per-WS-connection
        and a server restart wipes them all; auto-claim re-establishes.
    """
    from app.models.track import Track  # local to keep cyclic-import risk down
    from app.modes import loader as modes_loader
    from app.presets import loader as presets_loader

    out = dict(raw)

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
        out["ambient"] = ambient

    interrupt = out.get("interrupt")
    if isinstance(interrupt, dict):
        if _filter_track_id(interrupt.get("current_track_id")) is None:
            out["interrupt"] = None
        else:
            interrupt = dict(interrupt)
            interrupt["queue"] = _filter_track_list(interrupt.get("queue"))
            out["interrupt"] = interrupt

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

    active_scene_id = out.get("active_scene_id")
    if active_scene_id is not None and (
        active_mode is None or active_scene_id not in active_mode.scenes
    ):
        out["active_scene_id"] = None

    loaded_presets = presets_loader.all_presets()
    active_preset_ids = out.get("active_preset_ids") or []
    if isinstance(active_preset_ids, list):
        out["active_preset_ids"] = [
            p for p in active_preset_ids if isinstance(p, str) and p in loaded_presets
        ]

    # Snapshot captured at scene activation — same dangling-ref hazards as the
    # live ambient/preset fields, so prune in lockstep.
    pre_scene = out.get("pre_scene_state")
    if isinstance(pre_scene, dict):
        snap = dict(pre_scene)
        snap_ambient = snap.get("ambient")
        if isinstance(snap_ambient, dict):
            snap_ambient = dict(snap_ambient)
            snap_ambient["current_track_id"] = _filter_track_id(
                snap_ambient.get("current_track_id")
            )
            snap_ambient["queue"] = _filter_track_list(snap_ambient.get("queue"))
            snap_ambient["history"] = _filter_track_list(snap_ambient.get("history"))
            snap["ambient"] = snap_ambient
        snap_presets = snap.get("active_preset_ids")
        if isinstance(snap_presets, list):
            snap["active_preset_ids"] = [
                p for p in snap_presets if isinstance(p, str) and p in loaded_presets
            ]
        out["pre_scene_state"] = snap

    # Wipe active outputs on boot. Activation is fully manual — there is no
    # auto-claim and no auto-resume across a restart. The persistent output
    # *designation* lives in the known_devices table and is untouched; the
    # operator re-activates a designated device when they want sound. (The
    # client_ids here would otherwise be a stale snapshot of who was live.)
    out["active_output_device_ids"] = []

    # Can't be "playing" with nothing loaded. If pruning emptied both lanes
    # (deleted current track, no interrupt), force is_playing=false so
    # clients don't dead-reckon a position clock against a phantom track on
    # the next boot. Catches every "playing nothing" path, not just the
    # ones we know about.
    ambient_after = out.get("ambient") or {}
    interrupt_after = out.get("interrupt")
    nothing_loaded = (
        ambient_after.get("current_track_id") is None and interrupt_after is None
    )
    if nothing_loaded and out.get("is_playing"):
        out["is_playing"] = False

    return out


# Module-level singleton.
machine = StateMachine()


# --- Top-level mutators ----------------------------------------------------


def set_volume(volume: float) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        if state.volume == volume:
            return state
        return state.model_copy(update={"volume": volume})

    return _mut


def set_is_playing(playing: bool) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        if state.is_playing == playing:
            return state
        return state.model_copy(update={"is_playing": playing})

    return _mut


def set_active_mode(mode_id: str | None) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        if state.active_mode_id == mode_id:
            return state
        return state.model_copy(update={"active_mode_id": mode_id})

    return _mut


def set_active_outputs(device_ids: list[str]) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        if list(state.active_output_device_ids) == list(device_ids):
            return state
        return state.model_copy(update={"active_output_device_ids": list(device_ids)})

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


def set_active_soundboard(soundboard_id: str | None) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        if state.active_soundboard_id == soundboard_id:
            return state
        return state.model_copy(update={"active_soundboard_id": soundboard_id})

    return _mut


def set_active_presets(preset_ids: list[str]) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        # De-duplicate while preserving order.
        deduped: list[str] = []
        for p in preset_ids:
            if p not in deduped:
                deduped.append(p)
        if list(state.active_preset_ids) == deduped:
            return state
        return state.model_copy(update={"active_preset_ids": deduped})

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
    """Stamp telemetry. Updates whichever lane is active."""

    def _mut(state: PlayerState) -> PlayerState:
        report = PositionReport(
            device_id=device_id, position_ms=position_ms, reported_at=time.time()
        )
        if state.interrupt is not None:
            interrupt = state.interrupt.model_copy(update={"position_ms": position_ms})
            return state.model_copy(
                update={"last_position_report": report, "interrupt": interrupt}
            )
        ambient = state.ambient.model_copy(update={"position_ms": position_ms})
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
        new_ambient = AmbientState(
            current_track_id=track_id,
            queue=[],
            history=[],
            position_ms=0,
            loop=state.ambient.loop,
        )
        return state.model_copy(update={"ambient": new_ambient, "is_playing": True})

    return _mut


def ambient_set_queue(track_ids: list[int]) -> Any:
    """Replace the queue. Doesn't touch current."""

    def _mut(state: PlayerState) -> PlayerState:
        if list(state.ambient.queue) == list(track_ids):
            return state
        return _replace_ambient(state, state.ambient.model_copy(update={"queue": list(track_ids)}))

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


def ambient_skip_next() -> Any:
    """Advance to next track. Behavior at end of queue depends on loop mode."""

    def _mut(state: PlayerState) -> PlayerState:
        amb = state.ambient
        if amb.current_track_id is None and not amb.queue:
            return state  # nothing to skip from

        if amb.loop == "track" and amb.current_track_id is not None:
            # Restart current.
            return _replace_ambient(state, amb.model_copy(update={"position_ms": 0}))

        if amb.queue:
            # Normal advance. Shuffle (random/weighted) pulls a random entry
            # instead of the head; "off" keeps deterministic queue order.
            # Weighting is TODO — "weighted" draws uniformly for now.
            new_history = list(amb.history)
            if amb.current_track_id is not None:
                new_history.append(amb.current_track_id)
            pick = random.randrange(len(amb.queue)) if amb.shuffle != "off" else 0
            new_current = amb.queue[pick]
            new_queue = [t for i, t in enumerate(amb.queue) if i != pick]
            return _replace_ambient(
                state,
                amb.model_copy(
                    update={
                        "current_track_id": new_current,
                        "queue": new_queue,
                        "history": new_history,
                        "position_ms": 0,
                    }
                ),
            )

        # Queue empty.
        if amb.loop == "queue" and amb.history:
            # Wrap around: rebuild queue from history + current; play first.
            full = [*amb.history]
            if amb.current_track_id is not None:
                full.append(amb.current_track_id)
            new_current = full[0]
            new_queue = full[1:]
            return _replace_ambient(
                state,
                amb.model_copy(
                    update={
                        "current_track_id": new_current,
                        "queue": new_queue,
                        "history": [],
                        "position_ms": 0,
                    }
                ),
            )

        # Loop off, end of queue: clear current AND stop. Leaving
        # is_playing=true here is what let clients dead-reckon the position
        # clock upward against an empty lane ("Nothing playing" ticking up).
        return state.model_copy(
            update={
                "ambient": amb.model_copy(
                    update={"current_track_id": None, "position_ms": 0}
                ),
                "is_playing": False,
            }
        )

    return _mut


def ambient_skip_prev() -> Any:
    """Go back to previous track if history exists; otherwise restart current."""

    def _mut(state: PlayerState) -> PlayerState:
        amb = state.ambient
        if not amb.history:
            if amb.current_track_id is None:
                return state
            if amb.position_ms == 0:
                return state
            return _replace_ambient(state, amb.model_copy(update={"position_ms": 0}))

        # Move current back to queue head, pop history.
        new_queue = list(amb.queue)
        if amb.current_track_id is not None:
            new_queue.insert(0, amb.current_track_id)
        new_history = list(amb.history)
        new_current = new_history.pop()
        return _replace_ambient(
            state,
            amb.model_copy(
                update={
                    "current_track_id": new_current,
                    "queue": new_queue,
                    "history": new_history,
                    "position_ms": 0,
                }
            ),
        )

    return _mut


def ambient_seek(position_ms: int) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        if state.ambient.position_ms == position_ms:
            return state
        return _replace_ambient(
            state, state.ambient.model_copy(update={"position_ms": position_ms})
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
        return _replace_ambient(
            state,
            AmbientState(loop=state.ambient.loop),
        )

    return _mut


def ambient_play_playlist(track_ids: list[int], start_index: int) -> Any:
    """Load a pre-resolved track list into ambient and start at start_index.
    Implicitly sets is_playing=true."""

    def _mut(state: PlayerState) -> PlayerState:
        if not track_ids:
            return state
        idx = max(0, min(start_index, len(track_ids) - 1))
        new_ambient = AmbientState(
            current_track_id=track_ids[idx],
            queue=list(track_ids[idx + 1 :]),
            history=list(track_ids[:idx]),
            position_ms=0,
            loop=state.ambient.loop,
        )
        return state.model_copy(update={"ambient": new_ambient, "is_playing": True})

    return _mut


# --- Interrupt lane mutators -----------------------------------------------


def fire_interrupt_track(
    track_id: int,
    return_to_ambient: bool,
    fade_in_ms: int,
    fade_out_ms: int,
    duck_to: float | None = None,
) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        new_interrupt = InterruptState(
            current_track_id=track_id,
            queue=[],
            position_ms=0,
            return_to_ambient=return_to_ambient,
            fade_in_ms=fade_in_ms,
            fade_out_ms=fade_out_ms,
            duck_to=duck_to,
        )
        return state.model_copy(update={"interrupt": new_interrupt, "is_playing": True})

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
        new_interrupt = InterruptState(
            current_track_id=track_ids[0],
            queue=list(track_ids[1:]),
            position_ms=0,
            return_to_ambient=return_to_ambient,
            fade_in_ms=fade_in_ms,
            fade_out_ms=fade_out_ms,
            duck_to=duck_to,
        )
        return state.model_copy(update={"interrupt": new_interrupt, "is_playing": True})

    return _mut


def _end_interrupt(state: PlayerState) -> PlayerState:
    """Clear the interrupt lane. If return_to_ambient=true, ambient takes
    back over (its position_ms preserved by virtue of not being touched
    during the interrupt). Otherwise stop playback."""
    if state.interrupt is None:
        return state
    return_to_ambient = state.interrupt.return_to_ambient
    updates: dict = {"interrupt": None}
    if not return_to_ambient:
        updates["is_playing"] = False
    return state.model_copy(update=updates)


def interrupt_skip_next() -> Any:
    """Advance to next interrupt track. If queue empty, auto-completes."""

    def _mut(state: PlayerState) -> PlayerState:
        if state.interrupt is None:
            return state
        if state.interrupt.queue:
            new_current = state.interrupt.queue[0]
            new_queue = state.interrupt.queue[1:]
            new_interrupt = state.interrupt.model_copy(
                update={
                    "current_track_id": new_current,
                    "queue": new_queue,
                    "position_ms": 0,
                }
            )
            return state.model_copy(update={"interrupt": new_interrupt})
        return _end_interrupt(state)

    return _mut


def interrupt_seek(position_ms: int) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        if state.interrupt is None:
            return state
        if state.interrupt.position_ms == position_ms:
            return state
        new_interrupt = state.interrupt.model_copy(update={"position_ms": position_ms})
        return state.model_copy(update={"interrupt": new_interrupt})

    return _mut


def cancel_interrupt() -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        if state.interrupt is None:
            return state
        return _end_interrupt(state)

    return _mut


# --- Scene composition mutators -------------------------------------------


def activate_scene(
    scene_id: str,
    *,
    ambient_track_ids: list[int] | None,
    crossfade_ms: int | None,
    presets: list[str] | None,
    volume: float | None = None,
) -> Any:
    """Composite mutator. Applies whichever of ambient/crossfade/presets/volume
    the scene defined, plus stamps active_scene_id, in a single update so
    clients see one state_changed broadcast for the whole transition.

    Captures the prior values of any field the scene overwrites into
    `pre_scene_state` so `deactivate_scene` can restore them. If a scene
    is already active when this fires, the existing snapshot is preserved
    - deactivating then unwinds back to the *first* scene's pre-state,
    not the intermediate one."""

    def _mut(state: PlayerState) -> PlayerState:
        update: dict = {"active_scene_id": scene_id}

        snapshot_kwargs: dict = {}
        if presets is not None:
            deduped: list[str] = []
            for p in presets:
                if p not in deduped:
                    deduped.append(p)
            update["active_preset_ids"] = deduped
            snapshot_kwargs["active_preset_ids"] = list(state.active_preset_ids)

        if crossfade_ms is not None:
            update["crossfade_ms"] = crossfade_ms
            snapshot_kwargs["crossfade_ms"] = state.crossfade_ms

        if volume is not None:
            update["volume"] = volume
            snapshot_kwargs["volume"] = state.volume

        if ambient_track_ids:
            new_ambient = AmbientState(
                current_track_id=ambient_track_ids[0],
                queue=list(ambient_track_ids[1:]),
                history=[],
                position_ms=0,
                loop=state.ambient.loop,
            )
            update["ambient"] = new_ambient
            update["is_playing"] = True
            snapshot_kwargs["ambient"] = state.ambient.model_copy(deep=True)

        # Preserve any existing snapshot when transitioning between scenes
        # so deactivate always returns to the original pre-scene state.
        if state.pre_scene_state is None and snapshot_kwargs:
            update["pre_scene_state"] = ScenePreviousState(**snapshot_kwargs)

        return state.model_copy(update=update)

    return _mut


def deactivate_scene() -> Any:
    """Clear active_scene_id and restore any pre-scene values captured at
    activation time. Fields the scene didn't change (snapshot value None)
    are left as-is."""

    def _mut(state: PlayerState) -> PlayerState:
        if state.active_scene_id is None and state.pre_scene_state is None:
            return state
        update: dict = {"active_scene_id": None, "pre_scene_state": None}
        snap = state.pre_scene_state
        if snap is not None:
            if snap.active_preset_ids is not None:
                update["active_preset_ids"] = list(snap.active_preset_ids)
            if snap.crossfade_ms is not None:
                update["crossfade_ms"] = snap.crossfade_ms
            if snap.volume is not None:
                update["volume"] = snap.volume
            if snap.ambient is not None:
                update["ambient"] = snap.ambient.model_copy(deep=True)
        return state.model_copy(update=update)

    return _mut
