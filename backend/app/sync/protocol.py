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


class ActivateSceneAction(_Action):
    type: Literal["activate_scene"]
    scene_id: str = Field(min_length=1, max_length=128)


class DeactivateSceneAction(_Action):
    type: Literal["deactivate_scene"]


Action = Annotated[
    RegisterAction
    | SetVolumeAction
    | PauseAction
    | ResumeAction
    | SetActiveModeAction
    | SetActiveOutputsAction
    | PositionReportAction
    | AmbientPlayTrackAction
    | AmbientSetQueueAction
    | AmbientEnqueueAction
    | AmbientClearQueueAction
    | AmbientSkipNextAction
    | AmbientSkipPrevAction
    | AmbientSeekAction
    | AmbientSetLoopAction
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
    | ActivateSceneAction
    | DeactivateSceneAction,
    Field(discriminator="type"),
]

action_adapter: TypeAdapter[Action] = TypeAdapter(Action)


# --- Server → client: snapshots, state, errors -----------------------------


class DeviceInfo(BaseModel):
    device_id: str
    name: str
    capabilities: list[str]


class PositionReport(BaseModel):
    device_id: str
    position_ms: int
    reported_at: float  # epoch seconds


LoopMode = Literal["off", "queue", "track"]


class AmbientState(BaseModel):
    """The "main player" lane. Holds the current track, what's coming up,
    and what's already played (for skip_prev). `position_ms` is the server's
    best guess of where in the current track playback is — updated by user
    actions (play/seek/skip) and by position reports from the active output.

    While an interrupt is active, `position_ms` is *not* updated (position
    reports go to the interrupt lane), so it preserves the resume point
    automatically.
    """

    current_track_id: int | None = None
    queue: list[int] = Field(default_factory=list)
    history: list[int] = Field(default_factory=list)
    position_ms: int = 0
    loop: LoopMode = "off"


class InterruptState(BaseModel):
    """An interrupt is a short audio override. Its lifecycle ends either by
    queue exhaustion (auto-completion via `interrupt_skip_next` past the end)
    or explicit cancel. On completion, behavior depends on
    `return_to_ambient`: true → ambient takes back over from its preserved
    position; false → playback stops.
    """

    current_track_id: int
    queue: list[int] = Field(default_factory=list)
    position_ms: int = 0
    return_to_ambient: bool = True
    fade_in_ms: int = 0
    fade_out_ms: int = 0


class ScenePreviousState(BaseModel):
    """Snapshot of the fields a scene activation overwrote, captured so
    `deactivate_scene` can restore them. Only populated for fields the
    scene actually changed — a scene with no `ambient` block leaves
    `ambient` here as None and ambient state is left alone on deactivate.
    """

    ambient: AmbientState | None = None
    crossfade_ms: int | None = None
    active_preset_ids: list[str] | None = None


class PlayerState(BaseModel):
    """Canonical playback state. The server is the sole writer."""

    revision: int = 0
    is_playing: bool = False
    volume: float = 1.0

    active_mode_id: str | None = None
    active_output_device_ids: list[str] = Field(default_factory=list)
    active_soundboard_id: str | None = None
    active_preset_ids: list[str] = Field(default_factory=list)

    crossfade_ms: int = 0
    crossfade_type: str = "linear"

    active_scene_id: str | None = None
    pre_scene_state: ScenePreviousState | None = None

    ambient: AmbientState = Field(default_factory=AmbientState)
    interrupt: InterruptState | None = None

    last_position_report: PositionReport | None = None

    connected_devices: list[DeviceInfo] = Field(default_factory=list)


class StateSnapshot(BaseModel):
    type: Literal["state_snapshot"] = "state_snapshot"
    your_device_id: str
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


class SceneActivated(BaseModel):
    """A scene was just activated. Carries the full scene definition so each
    listener (audio engine, lights bridge, MQTT integration) can act on the
    fields it cares about. Broadcast to all clients — recipients ignore
    fields they don't handle."""

    type: Literal["scene_activated"] = "scene_activated"
    scene_id: str
    mode_id: str
    scene: dict  # full scene manifest contents


class SceneDeactivated(BaseModel):
    """A previously-active scene ended. Listeners should stop any side
    effects they started for that scene (looping SFX, persistent lights,
    etc.). PlayerState fields the scene overwrote (ambient / crossfade_ms /
    active_preset_ids) are auto-reverted server-side via `pre_scene_state`
    and reach clients in the accompanying `state_changed`."""

    type: Literal["scene_deactivated"] = "scene_deactivated"
    scene_id: str
    mode_id: str | None


class ErrorMessage(BaseModel):
    type: Literal["error"] = "error"
    detail: str
