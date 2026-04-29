"""Library HTTP surface.

The library is the indexed view of MUSIC_DIR. Uploads, browsing, search,
metadata edits, and file moves all funnel through here. The sync layer
references tracks by `id` minted in this index.
"""
from __future__ import annotations

import logging
import shutil
from datetime import datetime
from pathlib import Path
from typing import Annotated, Literal

from fastapi import APIRouter, File, HTTPException, Query, UploadFile, status
from fastapi.responses import FileResponse, Response
from pydantic import BaseModel, Field
from sqlalchemy import select

from app.api.deps import CurrentUser, DbSession
from app.library import index as library_index
from app.models.track import Track

logger = logging.getLogger(__name__)
router = APIRouter(prefix="/api/library", tags=["library"])

# Streamed upload chunk size. 1 MiB balances memory use against syscall count.
_UPLOAD_CHUNK = 1 << 20

SortKey = Literal["title", "artist", "album", "album_artist", "year", "length_s", "track_no", "added_at", "path"]
SortOrder = Literal["asc", "desc"]


# --- response models ------------------------------------------------------


class TrackOut(BaseModel):
    id: int
    path: str
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
    size_bytes: int
    added_at: datetime

    model_config = {"from_attributes": True}


class FolderOut(BaseModel):
    name: str
    path: str
    track_count: int


class TreeResponse(BaseModel):
    path: str
    folders: list[FolderOut]
    tracks: list[TrackOut]


class SearchResponse(BaseModel):
    tracks: list[TrackOut]
    total: int
    limit: int
    offset: int
    sort: SortKey
    order: SortOrder


class UploadResult(BaseModel):
    saved: list[TrackOut]
    destination: str


class RescanResult(BaseModel):
    added: int
    updated: int
    removed: int
    unchanged: int


class TrackMetadataUpdate(BaseModel):
    title: str | None = Field(None, max_length=512)
    artist: str | None = Field(None, max_length=512)
    album_artist: str | None = Field(None, max_length=512)
    album: str | None = Field(None, max_length=512)
    track_no: int | None = Field(None, ge=0, le=9999)
    year: int | None = Field(None, ge=0, le=9999)
    genre: str | None = Field(None, max_length=128)


class TrackMoveRequest(BaseModel):
    destination: str = Field(description="Folder path relative to MUSIC_DIR; '' for root")
    new_filename: str | None = Field(None, description="If set, also rename the file")


class FolderCreateRequest(BaseModel):
    path: str = Field(min_length=1, description="Folder path relative to MUSIC_DIR")


class FolderRenameRequest(BaseModel):
    src: str = Field(min_length=1, description="Source folder path relative to MUSIC_DIR")
    dst: str = Field(min_length=1, description="Destination folder path relative to MUSIC_DIR")


class FolderDeleteResult(BaseModel):
    removed_tracks: int


# --- helpers --------------------------------------------------------------


def _sort_key(track: Track, key: SortKey) -> tuple:
    """Normalised sort key. None values land at the end without TypeErrors."""
    value = getattr(track, key)
    if isinstance(value, str):
        normalized = value.lower().strip()
        if key in ("title", "album", "artist", "album_artist"):
            for article in ("the ", "a ", "an "):
                if normalized.startswith(article):
                    normalized = normalized[len(article):]
                    break
        return (0, normalized)
    if value is None:
        return (1, 0)
    if isinstance(value, datetime):
        return (0, value.timestamp())
    return (0, value)


def _track_or_404(db: DbSession, track_id: int) -> Track:
    track = db.get(Track, track_id)
    if track is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="track not found")
    return track


# --- browse ---------------------------------------------------------------


@router.get("/tree", response_model=TreeResponse)
def get_tree(
    _: CurrentUser,
    db: DbSession,
    path: str = Query("", description="Folder path relative to MUSIC_DIR; empty for root"),
) -> TreeResponse:
    """Direct contents of a folder: subfolder summaries (with recursive
    track counts) plus the audio files immediately in this folder."""
    folders, tracks = library_index.list_folder(db, path)
    return TreeResponse(
        path=path.strip("/"),
        folders=[
            FolderOut(name=f.name, path=f.path, track_count=f.track_count) for f in folders
        ],
        tracks=[TrackOut.model_validate(t) for t in tracks],
    )


@router.get("/search", response_model=SearchResponse)
def search(
    _: CurrentUser,
    db: DbSession,
    q: str = Query("", description="Substring match across title/artist/album/path"),
    limit: int = Query(100, ge=1, le=500),
    offset: int = Query(0, ge=0),
    sort: Annotated[SortKey, Query(description="Sort key")] = "artist",
    order: Annotated[SortOrder, Query(description="Sort direction")] = "asc",
) -> SearchResponse:
    rows = db.scalars(select(Track)).all()
    if q:
        needle = q.lower()
        rows = [
            t
            for t in rows
            if needle in t.title.lower()
            or needle in t.artist.lower()
            or needle in t.album.lower()
            or needle in t.path.lower()
        ]
    rows.sort(key=lambda t: _sort_key(t, sort), reverse=(order == "desc"))
    page = rows[offset : offset + limit]
    return SearchResponse(
        tracks=[TrackOut.model_validate(t) for t in page],
        total=len(rows),
        limit=limit,
        offset=offset,
        sort=sort,
        order=order,
    )


@router.get("/tracks/{track_id}", response_model=TrackOut)
def get_track(track_id: int, _: CurrentUser, db: DbSession) -> TrackOut:
    return TrackOut.model_validate(_track_or_404(db, track_id))


@router.get("/tracks/{track_id}/stream")
def stream_track(track_id: int, _: CurrentUser, db: DbSession) -> FileResponse:
    track = _track_or_404(db, track_id)
    abs_path = library_index.to_absolute(track.path)
    if not abs_path.is_file():
        raise HTTPException(status_code=status.HTTP_410_GONE, detail="track file missing")
    return FileResponse(abs_path, content_disposition_type="inline")


@router.get("/tracks/{track_id}/cover")
def track_cover(track_id: int, _: CurrentUser, db: DbSession) -> Response:
    track = _track_or_404(db, track_id)
    art = library_index.cover_art_for(track)
    if art is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="no cover art")
    data, mime = art
    return Response(content=data, media_type=mime)


# --- file-manager actions -------------------------------------------------


@router.patch("/tracks/{track_id}/metadata", response_model=TrackOut)
def update_metadata(
    track_id: int,
    payload: TrackMetadataUpdate,
    _: CurrentUser,
    db: DbSession,
) -> TrackOut:
    """Edit ID3-style tags on the underlying file, then re-index the row.
    Only fields that were explicitly set in the request are written."""
    track = _track_or_404(db, track_id)
    abs_path = library_index.to_absolute(track.path)
    if not abs_path.is_file():
        raise HTTPException(status_code=status.HTTP_410_GONE, detail="track file missing")

    fields = payload.model_dump(exclude_unset=True)
    if not fields:
        return TrackOut.model_validate(track)

    try:
        library_index.write_tags(abs_path, fields)
    except ValueError as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e

    library_index.scan_paths(db, [abs_path])
    db.refresh(track)
    return TrackOut.model_validate(track)


@router.post("/tracks/{track_id}/move", response_model=TrackOut)
def move_track_file(
    track_id: int,
    payload: TrackMoveRequest,
    _: CurrentUser,
    db: DbSession,
) -> TrackOut:
    """Move the file to another folder under MUSIC_DIR, optionally with a new
    filename. Both the disk move and the index update happen atomically from
    the caller's point of view (we move first; if that succeeds, we update)."""
    track = _track_or_404(db, track_id)
    src = library_index.to_absolute(track.path)
    if not src.is_file():
        raise HTTPException(status_code=status.HTTP_410_GONE, detail="source file missing")

    target_name = payload.new_filename or src.name
    try:
        dest_dir = library_index.ensure_folder(payload.destination)
        target = library_index.safe_join(payload.destination, target_name)
    except ValueError as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e

    if target == src:
        return TrackOut.model_validate(track)
    if target.exists():
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=f"a file already exists at {dest_dir.name}/{target_name}",
        )

    shutil.move(str(src), str(target))
    new_rel = library_index.to_relative(target)
    library_index.update_path(db, track.path, new_rel)
    db.refresh(track)
    return TrackOut.model_validate(track)


@router.delete("/tracks/{track_id}", status_code=status.HTTP_204_NO_CONTENT)
def delete_track(track_id: int, _: CurrentUser, db: DbSession) -> None:
    """Delete the file from disk AND the row from the index. Any playlist
    items referencing this track id cascade-delete (FK on playlist_items)."""
    track = _track_or_404(db, track_id)
    abs_path = library_index.to_absolute(track.path)
    if abs_path.is_file():
        abs_path.unlink()
    db.delete(track)
    db.commit()


# --- upload + rescan ------------------------------------------------------


@router.post(
    "/upload", response_model=UploadResult, status_code=status.HTTP_201_CREATED
)
async def upload(
    _: CurrentUser,
    db: DbSession,
    files: Annotated[list[UploadFile], File()],
    dest: str = Query("Uploads", description="Destination folder under MUSIC_DIR"),
) -> UploadResult:
    """Stream files into `<MUSIC_DIR>/<dest>/<original-name>`, dedupe name
    collisions, then index whatever just landed."""
    if not files:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST, detail="no files provided"
        )

    try:
        dest_dir = library_index.ensure_folder(dest)
    except ValueError as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e

    written: list[Path] = []
    for upload_file in files:
        if not upload_file.filename:
            continue
        try:
            target = library_index.safe_join(dest, upload_file.filename)
        except ValueError as e:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)
            ) from e
        # Dedupe name collisions: foo.mp3 -> foo-1.mp3, foo-2.mp3, ...
        if target.exists():
            stem, suffix = target.stem, target.suffix
            n = 1
            while True:
                candidate = dest_dir / f"{stem}-{n}{suffix}"
                if not candidate.exists():
                    target = candidate
                    break
                n += 1
        with target.open("wb") as out:
            while chunk := await upload_file.read(_UPLOAD_CHUNK):
                out.write(chunk)
        written.append(target)

    indexed = library_index.scan_paths(db, written)
    return UploadResult(
        saved=[TrackOut.model_validate(t) for t in indexed],
        destination=dest.strip("/"),
    )


@router.post("/rescan", response_model=RescanResult)
def rescan(_: CurrentUser, db: DbSession) -> RescanResult:
    """Walk the entire music directory and reconcile the index against disk."""
    summary = library_index.scan_full(db)
    return RescanResult(
        added=summary.added,
        updated=summary.updated,
        removed=summary.removed,
        unchanged=summary.unchanged,
    )


# --- folder ops -----------------------------------------------------------


@router.post(
    "/folders", status_code=status.HTTP_201_CREATED, response_model=FolderOut
)
def create_folder(
    payload: FolderCreateRequest, _: CurrentUser, db: DbSession
) -> FolderOut:
    try:
        abs_path = library_index.ensure_folder(payload.path)
    except ValueError as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e
    rel = library_index.to_relative(abs_path)
    return FolderOut(name=abs_path.name, path=rel, track_count=0)


@router.delete("/folders", response_model=FolderDeleteResult)
def delete_folder(
    _: CurrentUser,
    db: DbSession,
    path: str = Query(..., description="Folder path relative to MUSIC_DIR"),
    recursive: bool = Query(
        False, description="Delete contents too. Otherwise refuses non-empty folders."
    ),
) -> FolderDeleteResult:
    try:
        removed = library_index.delete_folder(db, path, recursive=recursive)
    except FileNotFoundError as e:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail=str(e)) from e
    except ValueError as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e
    return FolderDeleteResult(removed_tracks=removed)


@router.post("/folders/rename", response_model=FolderOut)
def rename_folder(
    payload: FolderRenameRequest, _: CurrentUser, db: DbSession
) -> FolderOut:
    try:
        library_index.rename_folder(db, payload.src, payload.dst)
    except FileNotFoundError as e:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail=str(e)) from e
    except FileExistsError as e:
        raise HTTPException(status_code=status.HTTP_409_CONFLICT, detail=str(e)) from e
    except ValueError as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e
    new_abs = library_index.to_absolute(payload.dst.strip("/"))
    return FolderOut(name=new_abs.name, path=payload.dst.strip("/"), track_count=0)
