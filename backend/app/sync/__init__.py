"""Sync layer module-level helpers.

`commit_and_broadcast` is the single funnel for state-mutating writes from
*any* entry point (WebSocket actions, HTTP endpoints, future integrations).
It guarantees the state machine, the persisted DB row, and connected clients
all see the same state.
"""
from __future__ import annotations

from typing import Any

from app.core.db import SessionLocal
from app.sync.connection import manager
from app.sync.state import machine


async def commit_and_broadcast(mutator: Any) -> tuple[bool, Any]:
    """Apply `mutator` through the state machine. If state changed, broadcast
    to all connected clients. Returns (changed, new_state)."""
    new_state, changed = await machine.apply(mutator, SessionLocal)
    if changed:
        await manager.broadcast_state()
    return (changed, new_state)
