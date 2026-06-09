"""Server-side interval timers for looping SFX.

Each active loop runs one asyncio task that broadcasts an `sfx_fired` event to
every client every `interval_s` seconds (each client plays it only if it's an
active output / force-local guest), until cancelled. The
loop *records* (id, soundboard, item, interval, volume) live in `PlayerState`
(so every client's LOOPS panel shows the same set); this module owns only the
live timer tasks behind them.

Server-owned (not client-owned) on purpose: a loop must keep firing across a
Console refresh and be visible/stoppable from any client — a per-tab timer
can't do that. Timers do not survive a process restart (PlayerState's
`looping_sfx` is wiped on boot to match).
"""
from __future__ import annotations

import asyncio
import logging

from app.sync.connection import manager
from app.sync.protocol import SfxFired

logger = logging.getLogger(__name__)

_tasks: dict[str, asyncio.Task] = {}


async def _run(
    loop_id: str, soundboard_id: str, item_path: str, interval_s: float, volume: float
) -> None:
    payload = SfxFired(
        soundboard_id=soundboard_id, item_path=item_path, volume=volume
    ).model_dump(mode="json")
    while True:
        await asyncio.sleep(interval_s)
        # Broadcast to all sockets; each client's engine plays it only when
        # it's an active output (or a force-local guest). See connection.broadcast.
        # The try is *inside* the loop so a single broadcast hiccup is logged and
        # retried next tick instead of killing the timer (which would orphan the
        # LOOPS-panel entry that has no timer behind it).
        try:
            await manager.broadcast(payload)
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.exception("looping sfx '%s' broadcast failed", loop_id)


def start(
    loop_id: str, soundboard_id: str, item_path: str, interval_s: float, volume: float
) -> None:
    """(Re)start the timer for `loop_id`. Replaces any existing timer with the
    same id so a re-fire doesn't double up."""
    stop(loop_id)
    _tasks[loop_id] = asyncio.create_task(
        _run(loop_id, soundboard_id, item_path, interval_s, volume)
    )


def stop(loop_id: str) -> None:
    task = _tasks.pop(loop_id, None)
    if task is not None:
        task.cancel()


def stop_all() -> None:
    """Cancel every running loop timer. Used on shutdown / test teardown."""
    for task in _tasks.values():
        task.cancel()
    _tasks.clear()
