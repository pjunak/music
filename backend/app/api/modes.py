from pathlib import Path

from fastapi import APIRouter, HTTPException, status
from fastapi.responses import FileResponse
from pydantic import BaseModel

from app.api.deps import CurrentUser
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


def _is_referenced_sfx(manifest: ModeManifest, filename: str) -> bool:
    for soundboard in manifest.soundboards.values():
        for category in soundboard.categories:
            for item in category.items:
                if item.file == filename:
                    return True
    return False


@router.get("/{mode_id}/soundboards/files/{filename}")
def get_sfx_file(mode_id: str, filename: str, _: CurrentUser) -> FileResponse:
    """Stream an SFX audio file. Only files referenced by some soundboard
    in the mode are served — prevents arbitrary file access via a crafted
    filename. Path is also re-validated against the mode's root_dir to
    defend against a manifest with traversal paths."""
    manifest = modes_loader.get_mode(mode_id)
    if manifest is None or manifest.root_dir is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="mode not loaded")
    if not _is_referenced_sfx(manifest, filename):
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="sfx file not referenced by any soundboard",
        )
    candidate = (manifest.root_dir / "soundboards" / "files" / filename).resolve()
    try:
        candidate.relative_to(manifest.root_dir)
    except ValueError as e:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="invalid path"
        ) from e
    if not candidate.is_file():
        raise HTTPException(
            status_code=status.HTTP_410_GONE, detail="sfx file missing on disk"
        )
    return FileResponse(candidate, content_disposition_type="inline")
