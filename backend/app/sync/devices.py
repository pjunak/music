"""In-memory table of currently-open WebSocket connections.

Two layers of identity now coexist:

- **connection_id** (`dev-…`) — minted per socket, unique even across two tabs
  of the same browser. The connection-manager key.
- **client_id** — the stable per-install token the client sends in `register`.
  The identity that matters for the persistent device registry and for audio-
  output designation; survives refresh and restart.

This module stays DB-free (it's imported by the state machine, the connection
manager and the router). The persistent `is_output` flag is read from the DB
in the router's register handler and *cached* here via `bind` so hot paths
(SFX fan-out, position gating) don't hit the database per event.
"""
from __future__ import annotations

import secrets
from dataclasses import dataclass

from app.sync.protocol import DeviceInfo


@dataclass
class LiveConnection:
    connection_id: str
    client_id: str | None = None
    name: str = ""
    is_output: bool = False  # cached from KnownDevice at register time
    registered: bool = False  # flips true after the client sends `register`

    def to_info(self) -> DeviceInfo:
        cid = self.client_id or self.connection_id
        return DeviceInfo(
            device_id=cid, client_id=cid, name=self.name, is_output=self.is_output
        )


class DeviceRegistry:
    def __init__(self) -> None:
        self._conns: dict[str, LiveConnection] = {}

    def add(self) -> LiveConnection:
        connection_id = f"dev-{secrets.token_urlsafe(8)}"
        conn = LiveConnection(connection_id=connection_id)
        self._conns[connection_id] = conn
        return conn

    def remove(self, connection_id: str) -> None:
        self._conns.pop(connection_id, None)

    def bind(
        self, connection_id: str, *, client_id: str, name: str, is_output: bool
    ) -> LiveConnection | None:
        """Attach the stable identity + cached designation to a live socket
        after its `register` was processed."""
        conn = self._conns.get(connection_id)
        if conn is None:
            return None
        conn.client_id = client_id
        conn.name = name
        conn.is_output = is_output
        conn.registered = True
        return conn

    def get(self, connection_id: str) -> LiveConnection | None:
        return self._conns.get(connection_id)

    def client_id_for(self, connection_id: str) -> str | None:
        conn = self._conns.get(connection_id)
        return conn.client_id if conn is not None else None

    def is_output_connection(self, connection_id: str) -> bool:
        conn = self._conns.get(connection_id)
        return conn is not None and conn.is_output

    def is_output_client(self, client_id: str | None) -> bool:
        if client_id is None:
            return False
        return any(
            c.client_id == client_id and c.is_output for c in self._conns.values()
        )

    def refresh_is_output(self, client_id: str, value: bool) -> None:
        """Push a designation change to every live socket of this device so
        SFX fan-out / position gating react without waiting for a reconnect."""
        for conn in self._conns.values():
            if conn.client_id == client_id:
                conn.is_output = value

    def refresh_name(self, client_id: str, name: str) -> None:
        """Push a rename to every live socket so the next broadcast's
        `connected_devices` shows the new name without a reconnect."""
        for conn in self._conns.values():
            if conn.client_id == client_id:
                conn.name = name

    def has_other_connection(self, client_id: str, exclude_connection_id: str) -> bool:
        """True if another live socket shares this client_id — the multi-tab
        guard that keeps closing one tab from de-activating a device that still
        has another output tab open."""
        return any(
            cid != exclude_connection_id and conn.client_id == client_id
            for cid, conn in self._conns.items()
        )

    def all_infos(self) -> list[DeviceInfo]:
        """Connected devices, one entry per client_id (two tabs collapse to a
        single device). Only registered connections appear."""
        by_client: dict[str, LiveConnection] = {}
        for conn in self._conns.values():
            if not conn.registered or conn.client_id is None:
                continue
            # Last writer wins — fine for name/is_output (same client_id shares
            # both); keeps a single representative entry.
            by_client[conn.client_id] = conn
        return [conn.to_info() for conn in by_client.values()]

    def reset_for_tests(self) -> None:
        """Drop all live connections. For test isolation only."""
        self._conns.clear()


# Module-level singleton — single-process scope.
registry = DeviceRegistry()
