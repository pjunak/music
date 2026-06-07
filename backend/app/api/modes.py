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
    CueSpec,
    IntegrationsSpec,
    InterruptSpec,
    ModeManifest,
    SoundboardManifest,
)
from app.presets.loader import SUPPORTED_EFFECT_TYPES, PresetManifest
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
    cues: dict[str, CueSpec]
    presets: dict[str, PresetManifest]


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


class AddCategoryRequest(BaseModel):
    id: str = Field(min_length=1, max_length=64)
    name: str = Field(min_length=1, max_length=128)


class AddItemRequest(BaseModel):
    file: str = Field(min_length=1, description="Path relative to SFX_LIBRARY_DIR")
    name: str = Field(min_length=1, max_length=128)
    hotkey: str | None = Field(None, max_length=8)
    icon: str | None = Field(None, max_length=8)


class UpdateItemRequest(BaseModel):
    name: str | None = Field(None, max_length=128)
    hotkey: str | None = Field(None, max_length=8)
    icon: str | None = Field(None, max_length=8)
    file: str | None = None


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
        cues=manifest.cues,
        presets=manifest.presets,
    )


@router.get("/{mode_id}/presets", response_model=list[PresetManifest])
def list_mode_presets(mode_id: str, _: CurrentUser) -> list[PresetManifest]:
    manifest = modes_loader.get_mode(mode_id)
    if manifest is None:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="mode not loaded"
        )
    return list(manifest.presets.values())


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
    (mode_dir / "cues").mkdir(exist_ok=True)
    (mode_dir / "presets").mkdir(exist_ok=True)

    modes_loader.load_all()
    return _summary(modes_loader.get_mode(payload.id))  # type: ignore[arg-type]


@router.delete("/{mode_id}", status_code=status.HTTP_204_NO_CONTENT)
def delete_mode(mode_id: str, _: CurrentUser) -> None:
    mode_dir = _mode_dir_or_404(mode_id)
    try:
        shutil.rmtree(mode_dir)
    except PermissionError as e:
        # The most common cause is MODES_DIR pointing at an image-baked path
        # owned by root while uvicorn runs as the `music` user. Surface the
        # full mode dir so the operator knows which permissions to fix
        # rather than seeing a bare "manifest.yaml" error.
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=(
                f"permission denied removing {mode_dir}: {e}. "
                "Ensure MODES_DIR is on a bind-mounted volume writable by "
                "the runtime user (uid 1000)."
            ),
        ) from e
    modes_loader.load_all()


class RenameModeRequest(BaseModel):
    name: str = Field(min_length=1, max_length=128)


@router.patch("/{mode_id}", response_model=ModeSummary)
def rename_mode(
    mode_id: str, payload: RenameModeRequest, _: CurrentUser
) -> ModeSummary:
    """Rename a mode's display name (the on-disk slug/id is unchanged, so no
    references need rewiring)."""
    path, manifest = _load_manifest_yaml(mode_id)
    manifest["name"] = payload.name
    reloaded = _save_manifest_yaml(path, manifest, mode_id)
    return _summary(reloaded)


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


# --- soundboard editing (categories + items inside an existing soundboard) ---


def _load_soundboard_yaml(mode_id: str, soundboard_id: str) -> tuple[Path, dict]:
    """Read the soundboard YAML for editing. Returns (path, parsed dict).
    The dict shape mirrors mutagen-loose: `id`, `name?`, `categories: [{id, name, items: [{file, name, hotkey?, icon?}]}]`."""
    mode_dir = _mode_dir_or_404(mode_id)
    sb_path = mode_dir / "soundboards" / f"{soundboard_id}.yaml"
    if not sb_path.is_file():
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail=f"soundboard '{soundboard_id}' not found in mode '{mode_id}'",
        )
    raw = yaml.safe_load(sb_path.read_text(encoding="utf-8")) or {}
    raw.setdefault("id", soundboard_id)
    raw.setdefault("categories", [])
    return (sb_path, raw)


def _save_soundboard_yaml(path: Path, payload: dict, mode_id: str) -> SoundboardManifest:
    _write_yaml(path, payload)
    modes_loader.load_all()
    manifest = modes_loader.get_mode(mode_id)
    if manifest is None or payload["id"] not in manifest.soundboards:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="soundboard saved but failed to reload",
        )
    return manifest.soundboards[payload["id"]]


def _find_category(sb: dict, category_id: str) -> dict:
    for cat in sb.get("categories", []):
        if cat.get("id") == category_id:
            return cat
    raise HTTPException(
        status_code=status.HTTP_404_NOT_FOUND,
        detail=f"category '{category_id}' not found",
    )


@router.post(
    "/{mode_id}/soundboards/{soundboard_id}/categories",
    response_model=SoundboardManifest,
    status_code=status.HTTP_201_CREATED,
)
def add_soundboard_category(
    mode_id: str,
    soundboard_id: str,
    payload: AddCategoryRequest,
    _: CurrentUser,
) -> SoundboardManifest:
    _validate_slug(payload.id, "category")
    path, sb = _load_soundboard_yaml(mode_id, soundboard_id)
    if any(c.get("id") == payload.id for c in sb.get("categories", [])):
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=f"category '{payload.id}' already exists",
        )
    sb.setdefault("categories", []).append(
        {"id": payload.id, "name": payload.name, "items": []}
    )
    return _save_soundboard_yaml(path, sb, mode_id)


@router.delete(
    "/{mode_id}/soundboards/{soundboard_id}/categories/{category_id}",
    response_model=SoundboardManifest,
)
def delete_soundboard_category(
    mode_id: str, soundboard_id: str, category_id: str, _: CurrentUser
) -> SoundboardManifest:
    path, sb = _load_soundboard_yaml(mode_id, soundboard_id)
    _find_category(sb, category_id)  # 404 if missing
    sb["categories"] = [c for c in sb["categories"] if c.get("id") != category_id]
    return _save_soundboard_yaml(path, sb, mode_id)


@router.post(
    "/{mode_id}/soundboards/{soundboard_id}/categories/{category_id}/items",
    response_model=SoundboardManifest,
    status_code=status.HTTP_201_CREATED,
)
def add_soundboard_item(
    mode_id: str,
    soundboard_id: str,
    category_id: str,
    payload: AddItemRequest,
    _: CurrentUser,
) -> SoundboardManifest:
    path, sb = _load_soundboard_yaml(mode_id, soundboard_id)
    cat = _find_category(sb, category_id)
    items = cat.setdefault("items", [])
    item: dict = {"file": payload.file, "name": payload.name}
    if payload.hotkey:
        item["hotkey"] = payload.hotkey
    if payload.icon:
        item["icon"] = payload.icon
    items.append(item)
    return _save_soundboard_yaml(path, sb, mode_id)


@router.patch(
    "/{mode_id}/soundboards/{soundboard_id}/categories/{category_id}/items/{index}",
    response_model=SoundboardManifest,
)
def update_soundboard_item(
    mode_id: str,
    soundboard_id: str,
    category_id: str,
    index: int,
    payload: UpdateItemRequest,
    _: CurrentUser,
) -> SoundboardManifest:
    path, sb = _load_soundboard_yaml(mode_id, soundboard_id)
    cat = _find_category(sb, category_id)
    items = cat.get("items", [])
    if index < 0 or index >= len(items):
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="item index out of range"
        )
    item = items[index]
    fields = payload.model_dump(exclude_unset=True)
    for key, value in fields.items():
        if value is None or value == "":
            item.pop(key, None)
        else:
            item[key] = value
    return _save_soundboard_yaml(path, sb, mode_id)


@router.delete(
    "/{mode_id}/soundboards/{soundboard_id}/categories/{category_id}/items/{index}",
    response_model=SoundboardManifest,
)
def delete_soundboard_item(
    mode_id: str,
    soundboard_id: str,
    category_id: str,
    index: int,
    _: CurrentUser,
) -> SoundboardManifest:
    path, sb = _load_soundboard_yaml(mode_id, soundboard_id)
    cat = _find_category(sb, category_id)
    items = cat.get("items", [])
    if index < 0 or index >= len(items):
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="item index out of range"
        )
    items.pop(index)
    return _save_soundboard_yaml(path, sb, mode_id)


# --- interrupt templates (CRUD on the mode's manifest.interrupts list) ----


class InterruptTemplateCreate(BaseModel):
    name: str = Field(min_length=1, max_length=128)
    playlist: str | None = Field(None, max_length=256)
    soundboard_item: str | None = Field(None, max_length=512)
    fade_in_ms: int = Field(0, ge=0, le=10000)
    fade_out_ms: int = Field(0, ge=0, le=10000)
    return_to_ambient: bool = True
    duck_to: float | None = Field(None, ge=0.0, le=1.0)


class InterruptTemplateUpdate(BaseModel):
    name: str | None = Field(None, min_length=1, max_length=128)
    playlist: str | None = Field(None, max_length=256)
    soundboard_item: str | None = Field(None, max_length=512)
    fade_in_ms: int | None = Field(None, ge=0, le=10000)
    fade_out_ms: int | None = Field(None, ge=0, le=10000)
    return_to_ambient: bool | None = None
    duck_to: float | None = Field(None, ge=0.0, le=1.0)


def _load_manifest_yaml(mode_id: str) -> tuple[Path, dict]:
    """Read a mode's manifest.yaml for editing, returning the path and a
    parsed dict (interrupts list seeded if missing)."""
    mode_dir = _mode_dir_or_404(mode_id)
    manifest_path = mode_dir / "manifest.yaml"
    if not manifest_path.is_file():
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail=f"manifest.yaml missing for mode '{mode_id}'",
        )
    raw = yaml.safe_load(manifest_path.read_text(encoding="utf-8")) or {}
    if not isinstance(raw, dict):
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"manifest.yaml for mode '{mode_id}' is not a mapping",
        )
    raw.setdefault("interrupts", [])
    return (manifest_path, raw)


def _save_manifest_yaml(path: Path, payload: dict, mode_id: str) -> ModeManifest:
    _write_yaml(path, payload)
    modes_loader.load_all()
    manifest = modes_loader.get_mode(mode_id)
    if manifest is None:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="manifest saved but failed to reload",
        )
    return manifest


def _validate_interrupt_payload(
    playlist: str | None, soundboard_item: str | None
) -> None:
    """Exactly one of playlist / soundboard_item must be set. Both empty
    is meaningless; both set is ambiguous."""
    has_playlist = bool(playlist)
    has_item = bool(soundboard_item)
    if has_playlist == has_item:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=(
                "interrupt template must reference exactly one of "
                "'playlist' or 'soundboard_item'"
            ),
        )


@router.post(
    "/{mode_id}/interrupts",
    response_model=list[InterruptSpec],
    status_code=status.HTTP_201_CREATED,
)
def add_interrupt_template(
    mode_id: str, payload: InterruptTemplateCreate, _: CurrentUser
) -> list[InterruptSpec]:
    _validate_interrupt_payload(payload.playlist, payload.soundboard_item)
    path, manifest = _load_manifest_yaml(mode_id)
    interrupts = manifest.setdefault("interrupts", [])
    new_entry: dict = {"name": payload.name}
    if payload.playlist:
        new_entry["playlist"] = payload.playlist
    if payload.soundboard_item:
        new_entry["soundboard_item"] = payload.soundboard_item
    if payload.fade_in_ms:
        new_entry["fade_in_ms"] = payload.fade_in_ms
    if payload.fade_out_ms:
        new_entry["fade_out_ms"] = payload.fade_out_ms
    if not payload.return_to_ambient:
        new_entry["return_to_ambient"] = False
    if payload.duck_to is not None:
        new_entry["duck_to"] = payload.duck_to
    interrupts.append(new_entry)
    reloaded = _save_manifest_yaml(path, manifest, mode_id)
    return reloaded.interrupts


@router.patch(
    "/{mode_id}/interrupts/{index}",
    response_model=list[InterruptSpec],
)
def update_interrupt_template(
    mode_id: str,
    index: int,
    payload: InterruptTemplateUpdate,
    _: CurrentUser,
) -> list[InterruptSpec]:
    path, manifest = _load_manifest_yaml(mode_id)
    interrupts = manifest.get("interrupts", [])
    if index < 0 or index >= len(interrupts):
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="interrupt index out of range",
        )
    entry = dict(interrupts[index])
    fields = payload.model_dump(exclude_unset=True)
    for key, value in fields.items():
        if value is None or value == "":
            entry.pop(key, None)
        else:
            entry[key] = value
    # Re-validate the playlist / soundboard_item invariant against the merged
    # entry — a partial update that nulls one source must leave the other set.
    _validate_interrupt_payload(
        entry.get("playlist") if isinstance(entry.get("playlist"), str) else None,
        entry.get("soundboard_item")
        if isinstance(entry.get("soundboard_item"), str)
        else None,
    )
    interrupts[index] = entry
    reloaded = _save_manifest_yaml(path, manifest, mode_id)
    return reloaded.interrupts


@router.delete(
    "/{mode_id}/interrupts/{index}",
    response_model=list[InterruptSpec],
)
def delete_interrupt_template(
    mode_id: str, index: int, _: CurrentUser
) -> list[InterruptSpec]:
    path, manifest = _load_manifest_yaml(mode_id)
    interrupts = manifest.get("interrupts", [])
    if index < 0 or index >= len(interrupts):
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="interrupt index out of range",
        )
    interrupts.pop(index)
    reloaded = _save_manifest_yaml(path, manifest, mode_id)
    return reloaded.interrupts


# --- cues (one YAML per cue under the mode's cues/ dir) -------------------


class CueSfxIn(BaseModel):
    soundboard: str = Field(min_length=1, max_length=128)
    item: str = Field(min_length=1, max_length=512)
    volume: float = Field(default=1.0, ge=0.0, le=1.0)


class CueLoopIn(BaseModel):
    soundboard: str = Field(min_length=1, max_length=128)
    item: str = Field(min_length=1, max_length=512)
    interval_s: float = Field(ge=1, le=3600)
    volume: float = Field(default=1.0, ge=0.0, le=1.0)


class CueBody(BaseModel):
    """The full editable cue. The editor always submits the whole thing, so
    create and update both replace the YAML wholesale."""

    name: str = Field(min_length=1, max_length=128)
    description: str | None = None
    preset: str | None = Field(None, max_length=64)
    playlist: str | None = Field(None, max_length=256)
    start_index: int = Field(default=0, ge=0)
    start_ms: int = Field(default=0, ge=0)
    sfx: list[CueSfxIn] = Field(default_factory=list)
    loops: list[CueLoopIn] = Field(default_factory=list)


class CreateCueRequest(CueBody):
    id: str = Field(min_length=1, max_length=64)


def _cue_yaml(cue_id: str, body: CueBody) -> dict:
    """Build a tidy cue YAML mapping — omit empty/default fields."""
    out: dict = {"id": cue_id, "name": body.name}
    if body.description:
        out["description"] = body.description
    if body.preset:
        out["preset"] = body.preset
    if body.playlist:
        out["playlist"] = body.playlist
    if body.start_index:
        out["start_index"] = body.start_index
    if body.start_ms:
        out["start_ms"] = body.start_ms
    if body.sfx:
        out["sfx"] = [s.model_dump() for s in body.sfx]
    if body.loops:
        out["loops"] = [loop.model_dump() for loop in body.loops]
    return out


def _save_cue_yaml(path: Path, payload: dict, mode_id: str, cue_id: str) -> CueSpec:
    _write_yaml(path, payload)
    modes_loader.load_all()
    manifest = modes_loader.get_mode(mode_id)
    if manifest is None or cue_id not in manifest.cues:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="cue saved but failed to reload",
        )
    return manifest.cues[cue_id]


@router.post(
    "/{mode_id}/cues",
    response_model=CueSpec,
    status_code=status.HTTP_201_CREATED,
)
def create_cue(mode_id: str, payload: CreateCueRequest, _: CurrentUser) -> CueSpec:
    _validate_slug(payload.id, "cue")
    mode_dir = _mode_dir_or_404(mode_id)
    cue_path = mode_dir / "cues" / f"{payload.id}.yaml"
    if cue_path.exists():
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=f"cue '{payload.id}' already exists in mode '{mode_id}'",
        )
    return _save_cue_yaml(cue_path, _cue_yaml(payload.id, payload), mode_id, payload.id)


@router.put("/{mode_id}/cues/{cue_id}", response_model=CueSpec)
def update_cue(
    mode_id: str, cue_id: str, payload: CueBody, _: CurrentUser
) -> CueSpec:
    _validate_slug(cue_id, "cue")
    mode_dir = _mode_dir_or_404(mode_id)
    cue_path = mode_dir / "cues" / f"{cue_id}.yaml"
    if not cue_path.is_file():
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail=f"cue '{cue_id}' not found in mode '{mode_id}'",
        )
    return _save_cue_yaml(cue_path, _cue_yaml(cue_id, payload), mode_id, cue_id)


@router.delete(
    "/{mode_id}/cues/{cue_id}", status_code=status.HTTP_204_NO_CONTENT
)
def delete_cue(mode_id: str, cue_id: str, _: CurrentUser) -> None:
    _validate_slug(cue_id, "cue")
    mode_dir = _mode_dir_or_404(mode_id)
    cue_path = mode_dir / "cues" / f"{cue_id}.yaml"
    if not cue_path.is_file():
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail=f"cue '{cue_id}' not found",
        )
    cue_path.unlink()
    modes_loader.load_all()


# --- EQ presets (one YAML per preset under the mode's presets/ dir) -------


class EffectSpecIn(BaseModel):
    """Loose effect spec — `type` plus arbitrary numeric params (the loader
    validates `type` against the supported set)."""

    type: str = Field(min_length=1)
    model_config = {"extra": "allow"}


class PresetBody(BaseModel):
    name: str = Field(min_length=1, max_length=128)
    description: str | None = None
    effects: list[EffectSpecIn] = Field(default_factory=list)
    volume: float | None = Field(default=None, ge=0.0, le=1.0)
    crossfade_ms: int | None = Field(default=None, ge=0, le=60000)


class CreatePresetRequest(PresetBody):
    id: str = Field(min_length=1, max_length=64)


def _validate_effect_types(effects: list[EffectSpecIn]) -> None:
    for e in effects:
        if e.type not in SUPPORTED_EFFECT_TYPES:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail=f"unknown effect type '{e.type}'",
            )


def _preset_yaml(preset_id: str, body: PresetBody) -> dict:
    out: dict = {"id": preset_id, "name": body.name}
    if body.description:
        out["description"] = body.description
    out["effects"] = [e.model_dump() for e in body.effects]
    if body.volume is not None:
        out["volume"] = body.volume
    if body.crossfade_ms is not None:
        out["crossfade_ms"] = body.crossfade_ms
    return out


def _save_preset_yaml(
    path: Path, payload: dict, mode_id: str, preset_id: str
) -> PresetManifest:
    _write_yaml(path, payload)
    modes_loader.load_all()
    manifest = modes_loader.get_mode(mode_id)
    if manifest is None or preset_id not in manifest.presets:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="preset saved but failed to reload",
        )
    return manifest.presets[preset_id]


@router.post(
    "/{mode_id}/presets",
    response_model=PresetManifest,
    status_code=status.HTTP_201_CREATED,
)
def create_preset(
    mode_id: str, payload: CreatePresetRequest, _: CurrentUser
) -> PresetManifest:
    _validate_slug(payload.id, "preset")
    _validate_effect_types(payload.effects)
    mode_dir = _mode_dir_or_404(mode_id)
    preset_path = mode_dir / "presets" / f"{payload.id}.yaml"
    if preset_path.exists():
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=f"preset '{payload.id}' already exists in mode '{mode_id}'",
        )
    return _save_preset_yaml(
        preset_path, _preset_yaml(payload.id, payload), mode_id, payload.id
    )


@router.put("/{mode_id}/presets/{preset_id}", response_model=PresetManifest)
def update_preset(
    mode_id: str, preset_id: str, payload: PresetBody, _: CurrentUser
) -> PresetManifest:
    _validate_slug(preset_id, "preset")
    _validate_effect_types(payload.effects)
    mode_dir = _mode_dir_or_404(mode_id)
    preset_path = mode_dir / "presets" / f"{preset_id}.yaml"
    if not preset_path.is_file():
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail=f"preset '{preset_id}' not found in mode '{mode_id}'",
        )
    return _save_preset_yaml(
        preset_path, _preset_yaml(preset_id, payload), mode_id, preset_id
    )


@router.delete(
    "/{mode_id}/presets/{preset_id}", status_code=status.HTTP_204_NO_CONTENT
)
def delete_preset(mode_id: str, preset_id: str, _: CurrentUser) -> None:
    _validate_slug(preset_id, "preset")
    mode_dir = _mode_dir_or_404(mode_id)
    preset_path = mode_dir / "presets" / f"{preset_id}.yaml"
    if not preset_path.is_file():
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail=f"preset '{preset_id}' not found",
        )
    preset_path.unlink()
    modes_loader.load_all()


