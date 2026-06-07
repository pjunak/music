"""Message schemas for the WebSocket sync protocol.

These schemas are the canonical contract. Discriminated on `type`; pydantic
parses the payload into the right concrete class.
"""
from __future__ import annotations

from typing import Annotated, Literal

from pydantic import BaseModel, Field, TypeAdapter

# --- Client → server: actions ----------------------------------------------


class _Action(BaseModel):
    """Marker base; concrete actions narrow the `type` literal."""


class RegisterAction(_Action):
    type: Literal["register"]
    name: str = Field(min_length=1, max_length=128)
    # Stable per-install identity the client mints once (localStorage). It's
    # what makes an audio-output designation stick to a physical device across
    # reconnects. Validated only as a non-empty opaque token (not a strict
    # UUID / length) so the documented headless-output protocol
    # (clients/README.md) stays open to any stable string.
    client_id: str = Field(min_length=1, max_length=64)
    # Accepted for wire-compat with older clients but ignored — output
    # eligibility now lives in the persistent device registry (`is_output`),
    # not in a self-asserted capability.
    capabilities: list[str] = Field(default_factory=list)


class SetVolumeAction(_Action):
    type: Literal["set_volume"]
    volume: float = Field(ge=0.0, le=1.0)


class PauseAction(_Action):
    type: Literal["pause"]


class ResumeAction(_Action):
    type: Literal["resume"]


class SetActiveModeAction(_Action):
    type: Literal["set_active_mode"]
    mode_id: str | None


class SetActiveOutputsAction(_Action):
    type: Literal["set_active_outputs"]
    device_ids: list[str] = Field(default_factory=list)


class SetDeviceVolumeAction(_Action):
    type: Literal["set_device_volume"]
    # The output device whose per-device trim to set (a stable client_id).
    device_id: str = Field(min_length=1, max_length=64)
    # 0..1 trim, multiplied by master volume on that device only. 1.0 = no trim.
    volume: float = Field(ge=0.0, le=1.0)


class PositionReportAction(_Action):
    type: Literal["position_report"]
    position_ms: int = Field(ge=0)


# Ambient lane actions. The "ambient" lane is the always-on background music
# the user is playing. Interrupt and SFX lanes land in subsequent commits.


class AmbientPlayTrackAction(_Action):
    type: Literal["ambient_play_track"]
    track_id: int


class AmbientSetQueueAction(_Action):
    type: Literal["ambient_set_queue"]
    track_ids: list[int] = Field(default_factory=list)


class AmbientEnqueueAction(_Action):
    type: Literal["ambient_enqueue"]
    track_id: int
    position: int | None = None  # None = append at end


class AmbientClearQueueAction(_Action):
    type: Literal["ambient_clear_queue"]


class AmbientSkipNextAction(_Action):
    type: Literal["ambient_skip_next"]


class AmbientSkipPrevAction(_Action):
    type: Literal["ambient_skip_prev"]


class AmbientSeekAction(_Action):
    type: Literal["ambient_seek"]
    position_ms: int = Field(ge=0)


class AmbientSetLoopAction(_Action):
    type: Literal["ambient_set_loop"]
    loop: Literal["off", "queue", "track"]


class AmbientSetShuffleAction(_Action):
    type: Literal["ambient_set_shuffle"]
    # "weighted" picks randomly too for now — the weighting (by play count /
    # recency) is the to-be-implemented refinement, not a separate behaviour.
    shuffle: Literal["off", "random", "weighted"]


class AmbientStopAction(_Action):
    type: Literal["ambient_stop"]


class AmbientPlayPlaylistAction(_Action):
    type: Literal["ambient_play_playlist"]
    playlist_id: int
    start_index: int = Field(0, ge=0)


class SetActiveSoundboardAction(_Action):
    type: Literal["set_active_soundboard"]
    soundboard_id: str | None  # null clears the active soundboard


class SetActivePresetsAction(_Action):
    type: Literal["set_active_presets"]
    preset_ids: list[str] = Field(default_factory=list)


class SetCrossfadeAction(_Action):
    type: Literal["set_crossfade"]
    crossfade_ms: int = Field(ge=0, le=30000)
    crossfade_type: str | None = None  # None = leave unchanged


# Interrupt lane actions. An interrupt takes over playback briefly; when it
# completes (or is cancelled) the active lane swaps back to ambient (unless
# `return_to_ambient=false`, in which case playback stops).


class FireInterruptTrackAction(_Action):
    type: Literal["fire_interrupt_track"]
    # See InterruptState.duck_to. Optional override per fire — falls back
    # to the template / current-state default when None (= ambient pauses).
    duck_to: float | None = Field(None, ge=0.0, le=1.0)
    track_id: int
    return_to_ambient: bool = True
    fade_in_ms: int = Field(0, ge=0, le=10000)
    fade_out_ms: int = Field(0, ge=0, le=10000)


class FireInterruptPlaylistAction(_Action):
    type: Literal["fire_interrupt_playlist"]
    playlist_id: int
    return_to_ambient: bool = True
    fade_in_ms: int = Field(0, ge=0, le=10000)
    fade_out_ms: int = Field(0, ge=0, le=10000)
    duck_to: float | None = Field(None, ge=0.0, le=1.0)


class InterruptSkipNextAction(_Action):
    type: Literal["interrupt_skip_next"]


class InterruptSeekAction(_Action):
    type: Literal["interrupt_seek"]
    position_ms: int = Field(ge=0)


class CancelInterruptAction(_Action):
    type: Literal["cancel_interrupt"]


class FireSfxAction(_Action):
    type: Literal["fire_sfx"]
    soundboard_id: str = Field(min_length=1, max_length=128)
    item_path: str = Field(min_length=1, max_length=512)
    volume: float = Field(1.0, ge=0.0, le=1.0)


class StartLoopAction(_Action):
    """Start a repeating SFX. A server-side timer fires it every `interval_s`
    until `stop_loop`. `id` is a client-minted handle (so the same client can
    stop it). Lives in PlayerState so every LOOPS panel sees it."""

    type: Literal["start_loop"]
    id: str = Field(min_length=1, max_length=64)
    name: str = Field(min_length=1, max_length=128)
    soundboard_id: str = Field(min_length=1, max_length=128)
    item_path: str = Field(min_length=1, max_length=512)
    interval_s: float = Field(ge=1, le=3600)
    volume: float = Field(1.0, ge=0.0, le=1.0)


class StopLoopAction(_Action):
    type: Literal["stop_loop"]
    id: str = Field(min_length=1, max_length=64)


class FireCueAction(_Action):
    """Fire a saved cue from the active mode: apply its preset, start its
    playlist (from a song + timestamp), fire one-shot SFX, and start loops."""

    type: Literal["fire_cue"]
    cue_id: str = Field(min_length=1, max_length=128)


Action = Annotated[
    RegisterAction
    | SetVolumeAction
    | PauseAction
    | ResumeAction
    | SetActiveModeAction
    | SetActiveOutputsAction
    | SetDeviceVolumeAction
    | PositionReportAction
    | AmbientPlayTrackAction
    | AmbientSetQueueAction
    | AmbientEnqueueAction
    | AmbientClearQueueAction
    | AmbientSkipNextAction
    | AmbientSkipPrevAction
    | AmbientSeekAction
    | AmbientSetLoopAction
    | AmbientSetShuffleAction
    | AmbientStopAction
    | AmbientPlayPlaylistAction
    | SetActiveSoundboardAction
    | SetActivePresetsAction
    | SetCrossfadeAction
    | FireInterruptTrackAction
    | FireInterruptPlaylistAction
    | InterruptSkipNextAction
    | InterruptSeekAction
    | CancelInterruptAction
    | FireSfxAction
    | StartLoopAction
    | StopLoopAction
    | FireCueAction,
    Field(discriminator="type"),
]

action_adapter: TypeAdapter[Action] = TypeAdapter(Action)


# --- Server → client: snapshots, state, errors -----------------------------


class DeviceInfo(BaseModel):
    """A currently-connected device. `device_id` carries the stable `client_id`
    (they're the same value now) so existing clients that key on `device_id`
    keep working unchanged. `is_output` reflects the persistent registry
    designation."""

    device_id: str
    client_id: str
    name: str
    is_output: bool = False


class PositionReport(BaseModel):
    device_id: str
    position_ms: int
    reported_at: float  # epoch seconds


LoopMode = Literal["off", "queue", "track"]
ShuffleMode = Literal["off", "random", "weighted"]


class AmbientState(BaseModel):
    """The "main player" lane. Holds the current track, what's coming up,
    and what's already played (for skip_prev). `position_ms` is the server's
    best guess of where in the current track playback is — updated by user
    actions (play/seek/skip) and by position reports from the active output.

    While an interrupt is active, `position_ms` is *not* updated (position
    reports go to the interrupt lane), so it preserves the resume point
    automatically.

    `shuffle` controls how `skip_next` picks the next track: "off" advances
    sequentially (queue head); "random"/"weighted" pull a random queue entry.
    Weighting is not yet implemented — "weighted" currently behaves as uniform
    random, so the control is usable without the algorithm landing first.
    """

    current_track_id: int | None = None
    queue: list[int] = Field(default_factory=list)
    history: list[int] = Field(default_factory=list)
    position_ms: int = 0
    loop: LoopMode = "off"
    shuffle: ShuffleMode = "off"
    # The playlist this lane was started from, so the Console can show which
    # quick-play playlist is "now driving". Set by `ambient_play_playlist`,
    # cleared whenever the queue is replaced from another source (play a single
    # track, set the queue explicitly, stop). Skips/seek keep it.
    source_playlist_id: int | None = None


class InterruptState(BaseModel):
    """An interrupt is a short audio override. Its lifecycle ends either by
    queue exhaustion (auto-completion via `interrupt_skip_next` past the end)
    or explicit cancel. On completion, behavior depends on
    `return_to_ambient`: true → ambient takes back over from its preserved
    position; false → playback stops.

    `duck_to` controls how ambient behaves *during* the interrupt:
    - `None` (default): ambient pauses - full cut, position frozen.
    - `0.0`..`1.0`: ambient keeps playing at this volume multiplier
      (relative to the master), creating a cinematic duck. Same fade
      durations as the interrupt's fade_in/out drive the ramp.
    """

    current_track_id: int
    queue: list[int] = Field(default_factory=list)
    position_ms: int = 0
    return_to_ambient: bool = True
    fade_in_ms: int = 0
    fade_out_ms: int = 0
    duck_to: float | None = Field(None, ge=0.0, le=1.0)


class LoopingSfx(BaseModel):
    """A repeating SFX driven by a server-side interval timer. Held in
    PlayerState so every client's LOOPS panel shows the same live set; the
    actual timers live in `sync/loops.py`. `id` is the stop handle."""

    id: str
    name: str
    soundboard_id: str
    item_path: str
    interval_s: float = Field(ge=1, le=3600)
    volume: float = Field(1.0, ge=0.0, le=1.0)


class PlayerState(BaseModel):
    """Canonical playback state. The server is the sole writer."""

    revision: int = 0
    is_playing: bool = False
    volume: float = 1.0

    active_mode_id: str | None = None
    active_output_device_ids: list[str] = Field(default_factory=list)
    # Per-device volume trim (client_id → 0..1), applied on top of master
    # `volume` on that device only. Absent = 1.0. Lets the operator tame a
    # too-loud TV without touching master. Only non-unity trims are stored.
    device_volumes: dict[str, float] = Field(default_factory=dict)
    active_soundboard_id: str | None = None
    active_preset_ids: list[str] = Field(default_factory=list)

    crossfade_ms: int = 0
    crossfade_type: str = "linear"

    ambient: AmbientState = Field(default_factory=AmbientState)
    interrupt: InterruptState | None = None

    # Repeating SFX currently running (server-timer driven). Cleared on boot
    # (session-only, like active outputs — no auto-resume across a restart).
    looping_sfx: list[LoopingSfx] = Field(default_factory=list)

    last_position_report: PositionReport | None = None

    connected_devices: list[DeviceInfo] = Field(default_factory=list)


class StateSnapshot(BaseModel):
    type: Literal["state_snapshot"] = "state_snapshot"
    # Empty by design: the snapshot is sent *before* the client's `register`
    # arrives, so the server doesn't yet know its client_id. The client knows
    # its own stable client_id (localStorage) and self-assigns `myDeviceId`
    # from that — see frontend playerStore. Kept as a (possibly empty) string
    # so the structural WS guard's type check is satisfied without change.
    your_device_id: str = ""
    state: PlayerState


class StateChanged(BaseModel):
    type: Literal["state_changed"] = "state_changed"
    state: PlayerState


class SfxFired(BaseModel):
    """Fire-and-forget event for soundboard items. Not part of PlayerState —
    broadcast only to clients with the `audio_output` capability so each plays
    the SFX locally."""

    type: Literal["sfx_fired"] = "sfx_fired"
    soundboard_id: str
    item_path: str
    volume: float = 1.0


class ErrorMessage(BaseModel):
    type: Literal["error"] = "error"
    detail: str
