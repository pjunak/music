import re
from typing import Any

import yaml
from fastapi import APIRouter, HTTPException, status
from pydantic import BaseModel, Field

from app.api.deps import CurrentUser
from app.core.config import get_settings
from app.presets import loader as presets_loader
from app.presets.loader import PresetManifest
from app.sync import commit_and_broadcast
from app.sync import state as sync_state

router = APIRouter(prefix="/api/presets", tags=["presets"])

_SLUG_RE = re.compile(r"^[a-z0-9][a-z0-9_-]*$")


def _validate_slug(value: str) -> None:
    if not _SLUG_RE.fullmatch(value):
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=(
                "invalid preset id: must be lowercase letters/digits with "
                "optional dashes/underscores, starting with a letter or digit"
            ),
        )


class ActivePresets(BaseModel):
    preset_ids: list[str]


class ReloadResult(BaseModel):
    loaded: list[str]
    errors: dict[str, str]


class EffectSpecIn(BaseModel):
    """Loose effect spec — `type` plus arbitrary numeric/string params.
    Mirrors the YAML shape; the loader validates `type` against the
    supported set on next reload."""

    type: str = Field(min_length=1)

    # Allow any extra keys that match Pydantic's permissive loader.
    model_config = {"extra": "allow"}


class CreatePresetRequest(BaseModel):
    id: str = Field(min_length=1, max_length=64)
    name: str = Field(min_length=1, max_length=128)
    description: str | None = None
    effects: list[EffectSpecIn] = Field(default_factory=list)


class UpdatePresetRequest(BaseModel):
    name: str | None = Field(None, max_length=128)
    description: str | None = None
    effects: list[EffectSpecIn] | None = None


def _filter_loaded(preset_ids: list[str]) -> list[str]:
    """Drop ids that are no longer loaded (e.g. preset deleted on disk).
    Keeps active list honest without requiring explicit cleanup."""
    loaded = presets_loader.all_presets()
    return [p for p in preset_ids if isinstance(p, str) and p in loaded]


@router.get("", response_model=list[PresetManifest])
def list_presets(_: CurrentUser) -> list[PresetManifest]:
    return list(presets_loader.all_presets().values())


@router.get("/active", response_model=ActivePresets)
async def get_active(_: CurrentUser) -> ActivePresets:
    state = await sync_state.machine.snapshot()
    return ActivePresets(preset_ids=_filter_loaded(state.active_preset_ids))


@router.put("/active", response_model=ActivePresets)
async def set_active(payload: ActivePresets, _: CurrentUser) -> ActivePresets:
    loaded = presets_loader.all_presets()
    unknown = [p for p in payload.preset_ids if p not in loaded]
    if unknown:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=f"unknown preset(s): {unknown}",
        )
    await commit_and_broadcast(sync_state.set_active_presets(payload.preset_ids))
    state = await sync_state.machine.snapshot()
    return ActivePresets(preset_ids=state.active_preset_ids)


@router.post("/reload", response_model=ReloadResult)
async def reload(_: CurrentUser) -> ReloadResult:
    result = presets_loader.load_all()
    # Prune active list to only reference presets still loaded.
    state = await sync_state.machine.snapshot()
    pruned = _filter_loaded(state.active_preset_ids)
    if pruned != list(state.active_preset_ids):
        await commit_and_broadcast(sync_state.set_active_presets(pruned))
    return ReloadResult(loaded=list(result.loaded.keys()), errors=result.errors)


@router.get("/{preset_id}", response_model=PresetManifest)
def get_one(preset_id: str, _: CurrentUser) -> PresetManifest:
    manifest = presets_loader.get_preset(preset_id)
    if manifest is None:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="preset not loaded"
        )
    return manifest


# --- scaffolding (write YAML to disk, then reload) ----------------------


def _presets_root() -> "Any":
    return get_settings().presets_dir.resolve()


def _preset_path(preset_id: str) -> "Any":
    return _presets_root() / f"{preset_id}.yaml"


def _write_preset_yaml(preset_id: str, payload: dict) -> None:
    root = _presets_root()
    root.mkdir(parents=True, exist_ok=True)
    path = _preset_path(preset_id)
    path.write_text(yaml.safe_dump(payload, sort_keys=False), encoding="utf-8")


@router.post("", response_model=PresetManifest, status_code=status.HTTP_201_CREATED)
def create_preset(payload: CreatePresetRequest, _: CurrentUser) -> PresetManifest:
    _validate_slug(payload.id)
    target = _preset_path(payload.id)
    if target.exists():
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=f"preset '{payload.id}' already exists",
        )
    yaml_payload: dict[str, Any] = {
        "id": payload.id,
        "name": payload.name,
    }
    if payload.description is not None:
        yaml_payload["description"] = payload.description
    yaml_payload["effects"] = [e.model_dump() for e in payload.effects]
    _write_preset_yaml(payload.id, yaml_payload)

    result = presets_loader.load_all()
    if payload.id in result.errors:
        # Loader rejected our scaffold (probably an unknown effect type).
        # Roll back the file so the operator can retry.
        target.unlink(missing_ok=True)
        presets_loader.load_all()
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=result.errors[payload.id],
        )
    manifest = presets_loader.get_preset(payload.id)
    if manifest is None:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="preset scaffolded but failed to reload",
        )
    return manifest


@router.put("/{preset_id}", response_model=PresetManifest)
def update_preset(
    preset_id: str, payload: UpdatePresetRequest, _: CurrentUser
) -> PresetManifest:
    _validate_slug(preset_id)
    existing = presets_loader.get_preset(preset_id)
    if existing is None:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="preset not found"
        )
    yaml_payload: dict[str, Any] = {
        "id": preset_id,
        "name": payload.name if payload.name is not None else existing.name,
    }
    desc = payload.description if payload.description is not None else existing.description
    if desc is not None:
        yaml_payload["description"] = desc
    if payload.effects is not None:
        yaml_payload["effects"] = [e.model_dump() for e in payload.effects]
    else:
        yaml_payload["effects"] = [e.model_dump() for e in existing.effects]
    _write_preset_yaml(preset_id, yaml_payload)

    result = presets_loader.load_all()
    if preset_id in result.errors:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST, detail=result.errors[preset_id]
        )
    manifest = presets_loader.get_preset(preset_id)
    assert manifest is not None
    return manifest


@router.delete("/{preset_id}", status_code=status.HTTP_204_NO_CONTENT)
async def delete_preset(preset_id: str, _: CurrentUser) -> None:
    _validate_slug(preset_id)
    target = _preset_path(preset_id)
    if not target.is_file():
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="preset not found"
        )
    target.unlink()

    presets_loader.load_all()
    state = await sync_state.machine.snapshot()
    pruned = _filter_loaded(state.active_preset_ids)
    if pruned != list(state.active_preset_ids):
        await commit_and_broadcast(sync_state.set_active_presets(pruned))
