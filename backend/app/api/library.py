"""Library HTTP surface.

The library is the indexed view of MUSIC_DIR. Uploads, browsing, search,
metadata edits, and file moves all funnel through here. The sync layer
references tracks by `id` minted in this index.
"""
from __future__ import annotations

import contextlib
import logging
import shutil
from datetime import datetime
from pathlib import Path
from typing import Annotated, Literal

from fastapi import APIRouter, File, HTTPException, Query, UploadFile, status
from fastapi.responses import FileResponse, Response
from pydantic import BaseModel, Field
from sqlalchemy import case, func, nullsfirst, nullslast, or_, select
from sqlalchemy.sql import ColumnElement

from app.api.deps import CurrentUser, DbSession, OptionalUser
from app.library import index as library_index
from app.models.track import Track

logger = logging.getLogger(__name__)
router = APIRouter(prefix="/api/library", tags=["library"])

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

    # User-entered, DB-only fields. Independent of ID3 — survive file moves
    # and tag rewrites because they're not derived from disk.
    display_title: str
    origin: str

    model_config = {"from_attributes": True}


class FolderOut(BaseModel):
    name: str
    path: str
    track_count: int
    has_children: bool


class TreeResponse(BaseModel):
    path: str
    folders: list[FolderOut]
    tracks: list[TrackOut]


class FoldersResponse(BaseModel):
    folders: list[FolderOut]


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
    # Names skipped because they already existed and conflict="skip" was set.
    skipped: list[str] = Field(default_factory=list)


# How an upload handles a filename that already exists at the destination.
UploadConflict = Literal["rename", "overwrite", "skip"]


class UploadCheckItem(BaseModel):
    dest: str
    name: str


class UploadCheckRequest(BaseModel):
    items: list[UploadCheckItem]


class UploadCheckResponse(BaseModel):
    # The subset of `items` that already exist on disk at their destination.
    collisions: list[UploadCheckItem]


class RescanResult(BaseModel):
    added: int
    updated: int
    removed: int
    unchanged: int


class TrackMetadataUpdate(BaseModel):
    """Mixed bag: tag-backed fields are written to the file's ID3/Vorbis
    tags AND mirrored in the DB. DB-only fields (`display_title`, `origin`)
    are written only to the row — the file on disk is left alone."""

    title: str | None = Field(None, max_length=512)
    artist: str | None = Field(None, max_length=512)
    album_artist: str | None = Field(None, max_length=512)
    album: str | None = Field(None, max_length=512)
    track_no: int | None = Field(None, ge=0, le=9999)
    disc_no: int | None = Field(None, ge=0, le=9999)
    year: int | None = Field(None, ge=0, le=9999)
    genre: str | None = Field(None, max_length=128)
    bpm: int | None = Field(None, ge=0, le=9999)
    display_title: str | None = Field(None, max_length=512)
    origin: str | None = Field(None, max_length=512)


class BulkMetadataUpdate(BaseModel):
    """Apply the same field set to many tracks. Empty / unset fields are
    skipped — only fields the caller explicitly sets are touched. To clear
    a field across a selection, send the empty string explicitly."""

    track_ids: list[int] = Field(min_length=1, max_length=1000)
    updates: TrackMetadataUpdate


class BulkMetadataSkip(BaseModel):
    """One row in the per-track skip list returned by bulk update."""

    track_id: int
    reason: str


class BulkMetadataResult(BaseModel):
    """Bulk-update result. `updated` is the rows that took the change;
    `skipped` is the per-track failures (file missing, format we can't
    write tags for, …) so the operator sees which tracks didn't apply
    instead of silently swallowing the discrepancy."""

    updated: list[TrackOut]
    skipped: list[BulkMetadataSkip]


# Fields that round-trip through ID3 / Vorbis tags. Derived from the library
# index's tag registry (the single source of truth for the tag<->format
# mapping) so this can't drift from what `write_tags` actually persists.
# Anything outside this set is treated as DB-only.
_TAG_BACKED_FIELDS: frozenset[str] = frozenset(library_index.WRITABLE_TAGS)
_DB_ONLY_FIELDS: frozenset[str] = frozenset({"display_title", "origin"})


class TrackMoveRequest(BaseModel):
    destination: str = Field(description="Folder path relative to MUSIC_DIR; '' for root")
    new_filename: str | None = Field(None, description="If set, also rename the file")


class BulkMoveRequest(BaseModel):
    track_ids: list[int] = Field(min_length=1, max_length=1000)
    destination: str = Field(description="Folder path relative to MUSIC_DIR; '' for root")


class BulkActionSkip(BaseModel):
    track_id: int
    reason: str


class BulkMoveResult(BaseModel):
    moved: list[TrackOut]
    skipped: list[BulkActionSkip]


class BulkDeleteRequest(BaseModel):
    track_ids: list[int] = Field(min_length=1, max_length=1000)


class BulkDeleteResult(BaseModel):
    deleted_ids: list[int]
    skipped: list[BulkActionSkip]


class FolderCreateRequest(BaseModel):
    path: str = Field(min_length=1, description="Folder path relative to MUSIC_DIR")


class FolderRenameRequest(BaseModel):
    src: str = Field(min_length=1, description="Source folder path relative to MUSIC_DIR")
    dst: str = Field(min_length=1, description="Destination folder path relative to MUSIC_DIR")


class FolderDeleteResult(BaseModel):
    removed_tracks: int


# --- helpers --------------------------------------------------------------


_STRING_SORT_KEYS: frozenset[SortKey] = frozenset(
    {"title", "artist", "album", "album_artist", "path"}
)
_ARTICLE_STRIP_KEYS: frozenset[SortKey] = frozenset(
    {"title", "artist", "album", "album_artist"}
)


def _sort_expression(key: SortKey) -> ColumnElement:
    """Column expression used for ORDER BY.

    String columns are lowercased+trimmed so sorting is case-insensitive.
    Title/artist/album/album_artist additionally drop a leading "the ",
    "a ", or "an " article so "The Doors" sorts under "doors"."""
    column = getattr(Track, key)
    if key in _STRING_SORT_KEYS:
        normalized = func.lower(func.trim(column))
        if key in _ARTICLE_STRIP_KEYS:
            return case(
                (normalized.like("the %"), func.substr(normalized, 5)),
                (normalized.like("an %"), func.substr(normalized, 4)),
                (normalized.like("a %"), func.substr(normalized, 3)),
                else_=normalized,
            )
        return normalized
    return column


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
            FolderOut(
                name=f.name,
                path=f.path,
                track_count=f.track_count,
                has_children=f.has_children,
            )
            for f in folders
        ],
        tracks=[TrackOut.model_validate(t) for t in tracks],
    )


@router.get("/folders", response_model=FoldersResponse)
def list_all_folders(_: CurrentUser, db: DbSession) -> FoldersResponse:
    """The whole folder hierarchy (any depth) in one response. Powers the
    client-side tree: filtering, type-ahead and auto-reveal need every
    folder up front rather than one lazy level per round trip."""
    return FoldersResponse(
        folders=[
            FolderOut(
                name=f.name,
                path=f.path,
                track_count=f.track_count,
                has_children=f.has_children,
            )
            for f in library_index.all_folders(db)
        ]
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
    stmt = select(Track)
    count_stmt = select(func.count()).select_from(Track)
    if q:
        # Escape LIKE metacharacters so a query like "AC_DC" or "50%" matches
        # the literal text instead of treating _ / % as wildcards.
        needle = f"%{library_index.like_escape(q.lower())}%"
        esc = library_index.LIKE_ESCAPE_CHAR
        haystack = or_(
            func.lower(Track.title).like(needle, escape=esc),
            func.lower(Track.display_title).like(needle, escape=esc),
            func.lower(Track.artist).like(needle, escape=esc),
            func.lower(Track.album).like(needle, escape=esc),
            func.lower(Track.origin).like(needle, escape=esc),
            func.lower(Track.path).like(needle, escape=esc),
        )
        stmt = stmt.where(haystack)
        count_stmt = count_stmt.where(haystack)

    sort_expr = _sort_expression(sort)
    directional = sort_expr.desc() if order == "desc" else sort_expr.asc()
    # Match prior behaviour: ascending pushes nulls to the end, descending
    # pulls them to the front. Tiebreak on id for deterministic pagination.
    ordered = nullsfirst(directional) if order == "desc" else nullslast(directional)
    stmt = stmt.order_by(ordered, Track.id.asc()).limit(limit).offset(offset)

    total = db.scalar(count_stmt) or 0
    rows = db.scalars(stmt).all()
    return SearchResponse(
        tracks=[TrackOut.model_validate(t) for t in rows],
        total=total,
        limit=limit,
        offset=offset,
        sort=sort,
        order=order,
    )


@router.get("/tracks", response_model=list[TrackOut])
def get_tracks_batch(
    _: OptionalUser,
    db: DbSession,
    ids: str = Query(
        ...,
        description=(
            "Comma-separated track ids, e.g. ?ids=1,2,3. Returns the matching "
            "tracks in requested order; duplicate and unknown ids are dropped. "
            "Max 500 ids per request."
        ),
    ),
) -> list[TrackOut]:
    """Resolve many tracks' metadata in one round trip. The queue/history in
    PlayerState are id lists, so a client rendering them would otherwise fan
    out into N calls to ``/tracks/{id}``. Order follows the request (first
    occurrence wins); duplicate and unknown ids are dropped. Guest-accessible,
    matching the single-track endpoint."""
    seen: set[int] = set()
    ordered_ids: list[int] = []
    for raw in ids.split(","):
        token = raw.strip()
        if not token:
            continue
        try:
            track_id = int(token)
        except ValueError:
            raise HTTPException(
                status_code=status.HTTP_422_UNPROCESSABLE_CONTENT,
                detail=f"invalid track id: {token!r}",
            ) from None
        if track_id not in seen:
            seen.add(track_id)
            ordered_ids.append(track_id)
    if len(ordered_ids) > 500:
        raise HTTPException(
            status_code=status.HTTP_422_UNPROCESSABLE_CONTENT,
            detail="too many ids (max 500 per request)",
        )
    if not ordered_ids:
        return []
    rows = db.scalars(select(Track).where(Track.id.in_(ordered_ids))).all()
    by_id = {track.id: track for track in rows}
    return [TrackOut.model_validate(by_id[tid]) for tid in ordered_ids if tid in by_id]


@router.get("/tracks/{track_id}", response_model=TrackOut)
def get_track(track_id: int, _: OptionalUser, db: DbSession) -> TrackOut:
    """Single-track metadata. Open to guests so a logged-out Player tab
    (e.g. a bookmarked TV display) can show what's playing."""
    return TrackOut.model_validate(_track_or_404(db, track_id))


@router.get("/tracks/{track_id}/stream")
def stream_track(track_id: int, _: OptionalUser, db: DbSession) -> FileResponse:
    """Stream audio bytes. Guest-accessible — same rationale as the
    metadata endpoint; a TV display has to actually play the audio."""
    track = _track_or_404(db, track_id)
    abs_path = library_index.to_absolute(track.path)
    if not abs_path.is_file():
        raise HTTPException(status_code=status.HTTP_410_GONE, detail="track file missing")
    return FileResponse(abs_path, content_disposition_type="inline")


@router.get("/tracks/{track_id}/cover")
def track_cover(track_id: int, _: OptionalUser, db: DbSession) -> Response:
    """Cover art. Guest-accessible — Player tab needs it for the room display."""
    track = _track_or_404(db, track_id)
    art = library_index.cover_art_for(track)
    if art is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="no cover art")
    data, mime = art
    return Response(content=data, media_type=mime)


# --- file-manager actions -------------------------------------------------


@router.patch("/tracks/bulk-metadata", response_model=BulkMetadataResult)
def update_metadata_bulk(
    payload: BulkMetadataUpdate,
    _: CurrentUser,
    db: DbSession,
) -> BulkMetadataResult:
    """Apply the same metadata update across many tracks at once.

    Tag-backed fields are written to each file's tags (and a re-scan picks
    them back up); DB-only fields (`display_title`, `origin`) are set
    directly on the row. Files that no longer exist on disk OR can't be
    written are reported in `skipped` with a reason — the operator sees
    exactly which tracks didn't take the change. DB-only fields apply
    unconditionally since they don't read from disk.
    """
    fields = payload.updates.model_dump(exclude_unset=True)
    if not fields:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="no fields to update",
        )
    tag_fields = {k: v for k, v in fields.items() if k in _TAG_BACKED_FIELDS}
    db_only_fields = {k: v for k, v in fields.items() if k in _DB_ONLY_FIELDS}

    tracks = list(
        db.scalars(select(Track).where(Track.id.in_(payload.track_ids))).all()
    )
    if not tracks:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="no tracks matched the supplied ids",
        )

    skipped: list[BulkMetadataSkip] = []
    tag_failed_ids: set[int] = set()
    if tag_fields:
        rescan_paths: list[Path] = []
        for track in tracks:
            abs_path = library_index.to_absolute(track.path)
            if not abs_path.is_file():
                skipped.append(
                    BulkMetadataSkip(track_id=track.id, reason="file missing on disk")
                )
                tag_failed_ids.add(track.id)
                continue
            try:
                library_index.write_tags(abs_path, tag_fields)
            except ValueError as e:
                skipped.append(
                    BulkMetadataSkip(
                        track_id=track.id,
                        reason=f"unsupported format: {e}",
                    )
                )
                tag_failed_ids.add(track.id)
                continue
            except Exception as e:
                skipped.append(
                    BulkMetadataSkip(
                        track_id=track.id,
                        reason=f"tag write failed: {e}",
                    )
                )
                tag_failed_ids.add(track.id)
                continue
            rescan_paths.append(abs_path)
        if rescan_paths:
            library_index.scan_paths(db, rescan_paths)

    if db_only_fields:
        for track in tracks:
            for key, value in db_only_fields.items():
                setattr(track, key, value if value is not None else "")
        db.commit()

    for track in tracks:
        db.refresh(track)
    # A track is "updated" only if every applicable field actually applied.
    # A tag-write failure with no DB-only fields → fully skipped, not in
    # `updated`. With DB-only fields too, the row partly took the update;
    # we still surface that mixed case via `skipped`.
    updated = [
        TrackOut.model_validate(t)
        for t in tracks
        if t.id not in tag_failed_ids or db_only_fields
    ]
    return BulkMetadataResult(updated=updated, skipped=skipped)


@router.patch("/tracks/{track_id}/metadata", response_model=TrackOut)
def update_metadata(
    track_id: int,
    payload: TrackMetadataUpdate,
    _: CurrentUser,
    db: DbSession,
) -> TrackOut:
    """Edit metadata for a single track. Tag-backed fields go to the
    underlying file's tags (and the row is re-indexed from disk); DB-only
    fields are written straight to the row. Only fields explicitly set in
    the request are touched."""
    track = _track_or_404(db, track_id)
    abs_path = library_index.to_absolute(track.path)

    fields = payload.model_dump(exclude_unset=True)
    if not fields:
        return TrackOut.model_validate(track)

    tag_fields = {k: v for k, v in fields.items() if k in _TAG_BACKED_FIELDS}
    db_only_fields = {k: v for k, v in fields.items() if k in _DB_ONLY_FIELDS}

    if tag_fields:
        if not abs_path.is_file():
            raise HTTPException(
                status_code=status.HTTP_410_GONE, detail="track file missing"
            )
        try:
            library_index.write_tags(abs_path, tag_fields)
        except ValueError as e:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)
            ) from e
        library_index.scan_paths(db, [abs_path])

    if db_only_fields:
        for key, value in db_only_fields.items():
            setattr(track, key, value if value is not None else "")
        db.commit()

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
    filename. The disk move and the index update are kept consistent: if the
    DB update raises after the file is moved, we attempt to move the file
    back before surfacing the error so the index and the filesystem don't
    drift."""
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
    try:
        library_index.update_path(db, track.path, new_rel)
    except Exception as e:
        # DB write failed — the file already moved. Try to put it back so
        # filesystem and index stay aligned. If the rollback also fails the
        # caller gets the underlying error and the operator will need to
        # rescan; we don't try to be cleverer than that.
        with contextlib.suppress(Exception):
            shutil.move(str(target), str(src))
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"index update failed after move: {e}",
        ) from e
    db.refresh(track)
    return TrackOut.model_validate(track)


@router.delete("/tracks/{track_id}", status_code=status.HTTP_204_NO_CONTENT)
def delete_track(track_id: int, _: CurrentUser, db: DbSession) -> None:
    """Delete the file from disk AND the row from the index. Any playlist
    items referencing this track id cascade-delete (FK on playlist_items)."""
    track = _track_or_404(db, track_id)
    abs_path = library_index.to_absolute(track.path)
    if abs_path.is_file():
        try:
            abs_path.unlink()
        except OSError as e:
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"file unlink failed: {e}",
            ) from e
    db.delete(track)
    db.commit()


@router.post("/tracks/bulk-move", response_model=BulkMoveResult)
def bulk_move_tracks(
    payload: BulkMoveRequest,
    _: CurrentUser,
    db: DbSession,
) -> BulkMoveResult:
    """Move many tracks to one destination folder, keeping their original
    filenames. Per-track failures (missing source, name collision, etc.) are
    collected into `skipped` so the operator sees exactly what didn't move,
    rather than the whole batch aborting on the first issue."""
    try:
        dest_dir = library_index.ensure_folder(payload.destination)
    except ValueError as e:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)
        ) from e

    moved: list[TrackOut] = []
    skipped: list[BulkActionSkip] = []
    for track_id in payload.track_ids:
        track = db.get(Track, track_id)
        if track is None:
            skipped.append(BulkActionSkip(track_id=track_id, reason="not found"))
            continue
        src = library_index.to_absolute(track.path)
        if not src.is_file():
            skipped.append(
                BulkActionSkip(track_id=track_id, reason="source file missing")
            )
            continue
        target = library_index.safe_join(payload.destination, src.name)
        if target == src:
            moved.append(TrackOut.model_validate(track))
            continue
        if target.exists():
            skipped.append(
                BulkActionSkip(
                    track_id=track_id,
                    reason=f"a file already exists at {dest_dir.name}/{src.name}",
                )
            )
            continue
        shutil.move(str(src), str(target))
        new_rel = library_index.to_relative(target)
        try:
            library_index.update_path(db, track.path, new_rel)
        except Exception as e:
            with contextlib.suppress(Exception):
                shutil.move(str(target), str(src))
            skipped.append(
                BulkActionSkip(track_id=track_id, reason=f"index update failed: {e}")
            )
            continue
        db.refresh(track)
        moved.append(TrackOut.model_validate(track))

    return BulkMoveResult(moved=moved, skipped=skipped)


@router.post("/tracks/bulk-delete", response_model=BulkDeleteResult)
def bulk_delete_tracks(
    payload: BulkDeleteRequest,
    _: CurrentUser,
    db: DbSession,
) -> BulkDeleteResult:
    """Delete many tracks at once. A missing file isn't fatal — the row is
    still dropped from the index so the operator can clean up dangling rows
    after manually removing files. Playlist items cascade via FK."""
    deleted_ids: list[int] = []
    skipped: list[BulkActionSkip] = []
    for track_id in payload.track_ids:
        track = db.get(Track, track_id)
        if track is None:
            skipped.append(BulkActionSkip(track_id=track_id, reason="not found"))
            continue
        abs_path = library_index.to_absolute(track.path)
        if abs_path.is_file():
            try:
                abs_path.unlink()
            except OSError as e:
                skipped.append(
                    BulkActionSkip(
                        track_id=track_id, reason=f"file unlink failed: {e}"
                    )
                )
                continue
        db.delete(track)
        deleted_ids.append(track_id)
    db.commit()
    return BulkDeleteResult(deleted_ids=deleted_ids, skipped=skipped)


# --- upload + rescan ------------------------------------------------------


@router.post(
    "/upload", response_model=UploadResult, status_code=status.HTTP_201_CREATED
)
async def upload(
    _: CurrentUser,
    db: DbSession,
    files: Annotated[list[UploadFile], File()],
    dest: str = Query("Uploads", description="Destination folder under MUSIC_DIR"),
    conflict: Annotated[
        UploadConflict, Query(description="Policy for existing files")
    ] = "rename",
) -> UploadResult:
    """Stream files into `<MUSIC_DIR>/<dest>/<original-name>`, applying the
    `conflict` policy on name collisions, then index whatever just landed."""
    if not files:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST, detail="no files provided"
        )

    try:
        library_index.ensure_folder(dest)
    except ValueError as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e

    written: list[Path] = []
    skipped: list[str] = []
    for upload_file in files:
        if not upload_file.filename:
            continue
        try:
            target = library_index.safe_join(dest, upload_file.filename)
        except ValueError as e:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)
            ) from e
        resolved = library_index.resolve_conflict(target, conflict)
        if resolved is None:
            skipped.append(upload_file.filename)
            continue
        target = resolved
        # Stream into a sibling .partial file, then atomic-rename to the
        # real path. If the upload is interrupted (network drop, server
        # restart), only the .partial is left behind — never a truncated
        # file at the real path that the indexer would pick up with bogus
        # tags. The .partial extension is outside AUDIO_EXTENSIONS so even
        # an orphaned one won't be indexed.
        partial = target.with_name(f".{target.name}.partial")
        try:
            with partial.open("wb") as out:
                while chunk := await upload_file.read(library_index.UPLOAD_CHUNK):
                    out.write(chunk)
            partial.replace(target)
        except Exception:
            if partial.exists():
                with contextlib.suppress(OSError):
                    partial.unlink()
            raise
        written.append(target)

    indexed = library_index.scan_paths(db, written)
    return UploadResult(
        saved=[TrackOut.model_validate(t) for t in indexed],
        destination=dest.strip("/"),
        skipped=skipped,
    )


@router.post("/upload/check", response_model=UploadCheckResponse)
def upload_check(payload: UploadCheckRequest, _: CurrentUser) -> UploadCheckResponse:
    """Report which of the proposed (dest, name) targets already exist on disk,
    so the client can ask the operator how to handle duplicates before sending
    any bytes. Paths that fail traversal validation aren't collisions here —
    the upload itself will reject them with a clear error."""
    collisions: list[UploadCheckItem] = []
    for item in payload.items:
        try:
            target = library_index.safe_join(item.dest, item.name)
        except ValueError:
            continue
        if target.exists():
            collisions.append(item)
    return UploadCheckResponse(collisions=collisions)


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
    # Newly created — empty by definition.
    return FolderOut(name=abs_path.name, path=rel, track_count=0, has_children=False)


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
    has_children = (
        new_abs.is_dir()
        and any(grandchild.is_dir() for grandchild in new_abs.iterdir())
    )
    return FolderOut(
        name=new_abs.name,
        path=payload.dst.strip("/"),
        track_count=0,
        has_children=has_children,
    )
