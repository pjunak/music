"""File-backed store of operator-remembered devices.

The list is **manually curated**: a device is only ever added by an explicit
operator action (`put`), never auto-created when a device connects. It lives in
its own JSON file (`settings.devices_file`) so it persists across reinstalls
and `app.db` wipes.

Shape on disk:

    {
      "<client_id>": {"name": "Living Room TV", "is_output": true,
                      "added_at": "2026-06-05T12:00:00+00:00"},
      ...
    }

An in-memory dict is the live source of truth (reads are instant, so the WS hot
path doesn't touch disk); every mutation write-through-persists atomically.
"""
from __future__ import annotations

import json
import logging
import threading
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from app.core.config import get_settings

logger = logging.getLogger(__name__)


class DeviceStore:
    def __init__(self) -> None:
        self._lock = threading.RLock()
        self._devices: dict[str, dict[str, Any]] = {}

    def _path(self) -> Path:
        # Read fresh each time so tests (which swap DEVICES_FILE + clear the
        # settings cache) and any runtime reconfig are respected.
        return Path(get_settings().devices_file)

    def load(self) -> None:
        """Hydrate from disk. A missing or corrupt file is a valid empty
        state — never a boot-blocker."""
        path = self._path()
        data: dict[str, dict[str, Any]] = {}
        if path.is_file():
            try:
                raw = json.loads(path.read_text(encoding="utf-8"))
                if isinstance(raw, dict):
                    data = {
                        str(cid): dict(rec)
                        for cid, rec in raw.items()
                        if isinstance(rec, dict)
                    }
            except (json.JSONDecodeError, OSError, ValueError):
                logger.exception(
                    "device store at %s is unreadable; starting empty", path
                )
        with self._lock:
            self._devices = data
        logger.info("device store loaded (%d device(s)) from %s", len(data), path)

    def _persist(self) -> None:
        path = self._path()
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_name(f".{path.name}.tmp")
        tmp.write_text(
            json.dumps(self._devices, indent=2, sort_keys=True), encoding="utf-8"
        )
        tmp.replace(path)  # atomic on the same filesystem

    @staticmethod
    def _with_id(client_id: str, rec: dict[str, Any]) -> dict[str, Any]:
        return {
            "client_id": client_id,
            "name": rec.get("name", ""),
            "is_output": bool(rec.get("is_output", False)),
            "added_at": rec.get("added_at"),
        }

    def list(self) -> list[dict[str, Any]]:
        """All remembered devices — outputs first, then alphabetical."""
        with self._lock:
            items = [self._with_id(cid, rec) for cid, rec in self._devices.items()]
        items.sort(key=lambda d: (not d["is_output"], d["name"].lower()))
        return items

    def get(self, client_id: str) -> dict[str, Any] | None:
        with self._lock:
            rec = self._devices.get(client_id)
            return self._with_id(client_id, rec) if rec is not None else None

    def is_output(self, client_id: str | None) -> bool:
        """Whether this device is designated 'output by default' — i.e. should
        auto-activate as a live output when it connects."""
        if client_id is None:
            return False
        with self._lock:
            rec = self._devices.get(client_id)
            return bool(rec and rec.get("is_output"))

    def put(self, client_id: str, name: str, is_output: bool) -> dict[str, Any]:
        """Add or update a remembered device (the manual 'save' action).
        Preserves `added_at` on update."""
        with self._lock:
            existing = self._devices.get(client_id)
            added_at = (
                existing.get("added_at")
                if existing is not None
                else datetime.now(UTC).isoformat()
            )
            self._devices[client_id] = {
                "name": name,
                "is_output": bool(is_output),
                "added_at": added_at,
            }
            self._persist()
            return self._with_id(client_id, self._devices[client_id])

    def delete(self, client_id: str) -> bool:
        with self._lock:
            if client_id not in self._devices:
                return False
            del self._devices[client_id]
            self._persist()
            return True

    def reset_for_tests(self) -> None:
        """Clear memory + remove the on-disk file. Test isolation only."""
        with self._lock:
            self._devices = {}
            path = self._path()
            if path.is_file():
                path.unlink()


# Module-level singleton — single-process scope.
device_store = DeviceStore()
