"""Preset models + helpers.

Presets are EQ/effect chains. As of the mode-scoped refactor they live
**per-mode** (`modes/<mode>/presets/<id>.yaml`) and are loaded by the modes
loader — so this module no longer keeps a global registry. It owns the shared
models, effect-type validation, and the override resolver; the modes loader and
modes API drive the actual per-mode loading + CRUD.

Each preset declares an ordered effect chain the frontend applies in its Web
Audio graph (the backend only tracks which are active); plus optional
"when active" master-volume / crossfade overrides.
"""
from __future__ import annotations

from pydantic import BaseModel, ConfigDict, Field

# Effect types the frontend is expected to implement. Additions here must be
# matched by frontend support; otherwise the preset loads but does nothing
# (the editor flags any type not in its EFFECT_UI map).
SUPPORTED_EFFECT_TYPES: frozenset[str] = frozenset(
    {
        "eq",
        "reverb",
        "lowpass",
        "highpass",
        "bandpass",
        "delay",
        "distortion",
        "tremolo",
        # NOTE: no "pitch_shift" — the Web Audio engine has no pitch shifter
        # (buildEffect returns null for it), so accepting it here would let a
        # preset save cleanly yet do nothing at playback. Re-add only once the
        # engine + editor support it.
    }
)


class EffectSpec(BaseModel):
    model_config = ConfigDict(extra="allow")
    type: str

    def validate_type(self) -> None:
        if self.type not in SUPPORTED_EFFECT_TYPES:
            raise ValueError(
                f"unknown effect type '{self.type}' (supported: {sorted(SUPPORTED_EFFECT_TYPES)})"
            )


class PresetManifest(BaseModel):
    id: str
    name: str
    description: str | None = None
    effects: list[EffectSpec] = Field(default_factory=list)
    # Optional "when active" overrides applied on activation. None = leave that
    # global alone. When several active presets set one, last-active wins.
    volume: float | None = Field(default=None, ge=0.0, le=1.0)
    crossfade_ms: int | None = Field(default=None, ge=0, le=60000)


def effective_overrides(
    presets: dict[str, PresetManifest], preset_ids: list[str]
) -> tuple[float | None, int | None]:
    """Resolve the master-volume / crossfade overrides for a set of active
    presets, looked up in `presets` (the active mode's preset map). Among the
    active presets in order, the last one that defines a value wins. Returns
    (volume, crossfade_ms), each None when no active preset sets it."""
    volume: float | None = None
    crossfade_ms: int | None = None
    for pid in preset_ids:
        p = presets.get(pid)
        if p is None:
            continue
        if p.volume is not None:
            volume = p.volume
        if p.crossfade_ms is not None:
            crossfade_ms = p.crossfade_ms
    return volume, crossfade_ms
