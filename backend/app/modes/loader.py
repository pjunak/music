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
        scenes/                # optional — one YAML per scene
          tavern.yaml
          ambush.yaml
          ...

Modes are validated with Pydantic at load time. Bad files don't crash the
running app; errors are captured per folder so one broken manifest
doesn't take down the rest.
"""
from __future__ import annotations

import logging
from dataclasses import dataclass, field
from pathlib import Path
from threading import Lock
from typing import Any

import yaml
from pydantic import BaseModel, ConfigDict, Field, ValidationError

from app.core.config import get_settings

logger = logging.getLogger(__name__)


class InterruptSpec(BaseModel):
    name: str
    playlist: str | None = None
    soundboard_item: str | None = None
    fade_in_ms: int = 0
    fade_out_ms: int = 0
    return_to_ambient: bool = True


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


class SceneSpec(BaseModel):
    # Scenes are forward-compatible — unknown fields are preserved so future
    # integrations (fog, lights, etc.) can add their own without schema churn.
    model_config = ConfigDict(extra="allow")

    id: str
    name: str
    description: str | None = None
    ambient: dict[str, Any] | None = None  # {playlist: str, crossfade_ms: int?}
    looping_sfx: list[dict[str, Any]] = Field(default_factory=list)
    lights: dict[str, Any] | None = None
    external: list[dict[str, Any]] = Field(default_factory=list)
    presets: list[str] = Field(default_factory=list)


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
    scenes: dict[str, SceneSpec] = Field(default_factory=dict)


@dataclass
class LoadResult:
    loaded: dict[str, ModeManifest] = field(default_factory=dict)
    errors: dict[str, str] = field(default_factory=dict)


_modes: dict[str, ModeManifest] = {}
_lock = Lock()


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


def _load_scenes(mode_dir: Path) -> dict[str, SceneSpec]:
    out: dict[str, SceneSpec] = {}
    for stem, data in _load_yaml_dir(mode_dir / "scenes"):
        data.setdefault("id", stem)
        scene = SceneSpec.model_validate(data)
        if scene.id != stem:
            raise ValueError(
                f"scene id '{scene.id}' does not match filename '{stem}'"
            )
        out[scene.id] = scene
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
    manifest.scenes = _load_scenes(mode_dir)
    if manifest.default_soundboard and manifest.default_soundboard not in manifest.soundboards:
        raise ValueError(
            f"default_soundboard '{manifest.default_soundboard}' not found in soundboards/"
        )
    return manifest


def load_all() -> LoadResult:
    """Read every mode under MODES_DIR. Replaces the in-memory registry."""
    modes_dir = get_settings().modes_dir.resolve()
    result = LoadResult()
    if not modes_dir.exists():
        logger.warning("modes directory does not exist: %s", modes_dir)
        with _lock:
            _modes.clear()
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

    with _lock:
        _modes.clear()
        _modes.update(result.loaded)

    logger.info(
        "loaded %d mode(s): %s%s",
        len(result.loaded),
        ", ".join(result.loaded) or "<none>",
        f" — {len(result.errors)} error(s)" if result.errors else "",
    )
    return result


def all_modes() -> dict[str, ModeManifest]:
    return dict(_modes)


def get_mode(mode_id: str) -> ModeManifest | None:
    return _modes.get(mode_id)
