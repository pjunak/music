"""In-memory registry of currently-connected clients.

Devices are ephemeral — they exist only while their WebSocket connection is
open. On reconnect a client gets a fresh device id; persistent identification
across sessions isn't a feature today.
"""
from __future__ import annotations

import secrets
from dataclasses import dataclass, field

from app.sync.protocol import DeviceInfo


@dataclass
class ConnectedDevice:
    device_id: str
    name: str = ""
    capabilities: list[str] = field(default_factory=list)
    registered: bool = False  # flips true after the client sends `register`

    def to_info(self) -> DeviceInfo:
        return DeviceInfo(
            device_id=self.device_id, name=self.name, capabilities=list(self.capabilities)
        )


class DeviceRegistry:
    def __init__(self) -> None:
        self._devices: dict[str, ConnectedDevice] = {}

    def add(self) -> ConnectedDevice:
        device_id = f"dev-{secrets.token_urlsafe(8)}"
        device = ConnectedDevice(device_id=device_id)
        self._devices[device_id] = device
        return device

    def remove(self, device_id: str) -> None:
        self._devices.pop(device_id, None)

    def update(
        self, device_id: str, *, name: str, capabilities: list[str]
    ) -> ConnectedDevice | None:
        dev = self._devices.get(device_id)
        if dev is None:
            return None
        dev.name = name
        dev.capabilities = list(capabilities)
        dev.registered = True
        return dev

    def get(self, device_id: str) -> ConnectedDevice | None:
        return self._devices.get(device_id)

    def has_capability(self, device_id: str, capability: str) -> bool:
        dev = self._devices.get(device_id)
        return dev is not None and capability in dev.capabilities

    def all_infos(self) -> list[DeviceInfo]:
        return [d.to_info() for d in self._devices.values() if d.registered]

    def reset_for_tests(self) -> None:
        """Drop all registered devices. For test isolation only."""
        self._devices.clear()


# Module-level singleton — single-process scope.
registry = DeviceRegistry()
