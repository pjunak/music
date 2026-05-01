"""Operator-only admin endpoints: backup/export of persistent state.

The "persistent state worth caring about" (per CLAUDE.md) is:

  - `app.db`: auth users, playlists, DB-only metadata (display_title, origin)
  - `MODES_DIR`: mode bundles (manifests, soundboards, scenes)
  - `PRESETS_DIR`: audio-effect preset YAMLs

Music + SFX libraries are *not* included — they're large media managed out-of-
band via SFTP/rsync, and a full backup roundtrip through HTTP is the wrong
mechanism for them. Backing those up is on the operator's filesystem-level
tools.
"""
from __future__ import annotations

import io
import sqlite3
import tarfile
import time
from datetime import UTC, datetime
from pathlib import Path

from fastapi import APIRouter
from fastapi.responses import StreamingResponse
from sqlalchemy.engine.url import make_url

from app.api.deps import CurrentUser
from app.core.config import get_settings

router = APIRouter(prefix="/api/admin", tags=["admin"])


def _sqlite_db_path() -> Path | None:
    """Return the SQLite file path, or None if the configured database isn't
    SQLite (the only dialect we actually deploy). Other dialects skip the DB
    leg of the backup and only ship the YAML directories."""
    url = make_url(get_settings().database_url)
    if not url.drivername.startswith("sqlite"):
        return None
    if url.database in (None, "", ":memory:"):
        return None
    return Path(url.database).resolve()


def _snapshot_sqlite(src: Path) -> bytes:
    """Use SQLite's backup API to get a consistent snapshot even if writes
    are happening. Reading the file directly is unsafe under WAL mode; the
    backup API copies pages atomically into an in-memory destination DB
    that we then serialise.

    Returns the snapshot bytes ready to drop straight into a tar entry.

    `sqlite3.Connection` as a context manager only manages transactions —
    not the connection lifetime — so we close explicitly to avoid leaking
    file descriptors (and the resulting ResourceWarnings under pytest)."""
    src_conn = sqlite3.connect(f"file:{src}?mode=ro", uri=True)
    try:
        dst_conn = sqlite3.connect(":memory:")
        try:
            src_conn.backup(dst_conn)
            # `serialize` returns the raw DB file bytes - safer than dumping
            # SQL since it preserves indexes, FTS tables, etc. exactly.
            return bytes(dst_conn.serialize())
        finally:
            dst_conn.close()
    finally:
        src_conn.close()


def _build_tarball() -> bytes:
    """Build the tarball entirely in memory. The backup is small (DB +
    config YAMLs, no media), so streaming-while-building isn't worth the
    extra plumbing."""
    settings = get_settings()
    buf = io.BytesIO()
    now = time.time()
    with tarfile.open(fileobj=buf, mode="w:gz") as tar:
        db_path = _sqlite_db_path()
        if db_path is not None and db_path.is_file():
            data = _snapshot_sqlite(db_path)
            info = tarfile.TarInfo(name="app.db")
            info.size = len(data)
            info.mtime = int(now)
            tar.addfile(info, io.BytesIO(data))

        for label, src in (("modes", settings.modes_dir), ("presets", settings.presets_dir)):
            src = src.resolve()
            if not src.is_dir():
                continue
            tar.add(src, arcname=label, recursive=True)

    return buf.getvalue()


@router.get("/backup")
def backup(_: CurrentUser) -> StreamingResponse:
    """Download a tar.gz containing app.db + modes/ + presets/. Restore is
    a manual operation: stop the server, replace files in place, restart."""
    payload = _build_tarball()
    timestamp = datetime.now(UTC).strftime("%Y%m%d-%H%M%S")
    filename = f"music-backup-{timestamp}.tar.gz"

    return StreamingResponse(
        iter([payload]),
        media_type="application/gzip",
        headers={
            "Content-Disposition": f'attachment; filename="{filename}"',
            "Content-Length": str(len(payload)),
        },
    )
