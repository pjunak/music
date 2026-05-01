"""Load audio effect preset manifests from disk.

Presets are global (shared across all modes) — one YAML per preset under
PRESETS_DIR. Each preset declares an ordered effect chain that the frontend
applies in its Web Audio graph. The backend only tracks which presets are
loaded and which are currently active; the actual audio processing is
entirely client-side.

Effect `type` is an enum of values the frontend knows how to realize. An
unknown type on load is an error so typos don't ship quietly.
"""
from __future__ import annotations

import logging
import time
from dataclasses import dataclass, field
from pathlib import Path
from threading import Lock
from typing import Any

import yaml
from pydantic import BaseModel, ConfigDict, Field, ValidationError

from app.core.config import get_settings

logger = logging.getLogger(__name__)

# Effect types the frontend is expected to implement. Additions here must be
# matched by frontend support; otherwise the preset loads but does nothing.
SUPPORTED_EFFECT_TYPES: frozenset[str] = frozenset(
    {
        "reverb",
        "lowpass",
        "highpass",
        "bandpass",
        "delay",
        "distortion",
        "tremolo",
        "pitch_shift",
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

    def to_dict(self) -> dict[str, Any]:
        return self.model_dump()


class PresetManifest(BaseModel):
    id: str
    name: str
    description: str | None = None
    effects: list[EffectSpec] = Field(default_factory=list)


@dataclass
class LoadResult:
    loaded: dict[str, PresetManifest] = field(default_factory=dict)
    errors: dict[str, str] = field(default_factory=dict)


_presets: dict[str, PresetManifest] = {}
_lock = Lock()

_last_load_result: LoadResult | None = None
_last_load_at: float | None = None


def last_load() -> tuple[LoadResult | None, float | None]:
    """Returns (result, unix_timestamp) for the most recent `load_all`."""
    return (_last_load_result, _last_load_at)


def _load_one(path: Path) -> PresetManifest:
    data = yaml.safe_load(path.read_text(encoding="utf-8")) or {}
    data.setdefault("id", path.stem)
    manifest = PresetManifest.model_validate(data)
    if manifest.id != path.stem:
        raise ValueError(
            f"preset id '{manifest.id}' does not match filename '{path.stem}'"
        )
    for effect in manifest.effects:
        effect.validate_type()
    return manifest


def load_all() -> LoadResult:
    presets_dir = get_settings().presets_dir.resolve()
    result = LoadResult()
    if not presets_dir.exists():
        logger.warning("presets directory does not exist: %s", presets_dir)
        with _lock:
            _presets.clear()
        return result

    for path in sorted(presets_dir.glob("*.yaml")):
        try:
            manifest = _load_one(path)
        except (ValueError, ValidationError, yaml.YAMLError) as e:
            result.errors[path.stem] = str(e)
            logger.exception("failed to load preset %s", path.stem)
            continue
        result.loaded[manifest.id] = manifest

    with _lock:
        _presets.clear()
        _presets.update(result.loaded)

    global _last_load_result, _last_load_at
    _last_load_result = result
    _last_load_at = time.time()

    logger.info(
        "loaded %d preset(s): %s%s",
        len(result.loaded),
        ", ".join(result.loaded) or "<none>",
        f" - {len(result.errors)} error(s)" if result.errors else "",
    )
    return result


def all_presets() -> dict[str, PresetManifest]:
    return dict(_presets)


def get_preset(preset_id: str) -> PresetManifest | None:
    return _presets.get(preset_id)
