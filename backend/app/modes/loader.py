"""Load mode bundles from disk.

A mode bundle:

    modes/
      <mode_id>/
        manifest.yaml          # required
        theme.css              # optional — referenced by manifest.theme
        soundboards/           # optional — one YAML per environment
          tavern.yaml
          dungeon.yaml
          ...
        cues/                  # optional — one YAML per cue
          kraken-fight.yaml
          ...
        presets/               # optional — one YAML per EQ preset
          underwater.yaml
          ...

Modes are validated with Pydantic at load time. Bad files don't crash the
running app; errors are captured per folder so one broken manifest
doesn't take down the rest.
"""
from __future__ import annotations

import logging
import time
from dataclasses import dataclass, field
from pathlib import Path
from threading import Lock

import yaml
from pydantic import BaseModel, Field, ValidationError

from app.core.config import get_settings
from app.presets.loader import PresetManifest

logger = logging.getLogger(__name__)


class InterruptSpec(BaseModel):
    name: str
    playlist: str | None = None
    soundboard_item: str | None = None
    fade_in_ms: int = 0
    fade_out_ms: int = 0
    return_to_ambient: bool = True
    # When set (0.0..1.0), ambient music continues at this volume during the
    # interrupt instead of pausing - produces a cinematic duck. Leave None
    # for the legacy "ambient pauses" behaviour.
    duck_to: float | None = Field(default=None, ge=0.0, le=1.0)


class IntegrationsSpec(BaseModel):
    lights: dict | None = None


class SoundboardItem(BaseModel):
    file: str
    name: str
    icon: str | None = None
    hotkey: str | None = None


class SoundboardCategory(BaseModel):
    id: str
    name: str
    items: list[SoundboardItem] = Field(default_factory=list)


class SoundboardManifest(BaseModel):
    id: str
    name: str | None = None
    categories: list[SoundboardCategory] = Field(default_factory=list)


class CueSfx(BaseModel):
    """A one-shot SFX a cue fires when it runs."""

    soundboard: str
    item: str
    volume: float = Field(default=1.0, ge=0.0, le=1.0)


class CueLoop(BaseModel):
    """A repeating SFX a cue starts (added to the live LOOPS panel)."""

    soundboard: str
    item: str
    interval_s: float = Field(ge=1, le=3600)
    volume: float = Field(default=1.0, ge=0.0, le=1.0)


class CueSpec(BaseModel):
    """A saved one-click setup. Firing it applies a preset, starts a playlist
    (from a given song + timestamp), fires one-shot SFX, and starts loops.
    Everything references existing pieces by name; all parts are optional."""

    id: str
    name: str
    description: str | None = None
    preset: str | None = None  # preset id to apply (replaces active presets)
    playlist: str | None = None  # playlist name to start on the ambient lane
    start_index: int = Field(default=0, ge=0)  # which song in the playlist
    start_ms: int = Field(default=0, ge=0)  # offset within that song
    sfx: list[CueSfx] = Field(default_factory=list)
    loops: list[CueLoop] = Field(default_factory=list)


class ModeManifest(BaseModel):
    id: str
    name: str
    theme: str | None = None
    panels: list[str] = Field(default_factory=list)
    playlist_categories: list[str] = Field(default_factory=list)
    interrupts: list[InterruptSpec] = Field(default_factory=list)
    integrations: IntegrationsSpec = Field(default_factory=IntegrationsSpec)
    default_crossfade_ms: int = 0
    default_soundboard: str | None = None

    # Populated at load time, not part of the YAML contract.
    root_dir: Path | None = None
    soundboards: dict[str, SoundboardManifest] = Field(default_factory=dict)
    cues: dict[str, CueSpec] = Field(default_factory=dict)
    presets: dict[str, PresetManifest] = Field(default_factory=dict)


@dataclass
class LoadResult:
    loaded: dict[str, ModeManifest] = field(default_factory=dict)
    errors: dict[str, str] = field(default_factory=dict)


_modes: dict[str, ModeManifest] = {}
_lock = Lock()

# Cached metadata about the most recent load_all run, surfaced via
# `last_load()` to the diagnostics endpoint. Lets the operator see what
# loaded vs what errored without having to call /api/modes/reload.
_last_load_result: LoadResult | None = None
_last_load_at: float | None = None


def last_load() -> tuple[LoadResult | None, float | None]:
    """Returns (result, unix_timestamp) for the most recent `load_all`."""
    return (_last_load_result, _last_load_at)


def _load_yaml_dir(dir_path: Path) -> list[tuple[str, dict]]:
    """Return (stem, parsed_yaml) for every *.yaml in the dir. Skips empties."""
    if not dir_path.exists():
        return []
    result: list[tuple[str, dict]] = []
    for path in sorted(dir_path.glob("*.yaml")):
        data = yaml.safe_load(path.read_text(encoding="utf-8"))
        if isinstance(data, dict):
            result.append((path.stem, data))
    return result


def _load_soundboards(mode_dir: Path) -> dict[str, SoundboardManifest]:
    out: dict[str, SoundboardManifest] = {}
    for stem, data in _load_yaml_dir(mode_dir / "soundboards"):
        data.setdefault("id", stem)
        manifest = SoundboardManifest.model_validate(data)
        if manifest.id != stem:
            raise ValueError(
                f"soundboard id '{manifest.id}' does not match filename '{stem}'"
            )
        out[manifest.id] = manifest
    return out


def _load_cues(mode_dir: Path) -> dict[str, CueSpec]:
    out: dict[str, CueSpec] = {}
    for stem, data in _load_yaml_dir(mode_dir / "cues"):
        data.setdefault("id", stem)
        cue = CueSpec.model_validate(data)
        if cue.id != stem:
            raise ValueError(f"cue id '{cue.id}' does not match filename '{stem}'")
        out[cue.id] = cue
    return out


def _load_presets(mode_dir: Path) -> dict[str, PresetManifest]:
    out: dict[str, PresetManifest] = {}
    for stem, data in _load_yaml_dir(mode_dir / "presets"):
        data.setdefault("id", stem)
        preset = PresetManifest.model_validate(data)
        if preset.id != stem:
            raise ValueError(
                f"preset id '{preset.id}' does not match filename '{stem}'"
            )
        for effect in preset.effects:
            effect.validate_type()
        out[preset.id] = preset
    return out


def _load_one(mode_dir: Path) -> ModeManifest:
    manifest_path = mode_dir / "manifest.yaml"
    if not manifest_path.exists():
        raise FileNotFoundError(f"missing manifest.yaml in {mode_dir.name}")
    data = yaml.safe_load(manifest_path.read_text(encoding="utf-8"))
    manifest = ModeManifest.model_validate(data)
    if manifest.id != mode_dir.name:
        raise ValueError(
            f"mode id '{manifest.id}' does not match folder name '{mode_dir.name}'"
        )
    manifest.root_dir = mode_dir.resolve()
    manifest.soundboards = _load_soundboards(mode_dir)
    manifest.cues = _load_cues(mode_dir)
    manifest.presets = _load_presets(mode_dir)
    if manifest.default_soundboard and manifest.default_soundboard not in manifest.soundboards:
        raise ValueError(
            f"default_soundboard '{manifest.default_soundboard}' not found in soundboards/"
        )
    return manifest


def load_all() -> LoadResult:
    """Read every mode under MODES_DIR. Replaces the in-memory registry."""
    global _modes
    modes_dir = get_settings().modes_dir.resolve()
    result = LoadResult()
    if not modes_dir.exists():
        logger.warning("modes directory does not exist: %s", modes_dir)
        with _lock:
            _modes = {}
        return result

    for child in sorted(modes_dir.iterdir()):
        if not child.is_dir() or child.name.startswith("."):
            continue
        try:
            manifest = _load_one(child)
        except (FileNotFoundError, ValueError, ValidationError, yaml.YAMLError) as e:
            result.errors[child.name] = str(e)
            logger.exception("failed to load mode %s", child.name)
            continue
        result.loaded[manifest.id] = manifest

    # Swap the whole dict reference — readers (`get_mode`, `all_modes`) run
    # without the lock, and a clear-then-update pair has a window where a
    # concurrent handler sees an empty registry and errors with a spurious
    # "unknown mode".
    with _lock:
        _modes = dict(result.loaded)

    global _last_load_result, _last_load_at
    _last_load_result = result
    _last_load_at = time.time()

    logger.info(
        "loaded %d mode(s): %s%s",
        len(result.loaded),
        ", ".join(result.loaded) or "<none>",
        f" - {len(result.errors)} error(s)" if result.errors else "",
    )
    return result


def all_modes() -> dict[str, ModeManifest]:
    return dict(_modes)


def get_mode(mode_id: str) -> ModeManifest | None:
    return _modes.get(mode_id)
