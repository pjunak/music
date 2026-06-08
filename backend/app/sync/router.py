"""WebSocket endpoint for sync.

Authenticates via session cookie on upgrade. Sends a state snapshot
immediately, then enters a receive loop dispatching client actions. On
disconnect, removes the device from the registry.
"""
from __future__ import annotations

import asyncio
import contextlib
import logging
from datetime import UTC, datetime
from typing import Any

from fastapi import APIRouter, WebSocket, WebSocketDisconnect, status
from pydantic import ValidationError
from starlette.concurrency import run_in_threadpool

from app.core.config import get_settings
from app.core.db import SessionLocal
from app.devices.store import device_store
from app.domain import playlists as playlists_domain
from app.models.auth_session import AuthSession
from app.models.playlist import Playlist
from app.models.track import Track
from app.models.user import User
from app.modes import loader as modes_loader
from app.presets import loader as presets_loader
from app.sync import commit_and_broadcast
from app.sync import loops as loops_manager
from app.sync import state as state_module
from app.sync.connection import manager
from app.sync.devices import registry
from app.sync.protocol import (
    AmbientClearQueueAction,
    AmbientEnqueueAction,
    AmbientPlayPlaylistAction,
    AmbientPlayTrackAction,
    AmbientSeekAction,
    AmbientSetLoopAction,
    AmbientSetQueueAction,
    AmbientSetShuffleAction,
    AmbientSkipNextAction,
    AmbientSkipPrevAction,
    AmbientStopAction,
    CancelInterruptAction,
    ErrorMessage,
    FireCueAction,
    FireInterruptPlaylistAction,
    FireInterruptTrackAction,
    FireSfxAction,
    InterruptSeekAction,
    InterruptSkipNextAction,
    LoopingSfx,
    PauseAction,
    PositionReportAction,
    RegisterAction,
    ResumeAction,
    SetActiveModeAction,
    SetActiveOutputsAction,
    SetActivePresetsAction,
    SetActiveSoundboardAction,
    SetCrossfadeAction,
    SetDeviceVolumeAction,
    SetVolumeAction,
    SfxFired,
    StartLoopAction,
    StateSnapshot,
    StopLoopAction,
    action_adapter,
)

logger = logging.getLogger(__name__)
router = APIRouter()


async def _authenticate(
    websocket: WebSocket,
) -> tuple[User | None, datetime | None]:
    """Validate the session cookie. Returns (user, session_expires_at) on
    success, (None, None) for missing / invalid / expired cookies. The
    expiry timestamp is captured so the dispatch loop can re-check it
    mid-connection without an extra DB roundtrip."""
    cookie = websocket.cookies.get(get_settings().session_cookie_name)
    if not cookie:
        return (None, None)
    db = SessionLocal()
    try:
        session = db.get(AuthSession, cookie)
        if session is None or session.expires_at <= datetime.now(UTC):
            return (None, None)
        return (db.get(User, session.user_id), session.expires_at)
    finally:
        db.close()


async def _send(websocket: WebSocket, payload: Any) -> None:
    await websocket.send_json(payload.model_dump(mode="json"))


async def _send_error(websocket: WebSocket, detail: str) -> None:
    await _send(websocket, ErrorMessage(detail=detail))


async def _track_exists(track_id: int) -> bool:
    def _work() -> bool:
        db = SessionLocal()
        try:
            return db.get(Track, track_id) is not None
        finally:
            db.close()

    return await run_in_threadpool(_work)


async def _resolve_playlist_track_ids(playlist_id: int) -> tuple[list[int] | None, str | None]:
    """Returns (track_ids, error_detail). Either is set, never both."""

    def _work() -> tuple[list[int] | None, str | None]:
        db = SessionLocal()
        try:
            pl = db.get(Playlist, playlist_id)
            if pl is None:
                return (None, "playlist not found")
            items = playlists_domain.list_items(db, pl)
            return ([it.track_id for it in items], None)
        finally:
            db.close()

    return await run_in_threadpool(_work)


async def _resolve_playlist_by_name(
    name: str, mode_id: str
) -> tuple[list[int] | None, int | None, str | None]:
    """Resolve a cue's playlist reference (by name) to its track ids and id.
    Playlists are per-mode — only this mode's playlists are searched (no global
    fallback). Returns (track_ids, playlist_id, error_detail)."""
    from sqlalchemy import select

    def _work() -> tuple[list[int] | None, int | None, str | None]:
        db = SessionLocal()
        try:
            stmt = select(Playlist).where(
                Playlist.name == name,
                Playlist.mode_id == mode_id,
            )
            pl = db.scalars(stmt).first()
            if pl is None:
                return (None, None, f"no playlist named '{name}' for mode '{mode_id}'")
            items = playlists_domain.list_items(db, pl)
            return ([it.track_id for it in items], pl.id, None)
        finally:
            db.close()

    return await run_in_threadpool(_work)


async def _apply_and_broadcast(mutator: Any) -> None:
    await commit_and_broadcast(mutator)


# --- per-action handlers ---------------------------------------------------
#
# Each action type maps to one async handler with the signature
# `(action, device_id, websocket) -> None`. Handlers either succeed silently
# (the state machine broadcast does the announcing) or send an `error` frame
# back to the originating socket via `_send_error`. The dispatch dict at
# the bottom of this section is the registry — adding a new action means
# writing one handler and adding one line to `_DISPATCH`.


async def _h_register(
    action: RegisterAction, device_id: str, _ws: WebSocket
) -> None:
    # `device_id` here is the ephemeral connection id. Registering does NOT add
    # the device to the persistent list — that's a manual operator action. We
    # only look up whether this client_id is *already* a remembered output and
    # cache that onto the live connection. An unremembered device connects with
    # is_output=False and simply appears in `connected_devices` for the
    # operator to optionally save.
    is_output = device_store.is_output(action.client_id)
    registry.bind(
        device_id,
        client_id=action.client_id,
        name=action.name,
        is_output=is_output,
    )
    new_state = await state_module.machine.snapshot()
    await manager.broadcast_state(new_state)


async def _h_position_report(
    action: PositionReportAction, device_id: str, websocket: WebSocket
) -> None:
    if not registry.is_output_connection(device_id):
        await _send_error(
            websocket, "only designated output devices may report position"
        )
        return
    # Stamp telemetry under the stable client_id (informational only — surfaced
    # in Diagnostics), falling back to the connection id if somehow unbound.
    client_id = registry.client_id_for(device_id) or device_id
    await state_module.machine.apply(
        state_module.report_position(client_id, action.position_ms),
        SessionLocal,
        broadcast=False,
    )


async def _h_set_volume(
    action: SetVolumeAction, _device_id: str, _ws: WebSocket
) -> None:
    await _apply_and_broadcast(state_module.set_volume(action.volume))


async def _h_pause(_a: PauseAction, _device_id: str, _ws: WebSocket) -> None:
    await _apply_and_broadcast(state_module.set_is_playing(False))


async def _h_resume(_a: ResumeAction, _device_id: str, _ws: WebSocket) -> None:
    await _apply_and_broadcast(state_module.set_is_playing(True))


async def _h_set_active_mode(
    action: SetActiveModeAction, _device_id: str, websocket: WebSocket
) -> None:
    if action.mode_id is not None and modes_loader.get_mode(action.mode_id) is None:
        await _send_error(websocket, f"unknown mode: {action.mode_id}")
        return
    await _apply_and_broadcast(state_module.set_active_mode(action.mode_id))


async def _h_set_active_outputs(
    action: SetActiveOutputsAction, _device_id: str, websocket: WebSocket
) -> None:
    # `device_ids` are stable client_ids. Only validate ids being **added** by
    # this action against the persistent output designation — existing ids may
    # be stale references (left over across restarts) and re-validating them
    # would block every toggle. Validating against the persisted `is_output`
    # (not liveness) lets the operator pre-arm a TV that's currently asleep.
    current_state = await state_module.machine.snapshot()
    already_active = set(current_state.active_output_device_ids)
    new_ids = [d for d in action.device_ids if d not in already_active]
    if new_ids:
        designated = device_store.output_client_ids()
        bad = [d for d in new_ids if d not in designated]
        if bad:
            await _send_error(
                websocket, f"device(s) not designated as outputs: {bad}"
            )
            return
    await _apply_and_broadcast(state_module.set_active_outputs(action.device_ids))


async def _h_set_device_volume(
    action: SetDeviceVolumeAction, _device_id: str, _ws: WebSocket
) -> None:
    # Per-device trim is informational state every output reads; setting it for
    # a device that isn't currently an output is harmless (absent = 1.0, applied
    # only by that device when it plays), so no designation check is needed.
    await _apply_and_broadcast(
        state_module.set_device_volume(action.device_id, action.volume)
    )


# --- ambient lane ---------------------------------------------------------


async def _h_ambient_play_track(
    action: AmbientPlayTrackAction, _device_id: str, websocket: WebSocket
) -> None:
    if not await _track_exists(action.track_id):
        await _send_error(websocket, "track not in library")
        return
    await _apply_and_broadcast(state_module.ambient_play_track(action.track_id))


async def _h_ambient_set_queue(
    action: AmbientSetQueueAction, _device_id: str, _ws: WebSocket
) -> None:
    await _apply_and_broadcast(state_module.ambient_set_queue(action.track_ids))


async def _h_ambient_enqueue(
    action: AmbientEnqueueAction, _device_id: str, websocket: WebSocket
) -> None:
    if not await _track_exists(action.track_id):
        await _send_error(websocket, "track not in library")
        return
    await _apply_and_broadcast(
        state_module.ambient_enqueue(action.track_id, action.position)
    )


async def _h_ambient_clear_queue(
    _a: AmbientClearQueueAction, _device_id: str, _ws: WebSocket
) -> None:
    await _apply_and_broadcast(state_module.ambient_clear_queue())


async def _h_ambient_skip_next(
    _a: AmbientSkipNextAction, _device_id: str, _ws: WebSocket
) -> None:
    await _apply_and_broadcast(state_module.ambient_skip_next())


async def _h_ambient_skip_prev(
    _a: AmbientSkipPrevAction, _device_id: str, _ws: WebSocket
) -> None:
    await _apply_and_broadcast(state_module.ambient_skip_prev())


async def _h_ambient_seek(
    action: AmbientSeekAction, _device_id: str, _ws: WebSocket
) -> None:
    await _apply_and_broadcast(state_module.ambient_seek(action.position_ms))


async def _h_ambient_set_loop(
    action: AmbientSetLoopAction, _device_id: str, _ws: WebSocket
) -> None:
    await _apply_and_broadcast(state_module.ambient_set_loop(action.loop))


async def _h_ambient_set_shuffle(
    action: AmbientSetShuffleAction, _device_id: str, _ws: WebSocket
) -> None:
    await _apply_and_broadcast(state_module.ambient_set_shuffle(action.shuffle))


async def _h_ambient_stop(
    _a: AmbientStopAction, _device_id: str, _ws: WebSocket
) -> None:
    await _apply_and_broadcast(state_module.ambient_stop())


async def _h_ambient_play_playlist(
    action: AmbientPlayPlaylistAction, _device_id: str, websocket: WebSocket
) -> None:
    track_ids, err = await _resolve_playlist_track_ids(action.playlist_id)
    if err is not None:
        await _send_error(websocket, err)
        return
    if not track_ids:
        await _send_error(websocket, "playlist is empty")
        return
    await _apply_and_broadcast(
        state_module.ambient_play_playlist(
            track_ids, action.start_index, source_playlist_id=action.playlist_id
        )
    )


# --- mode-scoped session settings -----------------------------------------


async def _h_set_active_soundboard(
    action: SetActiveSoundboardAction, _device_id: str, websocket: WebSocket
) -> None:
    # If clearing, no validation needed.
    if action.soundboard_id is not None:
        current_state = await state_module.machine.snapshot()
        mode_id = current_state.active_mode_id
        if mode_id is None:
            await _send_error(
                websocket,
                "no active mode; cannot set soundboard without a mode",
            )
            return
        mode = modes_loader.get_mode(mode_id)
        if mode is None or action.soundboard_id not in mode.soundboards:
            await _send_error(
                websocket,
                f"soundboard '{action.soundboard_id}' not in active mode '{mode_id}'",
            )
            return
    await _apply_and_broadcast(
        state_module.set_active_soundboard(action.soundboard_id)
    )


async def _h_set_active_presets(
    action: SetActivePresetsAction, _device_id: str, websocket: WebSocket
) -> None:
    current_state = await state_module.machine.snapshot()
    mode = (
        modes_loader.get_mode(current_state.active_mode_id)
        if current_state.active_mode_id
        else None
    )
    loaded = mode.presets if mode is not None else {}
    unknown = [p for p in action.preset_ids if p not in loaded]
    if unknown:
        await _send_error(websocket, f"unknown preset(s): {unknown}")
        return
    volume, crossfade_ms = presets_loader.effective_overrides(loaded, action.preset_ids)
    await _apply_and_broadcast(
        state_module.set_active_presets(
            action.preset_ids, volume=volume, crossfade_ms=crossfade_ms
        )
    )


async def _h_set_crossfade(
    action: SetCrossfadeAction, _device_id: str, websocket: WebSocket
) -> None:
    if action.crossfade_type is not None and action.crossfade_type not in (
        "linear",
        "equal_power",
        "cut",
    ):
        await _send_error(
            websocket, f"invalid crossfade_type: {action.crossfade_type}"
        )
        return
    await _apply_and_broadcast(
        state_module.set_crossfade(action.crossfade_ms, action.crossfade_type)
    )


# --- interrupt lane -------------------------------------------------------


async def _h_fire_interrupt_track(
    action: FireInterruptTrackAction, _device_id: str, websocket: WebSocket
) -> None:
    if not await _track_exists(action.track_id):
        await _send_error(websocket, "track not in library")
        return
    await _apply_and_broadcast(
        state_module.fire_interrupt_track(
            action.track_id,
            action.return_to_ambient,
            action.fade_in_ms,
            action.fade_out_ms,
            duck_to=action.duck_to,
        )
    )


async def _h_fire_interrupt_playlist(
    action: FireInterruptPlaylistAction, _device_id: str, websocket: WebSocket
) -> None:
    track_ids, err = await _resolve_playlist_track_ids(action.playlist_id)
    if err is not None:
        await _send_error(websocket, err)
        return
    if not track_ids:
        await _send_error(websocket, "playlist is empty")
        return
    await _apply_and_broadcast(
        state_module.fire_interrupt_playlist(
            track_ids,
            action.return_to_ambient,
            action.fade_in_ms,
            action.fade_out_ms,
            duck_to=action.duck_to,
        )
    )


async def _h_interrupt_skip_next(
    _a: InterruptSkipNextAction, _device_id: str, _ws: WebSocket
) -> None:
    await _apply_and_broadcast(state_module.interrupt_skip_next())


async def _h_interrupt_seek(
    action: InterruptSeekAction, _device_id: str, _ws: WebSocket
) -> None:
    await _apply_and_broadcast(state_module.interrupt_seek(action.position_ms))


async def _h_cancel_interrupt(
    _a: CancelInterruptAction, _device_id: str, _ws: WebSocket
) -> None:
    await _apply_and_broadcast(state_module.cancel_interrupt())


# --- SFX (fire-and-forget) ------------------------------------------------


async def _h_fire_sfx(
    action: FireSfxAction, _device_id: str, websocket: WebSocket
) -> None:
    current_state = await state_module.machine.snapshot()
    err = _sfx_item_error(
        current_state.active_mode_id, action.soundboard_id, action.item_path
    )
    if err is not None:
        await _send_error(websocket, err)
        return
    event = SfxFired(
        soundboard_id=action.soundboard_id,
        item_path=action.item_path,
        volume=action.volume,
    )
    await manager.broadcast_to_outputs(event.model_dump(mode="json"))


def _sfx_item_error(mode_id: str | None, soundboard_id: str, item_path: str) -> str | None:
    """Validate an SFX reference against the active mode's soundboards (same
    gate as fire_sfx). Returns an error string, or None if valid."""
    if mode_id is None:
        return "no active mode"
    mode = modes_loader.get_mode(mode_id)
    if mode is None or soundboard_id not in mode.soundboards:
        return f"unknown soundboard '{soundboard_id}' in mode '{mode_id}'"
    soundboard = mode.soundboards[soundboard_id]
    if not any(
        item.file == item_path for cat in soundboard.categories for item in cat.items
    ):
        return f"item '{item_path}' not in soundboard '{soundboard_id}'"
    return None


async def _h_start_loop(
    action: StartLoopAction, _device_id: str, websocket: WebSocket
) -> None:
    current_state = await state_module.machine.snapshot()
    err = _sfx_item_error(
        current_state.active_mode_id, action.soundboard_id, action.item_path
    )
    if err is not None:
        await _send_error(websocket, err)
        return
    loops_manager.start(
        action.id,
        action.soundboard_id,
        action.item_path,
        action.interval_s,
        action.volume,
    )
    loop = LoopingSfx(
        id=action.id,
        name=action.name,
        soundboard_id=action.soundboard_id,
        item_path=action.item_path,
        interval_s=action.interval_s,
        volume=action.volume,
    )
    await _apply_and_broadcast(state_module.start_loop(loop))


async def _h_stop_loop(
    action: StopLoopAction, _device_id: str, websocket: WebSocket
) -> None:
    loops_manager.stop(action.id)
    await _apply_and_broadcast(state_module.stop_loop(action.id))


async def _h_fire_cue(
    action: FireCueAction, _device_id: str, websocket: WebSocket
) -> None:
    current_state = await state_module.machine.snapshot()
    mode_id = current_state.active_mode_id
    if mode_id is None:
        await _send_error(websocket, "no active mode")
        return
    mode = modes_loader.get_mode(mode_id)
    cue = mode.cues.get(action.cue_id) if mode is not None else None
    if cue is None:
        await _send_error(
            websocket, f"unknown cue '{action.cue_id}' in mode '{mode_id}'"
        )
        return

    # Validate every reference up front so a broken cue fails atomically rather
    # than half-applying.
    if cue.preset is not None and cue.preset not in mode.presets:
        await _send_error(websocket, f"cue references unknown preset '{cue.preset}'")
        return
    track_ids: list[int] | None = None
    source_playlist_id: int | None = None
    if cue.playlist is not None:
        track_ids, source_playlist_id, err = await _resolve_playlist_by_name(
            cue.playlist, mode_id
        )
        if err is not None:
            await _send_error(websocket, err)
            return
        if not track_ids:
            await _send_error(websocket, f"cue playlist '{cue.playlist}' is empty")
            return
    for ref in [*cue.sfx, *cue.loops]:
        err = _sfx_item_error(mode_id, ref.soundboard, ref.item)
        if err is not None:
            await _send_error(websocket, f"cue SFX: {err}")
            return

    # All valid — apply. Preset first (so its crossfade override is in place
    # before the playlist swap reads it), then the playlist, then loops.
    if cue.preset is not None:
        volume, crossfade_ms = presets_loader.effective_overrides(
            mode.presets, [cue.preset]
        )
        await _apply_and_broadcast(
            state_module.set_active_presets(
                [cue.preset], volume=volume, crossfade_ms=crossfade_ms
            )
        )
    if track_ids is not None:
        start_index = min(cue.start_index, len(track_ids) - 1)
        await _apply_and_broadcast(
            state_module.ambient_play_playlist(
                track_ids, start_index, source_playlist_id=source_playlist_id
            )
        )
        if cue.start_ms > 0:
            await _apply_and_broadcast(state_module.ambient_seek(cue.start_ms))
    for i, loop in enumerate(cue.loops):
        # Stable per-cue id so re-firing the same cue REPLACES its loops
        # (timer + state both dedup by id) instead of stacking duplicate
        # timers and duplicate LOOPS-panel rows on every press.
        loop_id = f"cue:{action.cue_id}:{i}"
        loops_manager.start(
            loop_id, loop.soundboard, loop.item, loop.interval_s, loop.volume
        )
        await _apply_and_broadcast(
            state_module.start_loop(
                LoopingSfx(
                    id=loop_id,
                    name=_sfx_display_name(mode, loop.soundboard, loop.item),
                    soundboard_id=loop.soundboard,
                    item_path=loop.item,
                    interval_s=loop.interval_s,
                    volume=loop.volume,
                )
            )
        )
    for sfx in cue.sfx:
        await manager.broadcast_to_outputs(
            SfxFired(
                soundboard_id=sfx.soundboard, item_path=sfx.item, volume=sfx.volume
            ).model_dump(mode="json")
        )


def _sfx_display_name(mode: Any, soundboard_id: str, item_path: str) -> str:
    """Friendly name for a soundboard item (for the LOOPS panel). Falls back to
    the item path if the lookup misses."""
    sb = mode.soundboards.get(soundboard_id) if mode is not None else None
    if sb is not None:
        for cat in sb.categories:
            for item in cat.items:
                if item.file == item_path:
                    return item.name
    return item_path


# --- dispatch registry ----------------------------------------------------
#
# Action-type → handler. Adding a new action is one new handler + one new
# entry here; the dispatch loop at the WS endpoint stays untouched. The
# value type is `Any` because each handler narrows its action arg to a
# specific subclass — Python is duck-typed at the call site, the dict
# is the contract.

_DISPATCH: dict[type, Any] = {
    RegisterAction: _h_register,
    PositionReportAction: _h_position_report,
    SetVolumeAction: _h_set_volume,
    PauseAction: _h_pause,
    ResumeAction: _h_resume,
    SetActiveModeAction: _h_set_active_mode,
    SetActiveOutputsAction: _h_set_active_outputs,
    SetDeviceVolumeAction: _h_set_device_volume,
    AmbientPlayTrackAction: _h_ambient_play_track,
    AmbientSetQueueAction: _h_ambient_set_queue,
    AmbientEnqueueAction: _h_ambient_enqueue,
    AmbientClearQueueAction: _h_ambient_clear_queue,
    AmbientSkipNextAction: _h_ambient_skip_next,
    AmbientSkipPrevAction: _h_ambient_skip_prev,
    AmbientSeekAction: _h_ambient_seek,
    AmbientSetLoopAction: _h_ambient_set_loop,
    AmbientSetShuffleAction: _h_ambient_set_shuffle,
    AmbientStopAction: _h_ambient_stop,
    AmbientPlayPlaylistAction: _h_ambient_play_playlist,
    SetActiveSoundboardAction: _h_set_active_soundboard,
    SetActivePresetsAction: _h_set_active_presets,
    SetCrossfadeAction: _h_set_crossfade,
    FireInterruptTrackAction: _h_fire_interrupt_track,
    FireInterruptPlaylistAction: _h_fire_interrupt_playlist,
    InterruptSkipNextAction: _h_interrupt_skip_next,
    InterruptSeekAction: _h_interrupt_seek,
    CancelInterruptAction: _h_cancel_interrupt,
    FireSfxAction: _h_fire_sfx,
    StartLoopAction: _h_start_loop,
    StopLoopAction: _h_stop_loop,
    FireCueAction: _h_fire_cue,
}


async def _dispatch(action: Any, device_id: str, websocket: WebSocket) -> None:
    """Apply an action to the state machine, broadcasting on success.
    Looks the action's class up in `_DISPATCH` and forwards. Unknown
    actions get a one-line error frame — should be unreachable in practice
    because `action_adapter` already rejected anything off-schema."""
    handler = _DISPATCH.get(type(action))
    if handler is None:
        await _send_error(websocket, f"unhandled action type: {type(action).__name__}")
        return
    await handler(action, device_id, websocket)


@router.websocket("/api/ws")
async def ws_endpoint(websocket: WebSocket) -> None:
    # Guests (no/invalid session cookie) get a read-only socket — they
    # receive state broadcasts and SFX events so a logged-out Player tab
    # (TV bookmark) can act as an audio output, but any mutating action
    # they send is rejected. Keeps internet-exposed deployments from
    # giving anonymous viewers control over playback.
    user, session_expires_at = await _authenticate(websocket)
    is_guest = user is None

    await websocket.accept()

    device = registry.add()
    manager.add(device.connection_id, websocket)

    # The snapshot goes out before the client's `register` (which carries the
    # client_id), so the server can't name the device here. The client knows
    # its own stable client_id and self-assigns identity — your_device_id stays
    # empty (see StateSnapshot).
    snapshot_state = await state_module.machine.snapshot()
    await _send(websocket, StateSnapshot(state=snapshot_state))

    try:
        while True:
            try:
                raw = await websocket.receive_json()
            except WebSocketDisconnect:
                break
            except (ValueError, TypeError):
                await _send_error(websocket, "malformed JSON")
                continue

            try:
                action = action_adapter.validate_python(raw)
            except ValidationError as e:
                await _send_error(websocket, f"invalid action: {e.errors()[0]['msg']}")
                continue

            # Re-check session expiry on each action — the cookie that
            # was valid at WS upgrade may have expired since (long-lived
            # tabs, especially). Once expired, downgrade the connection
            # to guest mode so the next mutation surfaces the same
            # "guest cannot mutate" error path.
            if (
                not is_guest
                and session_expires_at is not None
                and datetime.now(UTC) >= session_expires_at
            ):
                is_guest = True
                session_expires_at = None
                await _send_error(
                    websocket, "session expired — please sign in again"
                )
                # Fall through so the action gets the standard guest rejection
                # below (or, for register, succeeds).

            # Guests can register themselves (so the device appears in the
            # operator's device list and can be designated as an output) but
            # nothing else.
            if is_guest and not isinstance(action, RegisterAction):
                await _send_error(
                    websocket, "guest sessions cannot mutate state — please sign in"
                )
                continue

            try:
                await _dispatch(action, device.connection_id, websocket)
            except Exception as e:
                # Single-user home server — surface the exception type +
                # message in the error frame so the operator sees the
                # actual cause via the toast layer instead of "internal
                # error" with a stack trace only in the server log.
                logger.exception("dispatch failed for action %r", action)
                await _send_error(
                    websocket, f"{type(e).__name__}: {e}" if str(e) else type(e).__name__
                )
    finally:
        client_id = registry.client_id_for(device.connection_id)
        manager.remove(device.connection_id)
        registry.remove(device.connection_id)
        # Prune the disconnecting device from active_output_device_ids — but
        # only when its *last* tab closes. Closing one tab of a device that has
        # another output tab open shouldn't silence it. The persistent
        # is_output designation is untouched; on reconnect the device sits
        # inactive (fully-manual: no auto-resume) until re-activated.
        if client_id is not None and not registry.has_other_connection(
            client_id, device.connection_id
        ):
            with contextlib.suppress(Exception):
                await commit_and_broadcast(
                    state_module.remove_active_output(client_id)
                )
        with contextlib.suppress(Exception):
            new_state = await state_module.machine.snapshot()
            await manager.broadcast_state(new_state)
        await asyncio.sleep(0)
        if websocket.client_state.value not in (2, 3):
            with contextlib.suppress(Exception):
                await websocket.close(code=status.WS_1000_NORMAL_CLOSURE)
