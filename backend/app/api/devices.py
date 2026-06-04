"""Operator-facing device registry — the visible, editable list of every
device that has ever connected, plus its audio-output designation.

This is the management surface for the "which devices may be speakers" model:
output eligibility (`is_output`) is set here by hand, never inferred. All
endpoints require a signed-in operator — guests can appear in the list (their
row is auto-created on connect) but can't see or edit it.
"""
from __future__ import annotations

from datetime import datetime

from fastapi import APIRouter, HTTPException, status
from pydantic import BaseModel, Field

from app.api.deps import CurrentUser, DbSession
from app.domain import known_devices as known_devices_domain
from app.sync import commit_and_broadcast
from app.sync import state as sync_state
from app.sync.connection import manager
from app.sync.devices import registry
from app.sync.state import machine

router = APIRouter(prefix="/api/devices", tags=["devices"])


class DeviceOut(BaseModel):
    client_id: str
    name: str
    is_output: bool
    connected: bool
    created_at: datetime
    last_seen: datetime

    model_config = {"from_attributes": True}


class DeviceUpdate(BaseModel):
    name: str | None = Field(None, min_length=1, max_length=128)
    is_output: bool | None = None


def _connected_client_ids() -> set[str]:
    return {info.client_id for info in registry.all_infos()}


def _to_out(row: known_devices_domain.KnownDevice, connected: set[str]) -> DeviceOut:
    return DeviceOut(
        client_id=row.client_id,
        name=row.name,
        is_output=row.is_output,
        connected=row.client_id in connected,
        created_at=row.created_at,
        last_seen=row.last_seen,
    )


async def _broadcast_snapshot() -> None:
    """Push a fresh state_changed so every client's `connected_devices`
    (names + is_output) reflects a registry edit without a reconnect."""
    state = await machine.snapshot()
    await manager.broadcast_state(state)


@router.get("", response_model=list[DeviceOut])
def list_devices(_: CurrentUser, db: DbSession) -> list[DeviceOut]:
    """Every known device, including ones that are currently offline — the
    point is to manage a TV that's powered down right now."""
    connected = _connected_client_ids()
    return [_to_out(row, connected) for row in known_devices_domain.list_all(db)]


@router.patch("/{client_id}", response_model=DeviceOut)
async def update_device(
    client_id: str, payload: DeviceUpdate, _: CurrentUser, db: DbSession
) -> DeviceOut:
    """Rename a device and/or change its audio-output designation.

    Turning output OFF also drops it from the live active outputs immediately
    (a de-designated device must stop being a speaker). Turning output ON does
    NOT auto-activate it — activation stays a separate, manual step."""
    row = known_devices_domain.get(db, client_id)
    if row is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="device not found")

    if payload.name is not None:
        known_devices_domain.rename(db, client_id, payload.name)
        registry.refresh_name(client_id, payload.name)

    if payload.is_output is not None:
        known_devices_domain.set_is_output(db, client_id, payload.is_output)
        registry.refresh_is_output(client_id, payload.is_output)
        if not payload.is_output:
            # De-designated → stop being a live output right now. This both
            # mutates and broadcasts, so connected_devices updates too.
            await commit_and_broadcast(sync_state.remove_active_output(client_id))
        else:
            await _broadcast_snapshot()
    elif payload.name is not None:
        await _broadcast_snapshot()

    db.refresh(row)
    return _to_out(row, _connected_client_ids())


@router.delete("/{client_id}", status_code=status.HTTP_204_NO_CONTENT)
async def delete_device(client_id: str, _: CurrentUser, db: DbSession) -> None:
    """Forget a device. Also drops it from active outputs. If it's currently
    connected it'll re-appear (with is_output reset to False) on its next
    register — deletion is a clean slate, not a permanent ban."""
    if not known_devices_domain.delete(db, client_id):
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="device not found")
    registry.refresh_is_output(client_id, False)
    await commit_and_broadcast(sync_state.remove_active_output(client_id))
