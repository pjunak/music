"""Sync layer module-level helpers.

`commit_and_broadcast` is the single funnel for state-mutating writes from
*any* entry point (WebSocket actions, HTTP endpoints, future integrations).
It guarantees the state machine, the persisted DB row, and connected clients
all see the same state.
"""
from __future__ import annotations

from typing import Any

import anyio

from app.core.db import SessionLocal
from app.sync.connection import manager
from app.sync.state import machine


async def commit_and_broadcast(
    mutator: Any,
    *,
    force_broadcast: bool = False,
    shield_commit_timeout_s: float | None = None,
) -> tuple[bool, Any]:
    """Apply `mutator` through the state machine. If state changed, broadcast
    to all connected clients. Returns (changed, new_state).

    Disconnect cleanup may shield the apply/persist phase from its endpoint
    task's cancellation, with a deadline so shutdown remains bounded. Network
    broadcasts deliberately stay outside that shield and keep the connection
    manager's per-send deadlines.
    """
    if shield_commit_timeout_s is None:
        new_state, changed = await machine.apply(mutator, SessionLocal)
    else:
        with anyio.CancelScope(shield=True):
            with anyio.fail_after(shield_commit_timeout_s):
                new_state, changed = await machine.apply(mutator, SessionLocal)
    if changed or force_broadcast:
        await manager.broadcast_state()
    return (changed, new_state)
