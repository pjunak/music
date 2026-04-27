from pathlib import Path

from fastapi import APIRouter, HTTPException, Query, status
from fastapi.responses import FileResponse
from pydantic import BaseModel

from app.api.deps import CurrentUser
from app.library import beets_adapter
from app.library.beets_adapter import Track

router = APIRouter(prefix="/api/library", tags=["library"])


class SearchResponse(BaseModel):
    tracks: list[Track]
    limit: int
    offset: int


@router.get("/search", response_model=SearchResponse)
def search(
    _: CurrentUser,
    q: str = Query("", description="Beets query DSL (e.g. 'artist:daft year:2001..2003')"),
    limit: int = Query(100, ge=1, le=500),
    offset: int = Query(0, ge=0),
) -> SearchResponse:
    tracks = beets_adapter.search(q, limit=limit, offset=offset)
    return SearchResponse(tracks=tracks, limit=limit, offset=offset)


@router.get("/tracks/{beets_id}", response_model=Track)
def get_track(beets_id: int, _: CurrentUser) -> Track:
    track = beets_adapter.get_track(beets_id)
    if track is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="track not found")
    return track


@router.get("/tracks/{beets_id}/stream")
def stream_track(beets_id: int, _: CurrentUser) -> FileResponse:
    track = beets_adapter.get_track(beets_id)
    if track is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="track not found")
    path = Path(track.path)
    if not path.is_file():
        # Beets DB references a file that's no longer on disk.
        raise HTTPException(status_code=status.HTTP_410_GONE, detail="track file missing")
    return FileResponse(path, content_disposition_type="inline")
