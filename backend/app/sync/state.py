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
)

logger = logging.getLogger(__name__)


class StateMachine:
    def __init__(self) -> None:
        self._state = PlayerState()
        self._lock = asyncio.Lock()
        self._loaded = False

    async def load(self, db_factory: Any) -> None:
        """Hydrate state from the playback_state DB row. Call once at startup."""

        def _load() -> dict:
            db: Session = db_factory()
            try:
                return playback_state_domain.get_state(db)
            finally:
                db.close()

        raw = await run_in_threadpool(_load)
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
            # Normal advance.
            new_history = list(amb.history)
            if amb.current_track_id is not None:
                new_history.append(amb.current_track_id)
            new_current = amb.queue[0]
            new_queue = amb.queue[1:]
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

        # Loop off, end of queue: clear current, become idle.
        return _replace_ambient(
            state,
            amb.model_copy(
                update={"current_track_id": None, "position_ms": 0}
            ),
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
) -> Any:
    def _mut(state: PlayerState) -> PlayerState:
        new_interrupt = InterruptState(
            current_track_id=track_id,
            queue=[],
            position_ms=0,
            return_to_ambient=return_to_ambient,
            fade_in_ms=fade_in_ms,
            fade_out_ms=fade_out_ms,
        )
        return state.model_copy(update={"interrupt": new_interrupt, "is_playing": True})

    return _mut


def fire_interrupt_playlist(
    track_ids: list[int],
    return_to_ambient: bool,
    fade_in_ms: int,
    fade_out_ms: int,
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
) -> Any:
    """Composite mutator. Applies whichever of ambient/crossfade/presets
    the scene defined, plus stamps active_scene_id, in a single update so
    clients see one state_changed broadcast for the whole transition."""

    def _mut(state: PlayerState) -> PlayerState:
        update: dict = {"active_scene_id": scene_id}

        if presets is not None:
            deduped: list[str] = []
            for p in presets:
                if p not in deduped:
                    deduped.append(p)
            update["active_preset_ids"] = deduped

        if crossfade_ms is not None:
            update["crossfade_ms"] = crossfade_ms

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

        return state.model_copy(update=update)

    return _mut


def deactivate_scene() -> Any:
    """Clear active_scene_id only. Doesn't revert state changes the scene
    applied (presets, ambient, crossfade) — those are sticky until the
    user changes them or activates another scene."""

    def _mut(state: PlayerState) -> PlayerState:
        if state.active_scene_id is None:
            return state
        return state.model_copy(update={"active_scene_id": None})

    return _mut
