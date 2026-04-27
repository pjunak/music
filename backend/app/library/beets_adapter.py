"""Read-only adapter over the Beets library.

Wraps `beets.library.Library` so the rest of the app sees a stable typed
surface (`Track`) regardless of Beets version details. We never mutate the
Beets DB through this adapter — ingest goes through the `beet` CLI.
"""
from __future__ import annotations

import contextlib
from pathlib import Path
from threading import Lock
from typing import TYPE_CHECKING

from pydantic import BaseModel

from app.core.config import get_settings

if TYPE_CHECKING:
    from beets.library import Item, Library


class Track(BaseModel):
    beets_id: int
    title: str
    artist: str
    album_artist: str
    album: str
    track_no: int | None
    disc_no: int | None
    year: int | None
    genre: str
    length_s: float
    bpm: int | None
    path: str  # absolute path on disk


def _from_item(item: Item) -> Track:
    raw_path = item.path
    path_str = raw_path.decode("utf-8", errors="replace") if isinstance(raw_path, bytes) else str(raw_path)
    return Track(
        beets_id=int(item.id),
        title=str(item.title or ""),
        artist=str(item.artist or ""),
        album_artist=str(item.albumartist or item.artist or ""),
        album=str(item.album or ""),
        track_no=int(item.track) if item.track else None,
        disc_no=int(item.disc) if item.disc else None,
        year=int(item.year) if item.year else None,
        genre=str(item.genre or ""),
        length_s=float(item.length or 0.0),
        bpm=int(item.bpm) if item.bpm else None,
        path=path_str,
    )


_library: Library | None = None
_lock = Lock()


def invalidate_cache() -> None:
    """Drop the cached Library handle so the next call re-checks the DB path.

    Two callers:
    - Tests that swap BEETS_LIBRARY_DB between cases.
    - The ingest endpoint after `beet import` runs — a fresh DB may have
      just been created on disk, and the cached `None` would otherwise stick.

    Closes the underlying sqlite connection before dropping the reference;
    otherwise Python's GC may emit a ResourceWarning at an inconvenient time.
    """
    global _library
    with _lock:
        if _library is not None:
            close = getattr(_library, "_close", None)
            if callable(close):
                with contextlib.suppress(Exception):
                    close()
        _library = None


def _get_library() -> Library | None:
    """Return the cached Library, or None if the DB doesn't exist yet.

    A fresh deployment has no DB until the first `beet import` runs, so
    callers (search, get_track) treat the missing-DB case as an empty
    library rather than a hard error.
    """
    global _library
    if _library is None:
        with _lock:
            if _library is None:
                db_path = get_settings().beets_library_db
                if not Path(db_path).exists():
                    return None
                from beets.library import Library  # imported lazily — heavy

                _library = Library(str(db_path))
    return _library


def get_track(beets_id: int) -> Track | None:
    lib = _get_library()
    if lib is None:
        return None
    item = lib.get_item(beets_id)
    return _from_item(item) if item is not None else None


def search(query: str = "", limit: int | None = None, offset: int = 0) -> list[Track]:
    """Run a Beets query string (e.g. `artist:daft year:2001..2003`)."""
    lib = _get_library()
    if lib is None:
        return []
    results: list[Track] = []
    skipped = 0
    for item in lib.items(query):
        if skipped < offset:
            skipped += 1
            continue
        results.append(_from_item(item))
        if limit is not None and len(results) >= limit:
            break
    return results
