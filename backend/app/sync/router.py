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
from app.domain import playlists as playlists_domain
from app.models.auth_session import AuthSession
from app.models.playlist import Playlist
from app.models.track import Track
from app.models.user import User
from app.modes import loader as modes_loader
from app.presets import loader as presets_loader
from app.sync import commit_and_broadcast
from app.sync import state as state_module
from app.sync.connection import manager
from app.sync.devices import registry
from app.sync.protocol import (
    ActivateSceneAction,
    AmbientClearQueueAction,
    AmbientEnqueueAction,
    AmbientPlayPlaylistAction,
    AmbientPlayTrackAction,
    AmbientSeekAction,
    AmbientSetLoopAction,
    AmbientSetQueueAction,
    AmbientSkipNextAction,
    AmbientSkipPrevAction,
    AmbientStopAction,
    CancelInterruptAction,
    DeactivateSceneAction,
    ErrorMessage,
    FireInterruptPlaylistAction,
    FireInterruptTrackAction,
    FireSfxAction,
    InterruptSeekAction,
    InterruptSkipNextAction,
    PauseAction,
    PositionReportAction,
    RegisterAction,
    ResumeAction,
    SceneActivated,
    SceneDeactivated,
    SetActiveModeAction,
    SetActiveOutputsAction,
    SetActivePresetsAction,
    SetActiveSoundboardAction,
    SetCrossfadeAction,
    SetVolumeAction,
    SfxFired,
    StateSnapshot,
    action_adapter,
)

logger = logging.getLogger(__name__)
router = APIRouter()


async def _authenticate(websocket: WebSocket) -> User | None:
    """Validate the session cookie. Returns the user on success, None otherwise."""
    cookie = websocket.cookies.get(get_settings().session_cookie_name)
    if not cookie:
        return None
    db = SessionLocal()
    try:
        session = db.get(AuthSession, cookie)
        if session is None or session.expires_at <= datetime.now(UTC):
            return None
        return db.get(User, session.user_id)
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
) -> tuple[list[int] | None, str | None]:
    """Resolve a scene's playlist reference (by name). Looks for a playlist
    in the given mode OR a global playlist (mode_id=null) — same scoping
    semantics as the playlists list endpoint's default."""
    from sqlalchemy import or_, select

    def _work() -> tuple[list[int] | None, str | None]:
        db = SessionLocal()
        try:
            stmt = select(Playlist).where(
                Playlist.name == name,
                or_(Playlist.mode_id == mode_id, Playlist.mode_id.is_(None)),
            )
            pl = db.scalars(stmt).first()
            if pl is None:
                return (None, f"no playlist named '{name}' for mode '{mode_id}'")
            items = playlists_domain.list_items(db, pl)
            return ([it.track_id for it in items], None)
        finally:
            db.close()

    return await run_in_threadpool(_work)


async def _apply_and_broadcast(mutator: Any) -> None:
    await commit_and_broadcast(mutator)


async def _dispatch(action: Any, device_id: str, websocket: WebSocket) -> None:
    """Apply an action to the state machine, broadcasting on success."""
    if isinstance(action, RegisterAction):
        registry.update(device_id, name=action.name, capabilities=action.capabilities)
        new_state = await state_module.machine.snapshot()
        await manager.broadcast_state(new_state)
        return

    if isinstance(action, PositionReportAction):
        if not registry.has_capability(device_id, "audio_output"):
            await _send_error(websocket, "only audio_output devices may report position")
            return
        await state_module.machine.apply(
            state_module.report_position(device_id, action.position_ms),
            SessionLocal,
            broadcast=False,
        )
        return

    if isinstance(action, SetVolumeAction):
        await _apply_and_broadcast(state_module.set_volume(action.volume))
        return

    if isinstance(action, PauseAction):
        await _apply_and_broadcast(state_module.set_is_playing(False))
        return

    if isinstance(action, ResumeAction):
        await _apply_and_broadcast(state_module.set_is_playing(True))
        return

    if isinstance(action, SetActiveModeAction):
        if action.mode_id is not None and modes_loader.get_mode(action.mode_id) is None:
            await _send_error(websocket, f"unknown mode: {action.mode_id}")
            return
        await _apply_and_broadcast(state_module.set_active_mode(action.mode_id))
        return

    if isinstance(action, SetActiveOutputsAction):
        # Only validate device ids being **added** by this action. Existing
        # ids in active_output_device_ids may be stale references to
        # devices that disconnected long ago (e.g. left over in
        # playback_state across server restarts) — re-validating them
        # would block every toggle attempt for the user.
        current_state = await state_module.machine.snapshot()
        already_active = set(current_state.active_output_device_ids)
        new_ids = [d for d in action.device_ids if d not in already_active]
        bad = [
            d for d in new_ids if not registry.has_capability(d, "audio_output")
        ]
        if bad:
            await _send_error(
                websocket, f"device(s) not registered with audio_output: {bad}"
            )
            return
        await _apply_and_broadcast(state_module.set_active_outputs(action.device_ids))
        return

    # --- ambient lane -----------------------------------------------------

    if isinstance(action, AmbientPlayTrackAction):
        if not await _track_exists(action.track_id):
            await _send_error(websocket, "track not in library")
            return
        await _apply_and_broadcast(state_module.ambient_play_track(action.track_id))
        return

    if isinstance(action, AmbientSetQueueAction):
        await _apply_and_broadcast(state_module.ambient_set_queue(action.track_ids))
        return

    if isinstance(action, AmbientEnqueueAction):
        if not await _track_exists(action.track_id):
            await _send_error(websocket, "track not in library")
            return
        await _apply_and_broadcast(
            state_module.ambient_enqueue(action.track_id, action.position)
        )
        return

    if isinstance(action, AmbientClearQueueAction):
        await _apply_and_broadcast(state_module.ambient_clear_queue())
        return

    if isinstance(action, AmbientSkipNextAction):
        await _apply_and_broadcast(state_module.ambient_skip_next())
        return

    if isinstance(action, AmbientSkipPrevAction):
        await _apply_and_broadcast(state_module.ambient_skip_prev())
        return

    if isinstance(action, AmbientSeekAction):
        await _apply_and_broadcast(state_module.ambient_seek(action.position_ms))
        return

    if isinstance(action, AmbientSetLoopAction):
        await _apply_and_broadcast(state_module.ambient_set_loop(action.loop))
        return

    if isinstance(action, AmbientStopAction):
        await _apply_and_broadcast(state_module.ambient_stop())
        return

    if isinstance(action, AmbientPlayPlaylistAction):
        track_ids, err = await _resolve_playlist_track_ids(action.playlist_id)
        if err is not None:
            await _send_error(websocket, err)
            return
        if not track_ids:
            await _send_error(websocket, "playlist is empty")
            return
        await _apply_and_broadcast(
            state_module.ambient_play_playlist(track_ids, action.start_index)
        )
        return

    # --- mode-scoped session settings -------------------------------------

    if isinstance(action, SetActiveSoundboardAction):
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
        return

    if isinstance(action, SetActivePresetsAction):
        loaded = presets_loader.all_presets()
        unknown = [p for p in action.preset_ids if p not in loaded]
        if unknown:
            await _send_error(websocket, f"unknown preset(s): {unknown}")
            return
        await _apply_and_broadcast(state_module.set_active_presets(action.preset_ids))
        return

    if isinstance(action, SetCrossfadeAction):
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
        return

    # --- interrupt lane ---------------------------------------------------

    if isinstance(action, FireInterruptTrackAction):
        if not await _track_exists(action.track_id):
            await _send_error(websocket, "track not in library")
            return
        await _apply_and_broadcast(
            state_module.fire_interrupt_track(
                action.track_id,
                action.return_to_ambient,
                action.fade_in_ms,
                action.fade_out_ms,
            )
        )
        return

    if isinstance(action, FireInterruptPlaylistAction):
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
            )
        )
        return

    if isinstance(action, InterruptSkipNextAction):
        await _apply_and_broadcast(state_module.interrupt_skip_next())
        return

    if isinstance(action, InterruptSeekAction):
        await _apply_and_broadcast(state_module.interrupt_seek(action.position_ms))
        return

    if isinstance(action, CancelInterruptAction):
        await _apply_and_broadcast(state_module.cancel_interrupt())
        return

    # --- SFX (fire-and-forget) -------------------------------------------

    # --- scene activation ------------------------------------------------

    if isinstance(action, ActivateSceneAction):
        current_state = await state_module.machine.snapshot()
        mode_id = current_state.active_mode_id
        if mode_id is None:
            await _send_error(websocket, "no active mode")
            return
        mode = modes_loader.get_mode(mode_id)
        if mode is None or action.scene_id not in mode.scenes:
            await _send_error(
                websocket, f"unknown scene '{action.scene_id}' in mode '{mode_id}'"
            )
            return

        scene = mode.scenes[action.scene_id]

        # Validate presets are loaded.
        scene_presets = scene.presets or []
        loaded = presets_loader.all_presets()
        unknown = [p for p in scene_presets if p not in loaded]
        if unknown:
            await _send_error(websocket, f"scene references unknown preset(s): {unknown}")
            return

        # Resolve ambient (optional).
        ambient_track_ids: list[int] | None = None
        crossfade_ms: int | None = None
        if scene.ambient:
            playlist_name = scene.ambient.get("playlist")
            if playlist_name:
                track_ids, err = await _resolve_playlist_by_name(playlist_name, mode_id)
                if err is not None:
                    await _send_error(websocket, err)
                    return
                if not track_ids:
                    await _send_error(
                        websocket, f"playlist '{playlist_name}' resolved to empty"
                    )
                    return
                ambient_track_ids = track_ids
            cf = scene.ambient.get("crossfade_ms")
            if isinstance(cf, int):
                crossfade_ms = cf

        await _apply_and_broadcast(
            state_module.activate_scene(
                action.scene_id,
                ambient_track_ids=ambient_track_ids,
                crossfade_ms=crossfade_ms,
                presets=scene_presets if scene_presets else None,
            )
        )

        # Broadcast the full scene definition so audio engine + integrations
        # can act on looping_sfx / lights / external. Goes to all clients —
        # any consumer (UI showing the active scene, audio engine starting
        # loops, future MQTT bridge firing externals) might care.
        scene_payload = scene.model_dump()
        scene_payload.pop("id", None)  # already in the event envelope
        event = SceneActivated(
            scene_id=action.scene_id, mode_id=mode_id, scene=scene_payload
        )
        await manager.broadcast(event.model_dump(mode="json"))
        return

    if isinstance(action, DeactivateSceneAction):
        current_state = await state_module.machine.snapshot()
        prev_scene_id = current_state.active_scene_id
        prev_mode_id = current_state.active_mode_id
        if prev_scene_id is None:
            return  # noop
        await _apply_and_broadcast(state_module.deactivate_scene())
        event = SceneDeactivated(scene_id=prev_scene_id, mode_id=prev_mode_id)
        await manager.broadcast(event.model_dump(mode="json"))
        return

    if isinstance(action, FireSfxAction):
        current_state = await state_module.machine.snapshot()
        mode_id = current_state.active_mode_id
        if mode_id is None:
            await _send_error(websocket, "no active mode")
            return
        mode = modes_loader.get_mode(mode_id)
        if mode is None or action.soundboard_id not in mode.soundboards:
            await _send_error(
                websocket,
                f"unknown soundboard '{action.soundboard_id}' in mode '{mode_id}'",
            )
            return
        soundboard = mode.soundboards[action.soundboard_id]
        if not any(
            item.file == action.item_path
            for cat in soundboard.categories
            for item in cat.items
        ):
            await _send_error(
                websocket,
                f"item '{action.item_path}' not in soundboard '{action.soundboard_id}'",
            )
            return
        event = SfxFired(
            soundboard_id=action.soundboard_id,
            item_path=action.item_path,
            volume=action.volume,
        )
        await manager.broadcast_to_capability(
            "audio_output", event.model_dump(mode="json")
        )
        return

    await _send_error(websocket, f"unhandled action type: {type(action).__name__}")


@router.websocket("/api/ws")
async def ws_endpoint(websocket: WebSocket) -> None:
    # Guests (no/invalid session cookie) get a read-only socket — they
    # receive state broadcasts and SFX events so a logged-out Player tab
    # (TV bookmark) can act as an audio output, but any mutating action
    # they send is rejected. Keeps internet-exposed deployments from
    # giving anonymous viewers control over playback.
    user = await _authenticate(websocket)
    is_guest = user is None

    await websocket.accept()

    device = registry.add()
    manager.add(device.device_id, websocket)

    snapshot_state = await state_module.machine.snapshot()
    await _send(websocket, StateSnapshot(your_device_id=device.device_id, state=snapshot_state))

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

            # Guests can register themselves (so the device shows up as a
            # potential audio_output) but nothing else.
            if is_guest and not isinstance(action, RegisterAction):
                await _send_error(
                    websocket, "guest sessions cannot mutate state — please sign in"
                )
                continue

            try:
                await _dispatch(action, device.device_id, websocket)
            except Exception:
                logger.exception("dispatch failed for action %r", action)
                await _send_error(websocket, "internal error")
    finally:
        manager.remove(device.device_id)
        registry.remove(device.device_id)
        # Prune the disconnecting device from active_output_device_ids so
        # the operator's next session doesn't inherit a stale reference
        # that blocks toggle attempts (see set_active_outputs validation).
        with contextlib.suppress(Exception):
            await commit_and_broadcast(
                state_module.remove_active_output(device.device_id)
            )
        with contextlib.suppress(Exception):
            new_state = await state_module.machine.snapshot()
            await manager.broadcast_state(new_state)
        await asyncio.sleep(0)
        if websocket.client_state.value not in (2, 3):
            with contextlib.suppress(Exception):
                await websocket.close(code=status.WS_1000_NORMAL_CLOSURE)
