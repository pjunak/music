"""HTTP polling fallback for sync state.

The `/api/ws` WebSocket is the primary push channel — every modern client
uses it exclusively. This endpoint exists for clients that can't establish
a `wss://` connection but *can* do HTTPS XHR. The canonical example is an
old smart-TV browser whose root-CA store predates the chain root currently
served by the reverse proxy: the user can dismiss the page-level cert
warning by hand, after which page subresources (XHR/fetch) inherit the
cert exception — but WebSocket handshakes get re-validated independently
and silently rejected. `compat-mode.js` (the compatibility-mode
fallback) detects the dead WS and switches to polling this endpoint instead.

Auth: `OptionalUser`, matching the WS endpoint's guest-friendly contract
(the `/api/ws` handler in `app/sync/router.py`). A logged-out TV bookmark can still read state
and play whatever the controller (the logged-in user on another device)
queues up. Anything that mutates state still requires a real login on the
mutating endpoints — read-only here is safe.
"""
from __future__ import annotations

from typing import Annotated

from fastapi import APIRouter, Query

from app.api.deps import OptionalUser
from app.sync import state as sync_state
from app.sync.connection import guest_state_view
from app.sync.protocol import PlayerState

router = APIRouter(prefix="/api/sync", tags=["sync"])


@router.get("/state", response_model=PlayerState)
async def get_state(
    user: OptionalUser,
    client_id: Annotated[str | None, Query(min_length=1, max_length=64)] = None,
) -> PlayerState:
    """Return the current `PlayerState` — the same payload the WS endpoint
    pushes as a `state_snapshot` on open and as `state_changed` after every
    mutation. Polling clients call this every ~2s; the response is
    cheap (in-memory snapshot + Pydantic serialization, no DB hit)."""
    state = await sync_state.machine.snapshot()
    return state if user is not None else guest_state_view(state, client_id)
