"""Operator-facing device registry — the visible, editable list of devices the
operator has chosen to remember, plus each one's audio-output designation.

The list is **manually curated**: connecting never adds a device here. The
operator saves a (currently-connected) device with `PUT`, which is also how
they (re)name it or flip its `is_output` designation. The data lives in a
standalone file (see `app.devices.store`) so it survives reinstalls.

All endpoints require a signed-in operator.
"""
from __future__ import annotations

from fastapi import APIRouter, HTTPException, status
from pydantic import BaseModel, Field

from app.api.deps import CurrentUser
from app.devices.store import device_store
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
    added_at: str | None = None


class DevicePut(BaseModel):
    name: str = Field(min_length=1, max_length=128)
    is_output: bool = False


def _connected_client_ids() -> set[str]:
    return {info.client_id for info in registry.all_infos()}


def _to_out(record: dict, connected: set[str]) -> DeviceOut:
    return DeviceOut(
        client_id=record["client_id"],
        name=record["name"],
        is_output=record["is_output"],
        connected=record["client_id"] in connected,
        added_at=record.get("added_at"),
    )


async def _broadcast_snapshot() -> None:
    """Push a fresh state_changed so every client's `connected_devices`
    (names + is_output) reflects a registry edit without a reconnect."""
    state = await machine.snapshot()
    await manager.broadcast_state(state)


@router.get("", response_model=list[DeviceOut])
def list_devices(_: CurrentUser) -> list[DeviceOut]:
    """Every remembered device, including offline ones — the whole point is to
    manage a TV that's powered down right now. (Currently-connected devices the
    operator hasn't saved yet are surfaced client-side from PlayerState's
    `connected_devices` with an 'Add' affordance.)"""
    connected = _connected_client_ids()
    return [_to_out(rec, connected) for rec in device_store.list()]


@router.put("/{client_id}", response_model=DeviceOut)
async def save_device(client_id: str, payload: DevicePut, _: CurrentUser) -> DeviceOut:
    """Remember a device (the manual 'save'/'add'), or update an existing one's
    name / output designation. Idempotent create-or-update.

    A device that ends up NOT a designated output is also dropped from the live
    active outputs immediately — a de-designated device must stop being a
    speaker. Designating one ON never auto-activates it (still manual)."""
    record = device_store.put(client_id, payload.name, payload.is_output)
    registry.refresh_is_output(client_id, payload.is_output)
    registry.refresh_name(client_id, payload.name)

    if not payload.is_output:
        changed, _state = await commit_and_broadcast(
            sync_state.remove_active_output(client_id)
        )
        if not changed:
            await _broadcast_snapshot()
    else:
        await _broadcast_snapshot()

    return _to_out(record, _connected_client_ids())


@router.delete("/{client_id}", status_code=status.HTTP_204_NO_CONTENT)
async def forget_device(client_id: str, _: CurrentUser) -> None:
    """Forget a device — drop it from the saved list and from active outputs.
    If it's still connected it stays usable as a guest/local player but can no
    longer be a designated/coordinated output until saved again."""
    if not device_store.delete(client_id):
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="device not found")
    registry.refresh_is_output(client_id, False)
    await commit_and_broadcast(sync_state.remove_active_output(client_id))
