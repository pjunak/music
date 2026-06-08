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
        """Send `message` to every connected client. Used for fire-and-forget
        audio events (SFX, loop ticks, cue stings): each client's engine plays
        them only if it's actually an output (active membership OR a force-local
        guest), so non-output controller tabs simply ignore the frames. Output
        membership is dynamic now, so the client — not the server — gates."""
        if not self._sockets:
            return
        await self._send_to_each(self._sockets.keys(), message)

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
