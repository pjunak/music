"""Beets ingest via subprocess.

Keeps Beets as the single writer to its own DB. We invoke the `beet` CLI
with specific flags and machine-readable output where possible.
"""
from __future__ import annotations

import asyncio
import logging
import re
from dataclasses import dataclass
from pathlib import Path

from app.core.config import get_settings

logger = logging.getLogger(__name__)


@dataclass
class IngestResult:
    returncode: int
    stdout: str
    stderr: str
    imported: int  # files successfully ingested into the library
    skipped: int   # files beets refused to import (no match, duplicate, etc.)

    @property
    def ok(self) -> bool:
        return self.returncode == 0


# Beet's stdout in `-A -q` mode includes lines like:
#   /path/to/file.mp3 -> /library/Artist/Album/01 - Title.mp3
# These are the successful imports. Skipped items show up as:
#   Skipping.
# (Plus context lines about why.) We count both for a quick summary.
_IMPORT_LINE = re.compile(r"->")
_SKIP_LINE = re.compile(r"^\s*Skipping\.?\s*$", re.MULTILINE)


def _summarize(stdout: str) -> tuple[int, int]:
    imported = sum(1 for line in stdout.splitlines() if _IMPORT_LINE.search(line))
    skipped = len(_SKIP_LINE.findall(stdout))
    return imported, skipped


async def run_autoimport(
    path: Path | None = None, *, autotag: bool = False
) -> IngestResult:
    """Run `beet import` against the incoming directory (or a subpath).

    With ``autotag=False`` (the default), passes ``-A`` so files are imported
    as-is using their embedded ID3/Vorbis tags — works for arbitrary content,
    custom audio, anything Beets can't recognise via MusicBrainz.

    With ``autotag=True``, passes ``-q`` so Beets queries MusicBrainz, accepts
    strong matches automatically, and skips weak ones. Use this for music
    that's already well-tagged and likely in MB's database.
    """
    target = Path(path) if path is not None else get_settings().incoming_dir
    if not target.exists():
        raise FileNotFoundError(f"ingest target does not exist: {target}")

    args = ["beet", "import"]
    if autotag:
        # Quiet auto-tag: accept strong matches, skip weak ones, never prompt.
        args.append("-q")
    else:
        # No auto-tag at all: take metadata from existing tags. -q ensures
        # beet still doesn't try to prompt on edge cases.
        args.extend(["-A", "-q"])
    args.append(str(target))

    proc = await asyncio.create_subprocess_exec(
        *args,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    stdout_bytes, stderr_bytes = await proc.communicate()
    stdout = stdout_bytes.decode("utf-8", errors="replace")
    stderr = stderr_bytes.decode("utf-8", errors="replace")
    imported, skipped = _summarize(stdout)
    result = IngestResult(
        returncode=proc.returncode or 0,
        stdout=stdout,
        stderr=stderr,
        imported=imported,
        skipped=skipped,
    )
    if not result.ok:
        logger.warning("beet import failed (%s): %s", result.returncode, result.stderr)
    return result
