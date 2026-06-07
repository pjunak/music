"""Track open WebSocket connections so the state machine can broadcast."""
from __future__ import annotations

import contextlib
import logging

from fastapi import WebSocket

from app.sync.protocol import PlayerState, StateChanged

logger = logging.getLogger(__name__)


class ConnectionManager:
    def __init__(self) -> None:
        self._sockets: dict[str, WebSocket] = {}

    def add(self, device_id: str, ws: WebSocket) -> None:
        self._sockets[device_id] = ws

    def remove(self, device_id: str) -> None:
        self._sockets.pop(device_id, None)

    def reset_for_tests(self) -> None:
        """Drop all tracked connections. For test isolation only."""
        self._sockets.clear()

    async def broadcast_state(self, state: PlayerState) -> None:
        if not self._sockets:
            return
        message = StateChanged(state=state).model_dump(mode="json")
        await self._send_to_each(self._sockets.keys(), message)

    async def broadcast(self, message: dict) -> None:
        """Send `message` to every connected client, regardless of
        capabilities. Use for events that any consumer might care about
        (sfx fired, future audit events, etc.)."""
        if not self._sockets:
            return
        await self._send_to_each(self._sockets.keys(), message)

    async def broadcast_to_outputs(self, message: dict) -> None:
        """Send `message` only to connections whose device is a designated
        audio output. Used for fire-and-forget audio events (SFX) that only
        the playback devices need to act on — controller-only clients ignore
        them."""
        if not self._sockets:
            return
        from app.sync.devices import registry

        targets = [
            connection_id
            for connection_id in self._sockets
            if registry.is_output_connection(connection_id)
        ]
        await self._send_to_each(targets, message)

    async def _send_to_each(self, device_ids, message: dict) -> None:
        for device_id in list(device_ids):
            ws = self._sockets.get(device_id)
            if ws is None:
                continue
            try:
                await ws.send_json(message)
            except Exception:
                logger.exception("broadcast to %s failed; dropping connection", device_id)
                self._sockets.pop(device_id, None)
                with contextlib.suppress(Exception):
                    await ws.close()


manager = ConnectionManager()
