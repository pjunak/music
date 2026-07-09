"""Track open WebSocket connections so the state machine can broadcast."""
from __future__ import annotations

import contextlib
import logging

from fastapi import WebSocket

from app.sync.devices import registry
from app.sync.protocol import PlayerState, StateChanged

logger = logging.getLogger(__name__)


def guest_state_view(state: PlayerState, own_client_id: str | None) -> PlayerState:
    """A guest-safe projection of the state. Guests (a logged-out TV acting as
    an output) still need playback state to render and play, but must NOT learn
    other devices' `client_id`s or the global active-output set. Those ids are
    the unguessable capability tokens that authorize output activation and
    position reports; leaking them to an anonymous socket lets it re-register
    under an active output's id and hijack the shared playback clock. A guest
    only needs to see ITS OWN active membership (to decide whether to play), so
    `active_output_device_ids` is filtered to its own id and `connected_devices`
    — an operator-console concern no guest client reads — is dropped."""
    active = (
        [own_client_id]
        if own_client_id is not None and own_client_id in state.active_output_device_ids
        else []
    )
    return state.model_copy(
        update={"connected_devices": [], "active_output_device_ids": active}
    )


class ConnectionManager:
    def __init__(self) -> None:
        self._sockets: dict[str, WebSocket] = {}
        # connection_ids whose socket is a guest (no/expired session). Their
        # broadcasts are redacted via guest_state_view.
        self._guests: set[str] = set()

    def add(self, device_id: str, ws: WebSocket, *, is_guest: bool = False) -> None:
        self._sockets[device_id] = ws
        if is_guest:
            self._guests.add(device_id)

    def set_guest(self, device_id: str, is_guest: bool) -> None:
        """Update guest status for a live connection (a session can expire
        mid-connection, downgrading an authed socket to guest)."""
        if is_guest:
            self._guests.add(device_id)
        else:
            self._guests.discard(device_id)

    def remove(self, device_id: str) -> None:
        self._sockets.pop(device_id, None)
        self._guests.discard(device_id)

    def reset_for_tests(self) -> None:
        """Drop all tracked connections. For test isolation only."""
        self._sockets.clear()
        self._guests.clear()

    async def broadcast_state(self, state: PlayerState) -> None:
        """Broadcast the player state, redacted per-socket for guests. The full
        state serialization is computed once and shared by all authed sockets;
        each guest gets a small tailored copy (there are only ever a handful)."""
        full: dict | None = None
        for device_id in list(self._sockets.keys()):
            if device_id in self._guests:
                own = registry.client_id_for(device_id)
                message = StateChanged(
                    state=guest_state_view(state, own)
                ).model_dump(mode="json")
            else:
                if full is None:
                    full = StateChanged(state=state).model_dump(mode="json")
                message = full
            await self._send_one(device_id, message)

    async def broadcast(self, message: dict) -> None:
        """Send `message` to every connected client. Used for fire-and-forget
        audio events (SFX, loop ticks, cue stings): each client's engine plays
        them only if it's actually an output (active membership OR a force-local
        guest), so non-output controller tabs simply ignore the frames. Output
        membership is dynamic now, so the client — not the server — gates.
        These carry no device identities, so no per-guest redaction is needed."""
        for device_id in list(self._sockets.keys()):
            await self._send_one(device_id, message)

    async def _send_one(self, device_id: str, message: dict) -> None:
        ws = self._sockets.get(device_id)
        if ws is None:
            return
        try:
            await ws.send_json(message)
        except Exception:
            logger.exception("broadcast to %s failed; dropping connection", device_id)
            self._sockets.pop(device_id, None)
            self._guests.discard(device_id)
            with contextlib.suppress(Exception):
                await ws.close()


manager = ConnectionManager()
