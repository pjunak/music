"""SFX library HTTP surface.

SFX assets live under SFX_LIBRARY_DIR (separate from the music library).
Mode soundboards reference these files by path relative to that root.
This endpoint serves them with a traversal guard plus a soundboard-
reference check (so only paths *some* soundboard knows about can be
fetched — defence in depth against unknown/crafted requests).
"""
from __future__ import annotations

from pathlib import Path, PurePosixPath

from fastapi import APIRouter, HTTPException, Query, status
from fastapi.responses import FileResponse

from app.api.deps import CurrentUser
from app.core.config import get_settings
from app.modes import loader as modes_loader

router = APIRouter(prefix="/api/sfx", tags=["sfx"])


def _sfx_root() -> Path:
    return get_settings().sfx_library_dir.resolve()


def _normalise(rel: str) -> str:
    """Forward-slash, no leading/trailing slashes, no `..` segments."""
    parts = PurePosixPath(rel.replace("\\", "/")).parts
    if any(p in ("..", "") for p in parts):
        raise ValueError(f"invalid path: {rel!r}")
    return "/".join(parts)


def _any_soundboard_references(rel_path: str) -> bool:
    """True iff some loaded mode has a soundboard item.file == rel_path."""
    for manifest in modes_loader.all_modes().values():
        for soundboard in manifest.soundboards.values():
            for category in soundboard.categories:
                for item in category.items:
                    if item.file == rel_path:
                        return True
    return False


@router.get("/file")
def get_sfx_file(
    _: CurrentUser,
    path: str = Query(..., description="Path relative to SFX_LIBRARY_DIR"),
) -> FileResponse:
    """Stream a single SFX asset. Validation:

    1. Path must normalise without traversal segments.
    2. Path must resolve inside SFX_LIBRARY_DIR.
    3. Path must be referenced by at least one loaded mode's soundboards
       (otherwise unknown files are not servable, even if present on disk)."""
    try:
        rel = _normalise(path)
    except ValueError as e:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)
        ) from e

    if not _any_soundboard_references(rel):
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="sfx path not referenced by any soundboard",
        )

    root = _sfx_root()
    target = (root / rel).resolve()
    try:
        target.relative_to(root)
    except ValueError as e:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST, detail="path escapes sfx root"
        ) from e

    if not target.is_file():
        raise HTTPException(
            status_code=status.HTTP_410_GONE, detail="sfx file missing on disk"
        )
    return FileResponse(target, content_disposition_type="inline")
