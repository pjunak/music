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
from fastapi.concurrency import run_in_threadpool
from pydantic import BaseModel, Field

from app.api.deps import CurrentUser
from app.devices.store import device_store
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
    name / default-on designation. Idempotent create-or-update.

    `is_output` here is the "output by default" flag: it controls whether the
    device auto-activates as a live output when it next connects — it does NOT
    gate or change the *current* live active set. Toggling it on/off only
    affects future connects (and the badge in the footer); a device that's
    live right now keeps playing until it's turned off from the Speakers
    popover or disconnects. (Activation is independent — any connected device
    can be ticked on without being saved.)"""
    # The store write hits disk (fsync'd JSON) — keep it off the event loop.
    record = await run_in_threadpool(
        device_store.put, client_id, payload.name, payload.is_output
    )
    registry.refresh_is_output(client_id, payload.is_output)
    registry.refresh_name(client_id, payload.name)
    await _broadcast_snapshot()
    return _to_out(record, _connected_client_ids())


@router.delete("/{client_id}", status_code=status.HTTP_204_NO_CONTENT)
async def forget_device(client_id: str, _: CurrentUser) -> None:
    """Forget a device — drop it from the saved list (so it no longer
    auto-activates by default). If it's still connected it remains usable and,
    if it was a live output, keeps playing this session until turned off."""
    if not await run_in_threadpool(device_store.delete, client_id):
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="device not found")
    registry.refresh_is_output(client_id, False)
    await _broadcast_snapshot()
