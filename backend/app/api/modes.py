import re
import shutil
from pathlib import Path

import yaml
from fastapi import APIRouter, HTTPException, status
from fastapi.responses import FileResponse
from pydantic import BaseModel, Field

from app.api.deps import CurrentUser
from app.core.config import get_settings
from app.modes import loader as modes_loader
from app.modes.loader import (
    IntegrationsSpec,
    InterruptSpec,
    ModeManifest,
    SceneSpec,
    SoundboardManifest,
)
from app.sync import commit_and_broadcast
from app.sync import state as sync_state

router = APIRouter(prefix="/api/modes", tags=["modes"])

# Slugs are filesystem dirnames / filenames; constrain conservatively.
_SLUG_RE = re.compile(r"^[a-z0-9][a-z0-9_-]*$")


def _validate_slug(value: str, kind: str) -> None:
    if not _SLUG_RE.fullmatch(value):
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=(
                f"invalid {kind} id: must be lowercase letters/digits with "
                "optional dashes/underscores, starting with a letter or digit"
            ),
        )


def _modes_root() -> Path:
    return get_settings().modes_dir.resolve()


def _mode_dir_or_404(mode_id: str) -> Path:
    mode_dir = _modes_root() / mode_id
    if not mode_dir.is_dir():
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail=f"mode '{mode_id}' not found"
        )
    return mode_dir.resolve()


def _write_yaml(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(yaml.safe_dump(payload, sort_keys=False), encoding="utf-8")


class ModeSummary(BaseModel):
    id: str
    name: str
    panels: list[str]
    playlist_categories: list[str]
    has_theme: bool
    default_crossfade_ms: int
    default_soundboard: str | None


class ModeDetail(ModeSummary):
    interrupts: list[InterruptSpec]
    integrations: IntegrationsSpec
    soundboards: dict[str, SoundboardManifest]
    scenes: dict[str, SceneSpec]


class ActiveMode(BaseModel):
    mode_id: str | None


class SetActiveModeRequest(BaseModel):
    mode_id: str | None


class ReloadResult(BaseModel):
    loaded: list[str]
    errors: dict[str, str]


class CreateModeRequest(BaseModel):
    id: str = Field(min_length=1, max_length=64)
    name: str = Field(min_length=1, max_length=128)


class CreateSoundboardRequest(BaseModel):
    id: str = Field(min_length=1, max_length=64)
    name: str | None = Field(None, max_length=128)


class CreateSceneRequest(BaseModel):
    id: str = Field(min_length=1, max_length=64)
    name: str = Field(min_length=1, max_length=128)
    description: str | None = None


def _theme_path(manifest: ModeManifest) -> Path | None:
    """Resolve the theme file path safely: must be inside the mode's
    root_dir to prevent traversal via a malicious manifest."""
    if not manifest.theme or manifest.root_dir is None:
        return None
    candidate = (manifest.root_dir / manifest.theme).resolve()
    try:
        candidate.relative_to(manifest.root_dir)
    except ValueError:
        return None
    return candidate if candidate.is_file() else None


def _summary(manifest: ModeManifest) -> ModeSummary:
    return ModeSummary(
        id=manifest.id,
        name=manifest.name,
        panels=manifest.panels,
        playlist_categories=manifest.playlist_categories,
        has_theme=_theme_path(manifest) is not None,
        default_crossfade_ms=manifest.default_crossfade_ms,
        default_soundboard=manifest.default_soundboard,
    )


@router.get("", response_model=list[ModeSummary])
def list_modes(_: CurrentUser) -> list[ModeSummary]:
    return [_summary(m) for m in modes_loader.all_modes().values()]


@router.get("/active", response_model=ActiveMode)
async def get_active_mode(_: CurrentUser) -> ActiveMode:
    state = await sync_state.machine.snapshot()
    return ActiveMode(mode_id=state.active_mode_id)


@router.put("/active", response_model=ActiveMode)
async def set_active_mode(payload: SetActiveModeRequest, _: CurrentUser) -> ActiveMode:
    if payload.mode_id is not None and modes_loader.get_mode(payload.mode_id) is None:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=f"unknown mode: {payload.mode_id}",
        )
    await commit_and_broadcast(sync_state.set_active_mode(payload.mode_id))
    return ActiveMode(mode_id=payload.mode_id)


@router.post("/reload", response_model=ReloadResult)
def reload_modes(_: CurrentUser) -> ReloadResult:
    result = modes_loader.load_all()
    return ReloadResult(loaded=list(result.loaded.keys()), errors=result.errors)


@router.get("/{mode_id}", response_model=ModeDetail)
def get_mode(mode_id: str, _: CurrentUser) -> ModeDetail:
    manifest = modes_loader.get_mode(mode_id)
    if manifest is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="mode not loaded")
    summary = _summary(manifest)
    return ModeDetail(
        **summary.model_dump(),
        interrupts=manifest.interrupts,
        integrations=manifest.integrations,
        soundboards=manifest.soundboards,
        scenes=manifest.scenes,
    )


@router.get("/{mode_id}/theme.css")
def get_mode_theme(mode_id: str, _: CurrentUser) -> FileResponse:
    manifest = modes_loader.get_mode(mode_id)
    if manifest is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="mode not loaded")
    path = _theme_path(manifest)
    if path is None:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="theme not declared or missing"
        )
    return FileResponse(path, media_type="text/css", content_disposition_type="inline")


# --- scaffolding (write YAML to disk, then reload) ----------------------


@router.post("", response_model=ModeSummary, status_code=status.HTTP_201_CREATED)
def create_mode(payload: CreateModeRequest, _: CurrentUser) -> ModeSummary:
    """Scaffold a new mode directory with a minimal manifest.yaml. Power
    users can keep editing the YAML on disk; this endpoint is just so the
    UI can bootstrap a new mode without leaving the app."""
    _validate_slug(payload.id, "mode")
    mode_dir = _modes_root() / payload.id
    if mode_dir.exists():
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=f"mode '{payload.id}' already exists",
        )
    manifest = {
        "id": payload.id,
        "name": payload.name,
        "panels": [],
        "playlist_categories": [],
        "interrupts": [],
        "integrations": {"lights": None},
        "default_crossfade_ms": 0,
    }
    mode_dir.mkdir(parents=True, exist_ok=False)
    _write_yaml(mode_dir / "manifest.yaml", manifest)
    (mode_dir / "soundboards").mkdir(exist_ok=True)
    (mode_dir / "scenes").mkdir(exist_ok=True)

    modes_loader.load_all()
    return _summary(modes_loader.get_mode(payload.id))  # type: ignore[arg-type]


@router.delete("/{mode_id}", status_code=status.HTTP_204_NO_CONTENT)
def delete_mode(mode_id: str, _: CurrentUser) -> None:
    mode_dir = _mode_dir_or_404(mode_id)
    shutil.rmtree(mode_dir)
    modes_loader.load_all()


@router.post(
    "/{mode_id}/soundboards",
    response_model=SoundboardManifest,
    status_code=status.HTTP_201_CREATED,
)
def create_soundboard(
    mode_id: str, payload: CreateSoundboardRequest, _: CurrentUser
) -> SoundboardManifest:
    _validate_slug(payload.id, "soundboard")
    mode_dir = _mode_dir_or_404(mode_id)
    sb_path = mode_dir / "soundboards" / f"{payload.id}.yaml"
    if sb_path.exists():
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=f"soundboard '{payload.id}' already exists in mode '{mode_id}'",
        )
    yaml_payload: dict = {"id": payload.id}
    if payload.name is not None:
        yaml_payload["name"] = payload.name
    yaml_payload["categories"] = []
    _write_yaml(sb_path, yaml_payload)

    modes_loader.load_all()
    manifest = modes_loader.get_mode(mode_id)
    if manifest is None or payload.id not in manifest.soundboards:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="soundboard scaffolded but failed to reload",
        )
    return manifest.soundboards[payload.id]


@router.delete(
    "/{mode_id}/soundboards/{soundboard_id}", status_code=status.HTTP_204_NO_CONTENT
)
def delete_soundboard(mode_id: str, soundboard_id: str, _: CurrentUser) -> None:
    _validate_slug(soundboard_id, "soundboard")
    mode_dir = _mode_dir_or_404(mode_id)
    sb_path = mode_dir / "soundboards" / f"{soundboard_id}.yaml"
    if not sb_path.is_file():
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail=f"soundboard '{soundboard_id}' not found",
        )
    sb_path.unlink()
    modes_loader.load_all()


@router.post(
    "/{mode_id}/scenes",
    response_model=SceneSpec,
    status_code=status.HTTP_201_CREATED,
)
def create_scene(
    mode_id: str, payload: CreateSceneRequest, _: CurrentUser
) -> SceneSpec:
    _validate_slug(payload.id, "scene")
    mode_dir = _mode_dir_or_404(mode_id)
    scene_path = mode_dir / "scenes" / f"{payload.id}.yaml"
    if scene_path.exists():
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=f"scene '{payload.id}' already exists in mode '{mode_id}'",
        )
    yaml_payload: dict = {"id": payload.id, "name": payload.name}
    if payload.description is not None:
        yaml_payload["description"] = payload.description
    _write_yaml(scene_path, yaml_payload)

    modes_loader.load_all()
    manifest = modes_loader.get_mode(mode_id)
    if manifest is None or payload.id not in manifest.scenes:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="scene scaffolded but failed to reload",
        )
    return manifest.scenes[payload.id]


@router.delete(
    "/{mode_id}/scenes/{scene_id}", status_code=status.HTTP_204_NO_CONTENT
)
def delete_scene(mode_id: str, scene_id: str, _: CurrentUser) -> None:
    _validate_slug(scene_id, "scene")
    mode_dir = _mode_dir_or_404(mode_id)
    scene_path = mode_dir / "scenes" / f"{scene_id}.yaml"
    if not scene_path.is_file():
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail=f"scene '{scene_id}' not found",
        )
    scene_path.unlink()
    modes_loader.load_all()


