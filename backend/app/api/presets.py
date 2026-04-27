from fastapi import APIRouter, HTTPException, status
from pydantic import BaseModel

from app.api.deps import CurrentUser
from app.presets import loader as presets_loader
from app.presets.loader import PresetManifest
from app.sync import commit_and_broadcast
from app.sync import state as sync_state

router = APIRouter(prefix="/api/presets", tags=["presets"])


class ActivePresets(BaseModel):
    preset_ids: list[str]


class ReloadResult(BaseModel):
    loaded: list[str]
    errors: dict[str, str]


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
