"""Beets ingest via subprocess.

Keeps Beets as the single writer to its own DB. We invoke the `beet` CLI
with specific flags and machine-readable output where possible.

This module is intentionally minimal at the scaffold stage — the review-queue
UI that resolves low-confidence matches will be built on top of it.
"""
from __future__ import annotations

import asyncio
import logging
from dataclasses import dataclass
from pathlib import Path

from app.core.config import get_settings

logger = logging.getLogger(__name__)


@dataclass
class IngestResult:
    returncode: int
    stdout: str
    stderr: str

    @property
    def ok(self) -> bool:
        return self.returncode == 0


async def run_autoimport(path: Path | None = None) -> IngestResult:
    """Run `beet import -q` against the incoming directory (or a subpath).

    -q = quiet, auto-accept strong matches, skip weak ones.
    Files Beets can't confidently match stay in place for manual review later.
    """
    target = Path(path) if path is not None else get_settings().incoming_dir
    if not target.exists():
        raise FileNotFoundError(f"ingest target does not exist: {target}")

    proc = await asyncio.create_subprocess_exec(
        "beet",
        "import",
        "-q",
        str(target),
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    stdout, stderr = await proc.communicate()
    result = IngestResult(
        returncode=proc.returncode or 0,
        stdout=stdout.decode("utf-8", errors="replace"),
        stderr=stderr.decode("utf-8", errors="replace"),
    )
    if not result.ok:
        logger.warning("beet import failed (%s): %s", result.returncode, result.stderr)
    return result
