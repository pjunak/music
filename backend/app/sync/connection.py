"""Track open WebSocket connections so the state machine can broadcast."""
from __future__ import annotations

import asyncio
import contextlib
import logging

from fastapi import WebSocket

from app.sync.devices import registry
from app.sync.protocol import PlayerState, StateChanged, StateSnapshot

logger = logging.getLogger(__name__)

ABSOLUTE_VOLUME_PROTOCOL_VERSION = 2


def legacy_state_view(state: PlayerState) -> PlayerState:
    """Project absolute device levels into the old master x trim model."""
    master = max(
        [state.default_device_volume, *state.device_volumes.values()],
        default=state.default_device_volume,
    )
    trims = (
        {device_id: level / master for device_id, level in state.device_volumes.items()}
        if master > 1e-9
        else {device_id: 1.0 for device_id in state.device_volumes}
    )
    return state.model_copy(update={"volume": master, "device_volumes": trims})


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
        update={
            "connected_devices": [],
            "active_output_device_ids": active,
            "device_volumes": (
                {own_client_id: state.device_volumes[own_client_id]}
                if own_client_id is not None and own_client_id in state.device_volumes
                else {}
            ),
        }
    )


class ConnectionManager:
    def __init__(self) -> None:
        self._sockets: dict[str, WebSocket] = {}
        # connection_ids whose socket is a guest (no/expired session). Their
        # broadcasts are redacted via guest_state_view.
        self._guests: set[str] = set()
        self._absolute_volume_clients: set[str] = set()
        self._state_broadcast_lock = asyncio.Lock()

    def add(self, device_id: str, ws: WebSocket, *, is_guest: bool = False) -> None:
        self._sockets[device_id] = ws
        if is_guest:
            self._guests.add(device_id)

    async def add_with_snapshot(
        self,
        device_id: str,
        ws: WebSocket,
        *,
        is_guest: bool = False,
    ) -> None:
        """Add a socket and send its baseline inside state-broadcast ordering.

        A mutation either broadcasts before this socket is visible (then the
        snapshot includes it) or after the snapshot. It can never deliver a
        newer delta followed by an older baseline.
        """
        async with self._state_broadcast_lock:
            self.add(device_id, ws, is_guest=is_guest)
            from app.sync.state import machine

            state = await machine.snapshot()
            await self._send_one(
                device_id,
                self._state_message(
                    state,
                    device_id,
                    snapshot=True,
                    assume_absolute=True,
                ),
            )

    def set_guest(self, device_id: str, is_guest: bool) -> None:
        """Update guest status for a live connection (a session can expire
        mid-connection, downgrading an authed socket to guest)."""
        if is_guest:
            self._guests.add(device_id)
        else:
            self._guests.discard(device_id)

    def set_protocol_version(self, device_id: str, protocol_version: int) -> None:
        if protocol_version >= ABSOLUTE_VOLUME_PROTOCOL_VERSION:
            self._absolute_volume_clients.add(device_id)
        else:
            self._absolute_volume_clients.discard(device_id)

    def remove(self, device_id: str) -> None:
        self._sockets.pop(device_id, None)
        self._guests.discard(device_id)
        self._absolute_volume_clients.discard(device_id)

    def reset_for_tests(self) -> None:
        """Drop all tracked connections. For test isolation only."""
        self._sockets.clear()
        self._guests.clear()
        self._absolute_volume_clients.clear()
        self._state_broadcast_lock = asyncio.Lock()

    def _state_message(
        self,
        state: PlayerState,
        device_id: str,
        *,
        snapshot: bool,
        assume_absolute: bool = False,
    ) -> dict:
        absolute = assume_absolute or device_id in self._absolute_volume_clients
        projected = state if absolute else legacy_state_view(state)
        if device_id in self._guests:
            projected = guest_state_view(projected, registry.client_id_for(device_id))
        message = (
            StateSnapshot(state=projected).model_dump(mode="json")
            if snapshot
            else StateChanged(state=projected).model_dump(mode="json")
        )
        if not absolute:
            message["state"].pop("default_device_volume", None)
        return message

    async def broadcast_state(self, _state: PlayerState | None = None) -> None:
        """Broadcast the latest state in one globally ordered sequence.

        Callers may have captured a snapshot before waiting on I/O. Re-read the
        state under the broadcast lock so that stale presence snapshots can
        never arrive after a newer playback mutation.
        """
        async with self._state_broadcast_lock:
            # Local import avoids the state -> registry -> connection cycle at
            # module import time.
            from app.sync.state import machine

            state = await machine.snapshot()
            pending: list[tuple[str, dict]] = []
            for device_id in list(self._sockets.keys()):
                message = self._state_message(
                    state,
                    device_id,
                    snapshot=False,
                )
                pending.append((device_id, message))
            await asyncio.gather(
                *(self._send_one(device_id, message) for device_id, message in pending)
            )

    async def broadcast(self, message: dict) -> None:
        """Send `message` to every connected client. Used for fire-and-forget
        audio events (SFX, loop ticks, cue stings): each client's engine plays
        them only if it's actually an output (active membership OR a force-local
        guest), so non-output controller tabs simply ignore the frames. Output
        membership is dynamic now, so the client — not the server — gates.
        These carry no device identities, so no per-guest redaction is needed."""
        await asyncio.gather(
            *(self._send_one(device_id, message) for device_id in list(self._sockets.keys()))
        )

    async def _send_one(self, device_id: str, message: dict) -> None:
        ws = self._sockets.get(device_id)
        if ws is None:
            return
        try:
            await asyncio.wait_for(ws.send_json(message), timeout=SEND_TIMEOUT_S)
        except Exception:
            logger.exception("broadcast to %s failed; dropping connection", device_id)
            self._sockets.pop(device_id, None)
            self._guests.discard(device_id)
            with contextlib.suppress(Exception):
                await asyncio.wait_for(ws.close(), timeout=CLOSE_TIMEOUT_S)


SEND_TIMEOUT_S = 5.0
CLOSE_TIMEOUT_S = 1.0
manager = ConnectionManager()
