"""Operator-facing diagnostics endpoint.

Surfaces in-memory state that's otherwise only visible in server logs:
last scan/reload timestamps, mode load errors (a per-mode preset error is
folded into that mode's error string), and live device/output counts. Read
by the frontend's Diagnostics tab so the
operator can see why a YAML edit didn't take effect (or whether the
library got reindexed) without SSH'ing into the host.

Auth: behind `CurrentUser` since it leaks operational detail.
"""
from __future__ import annotations

from fastapi import APIRouter
from pydantic import BaseModel
from sqlalchemy import func, select

from app.api.deps import CurrentUser, DbSession
from app.library import index as library_index
from app.models.track import Track
from app.modes import loader as modes_loader
from app.sync import state as sync_state
from app.sync.devices import registry

router = APIRouter(prefix="/api/diagnostics", tags=["diagnostics"])


class LoaderStatus(BaseModel):
    """Snapshot of the most recent load_all run for modes or presets.
    `loaded_ids` and `errors` reflect the state as of `last_load_at`.
    Both empty + `last_load_at == None` means load_all hasn't run yet
    (boot still in progress, or the seed dirs were empty)."""

    last_load_at: float | None
    loaded_ids: list[str]
    errors: dict[str, str]


class DiagnosticsResponse(BaseModel):
    """Operational snapshot. Stable JSON keys; the frontend renders
    whatever it knows about and ignores the rest, so future additions
    don't require frontend coordination."""

    track_count: int
    last_scan_at: float | None
    modes: LoaderStatus
    connected_device_count: int
    state_revision: int


@router.get("", response_model=DiagnosticsResponse)
async def get_diagnostics(_: CurrentUser, db: DbSession) -> DiagnosticsResponse:
    track_count = db.scalar(select(func.count(Track.id))) or 0

    modes_result, modes_at = modes_loader.last_load()
    modes_status = LoaderStatus(
        last_load_at=modes_at,
        loaded_ids=list(modes_result.loaded.keys()) if modes_result else [],
        errors=dict(modes_result.errors) if modes_result else {},
    )

    state = await sync_state.machine.snapshot()
    return DiagnosticsResponse(
        track_count=track_count,
        last_scan_at=library_index.last_scan_at(),
        modes=modes_status,
        connected_device_count=len(registry.all_infos()),
        state_revision=state.revision,
    )
