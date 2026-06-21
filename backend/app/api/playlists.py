import json
import re
from datetime import datetime
from typing import Annotated, Literal

from fastapi import APIRouter, HTTPException, Query, status
from fastapi.responses import Response
from pydantic import BaseModel, Field
from sqlalchemy import select

from app.api.deps import CurrentUser, DbSession
from app.domain import playlists as playlists_domain
from app.models.playlist import Playlist
from app.models.track import Track
from app.modes import loader as modes_loader

router = APIRouter(prefix="/api/playlists", tags=["playlists"])


# --- request / response models ---------------------------------------------


class PlaylistCreate(BaseModel):
    name: str = Field(min_length=1, max_length=256)
    mode_id: str | None = Field(None, max_length=64)
    category: str | None = Field(None, max_length=64)


class PlaylistUpdate(BaseModel):
    name: str | None = Field(None, min_length=1, max_length=256)
    mode_id: str | None = Field(None, max_length=64)
    category: str | None = Field(None, max_length=64)


class PlaylistMeta(BaseModel):
    id: int
    name: str
    mode_id: str | None
    category: str | None
    created_at: datetime
    updated_at: datetime

    model_config = {"from_attributes": True}


class TrackSummary(BaseModel):
    id: int
    path: str
    title: str
    artist: str
    album: str
    length_s: float

    model_config = {"from_attributes": True}


class TrackInPlaylist(BaseModel):
    position: int
    track_id: int
    track: TrackSummary | None  # None when the underlying file was deleted


class TrackAddRequest(BaseModel):
    track_id: int
    position: int | None = None


class TrackMoveRequest(BaseModel):
    to_position: int = Field(ge=0)


# --- helpers ----------------------------------------------------------------


def _get_playlist(db: DbSession, playlist_id: int) -> Playlist:
    pl = db.get(Playlist, playlist_id)
    if pl is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="playlist not found")
    return pl


def _validate_mode(mode_id: str | None) -> None:
    if mode_id is None:
        return
    if modes_loader.get_mode(mode_id) is None:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST, detail=f"unknown mode: {mode_id}"
        )


def _enrich_items(db: DbSession, playlist: Playlist) -> list[TrackInPlaylist]:
    items = playlists_domain.list_items(db, playlist)
    if not items:
        return []
    track_ids = [it.track_id for it in items]
    rows = db.scalars(select(Track).where(Track.id.in_(track_ids))).all()
    by_id = {t.id: t for t in rows}
    return [
        TrackInPlaylist(
            position=it.position,
            track_id=it.track_id,
            track=TrackSummary.model_validate(by_id[it.track_id])
            if it.track_id in by_id
            else None,
        )
        for it in items
    ]


# --- playlist CRUD ----------------------------------------------------------


@router.post("", response_model=PlaylistMeta, status_code=status.HTTP_201_CREATED)
def create_playlist(payload: PlaylistCreate, _: CurrentUser, db: DbSession) -> PlaylistMeta:
    _validate_mode(payload.mode_id)
    pl = Playlist(
        name=payload.name,
        mode_id=payload.mode_id,
        category=payload.category,
    )
    db.add(pl)
    db.commit()
    db.refresh(pl)
    return PlaylistMeta.model_validate(pl)


@router.get("", response_model=list[PlaylistMeta])
def list_playlists(
    _: CurrentUser,
    db: DbSession,
    mode_id: str | None = Query(None),
    category: str | None = Query(None),
) -> list[PlaylistMeta]:
    # Playlists are per-mode now (no global tier) — filtering by mode_id returns
    # exactly that mode's playlists.
    stmt = select(Playlist).order_by(Playlist.created_at.desc())
    if mode_id is not None:
        stmt = stmt.where(Playlist.mode_id == mode_id)
    if category is not None:
        stmt = stmt.where(Playlist.category == category)
    rows = db.scalars(stmt).all()
    return [PlaylistMeta.model_validate(r) for r in rows]


@router.get("/{playlist_id}", response_model=PlaylistMeta)
def get_playlist(playlist_id: int, _: CurrentUser, db: DbSession) -> PlaylistMeta:
    return PlaylistMeta.model_validate(_get_playlist(db, playlist_id))


@router.patch("/{playlist_id}", response_model=PlaylistMeta)
def update_playlist(
    playlist_id: int, payload: PlaylistUpdate, _: CurrentUser, db: DbSession
) -> PlaylistMeta:
    pl = _get_playlist(db, playlist_id)
    # exclude_unset distinguishes "field omitted" (leave alone) from an
    # explicit null (clear) — without it, sending `category: null` to blank a
    # category was silently dropped by the old `is not None` guard.
    fields = payload.model_dump(exclude_unset=True)
    if fields.get("name") is not None:
        pl.name = fields["name"]
    # mode_id is never cleared (a null would orphan the playlist out of every
    # mode); only re-point it when a real id is supplied.
    if fields.get("mode_id") is not None:
        _validate_mode(fields["mode_id"])
        pl.mode_id = fields["mode_id"]
    if "category" in fields:
        pl.category = fields["category"]
    db.commit()
    db.refresh(pl)
    return PlaylistMeta.model_validate(pl)


@router.delete("/{playlist_id}", status_code=status.HTTP_204_NO_CONTENT)
def delete_playlist(playlist_id: int, _: CurrentUser, db: DbSession) -> None:
    pl = _get_playlist(db, playlist_id)
    db.delete(pl)
    db.commit()


# --- tracks within a playlist ----------------------------------------------


@router.get("/{playlist_id}/tracks", response_model=list[TrackInPlaylist])
def get_tracks(playlist_id: int, _: CurrentUser, db: DbSession) -> list[TrackInPlaylist]:
    pl = _get_playlist(db, playlist_id)
    return _enrich_items(db, pl)


@router.post(
    "/{playlist_id}/tracks", response_model=TrackInPlaylist, status_code=status.HTTP_201_CREATED
)
def add_track(
    playlist_id: int, payload: TrackAddRequest, _: CurrentUser, db: DbSession
) -> TrackInPlaylist:
    pl = _get_playlist(db, playlist_id)
    if db.get(Track, payload.track_id) is None:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="track not in library"
        )
    try:
        item = playlists_domain.add_track(db, pl, payload.track_id, payload.position)
    except playlists_domain.PositionOutOfRange as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e

    track = db.get(Track, item.track_id)
    return TrackInPlaylist(
        position=item.position,
        track_id=item.track_id,
        track=TrackSummary.model_validate(track) if track is not None else None,
    )


@router.delete("/{playlist_id}/tracks/{position}", status_code=status.HTTP_204_NO_CONTENT)
def remove_track(
    playlist_id: int, position: int, _: CurrentUser, db: DbSession
) -> None:
    pl = _get_playlist(db, playlist_id)
    try:
        playlists_domain.remove_track(db, pl, position)
    except playlists_domain.PositionOutOfRange as e:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail=str(e)) from e


@router.patch("/{playlist_id}/tracks/{position}", status_code=status.HTTP_204_NO_CONTENT)
def move_track(
    playlist_id: int,
    position: int,
    payload: TrackMoveRequest,
    _: CurrentUser,
    db: DbSession,
) -> None:
    pl = _get_playlist(db, playlist_id)
    try:
        playlists_domain.move_track(db, pl, position, payload.to_position)
    except playlists_domain.PositionOutOfRange as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e


# --- export -----------------------------------------------------------------


_FILENAME_SAFE_RE = re.compile(r"[^A-Za-z0-9._-]+")


def _safe_filename(name: str) -> str:
    """Sanitise a playlist name into something safe for Content-Disposition.
    Strict ASCII subset since Content-Disposition's quoted-string is finicky
    with non-ASCII; the operator can rename after download if needed."""
    cleaned = _FILENAME_SAFE_RE.sub("_", name).strip("._")
    return cleaned or "playlist"


def _build_m3u(name: str, items: list[TrackInPlaylist]) -> str:
    """Extended M3U with per-track #EXTINF lines. Paths are relative to
    MUSIC_DIR — the operator drops this file alongside the music tree (or
    at the tree root) and it just works in any player that understands M3U.

    `m3u8` (UTF-8) is the modern variant; we always emit UTF-8 so the
    extension is `.m3u8`."""
    lines = ["#EXTM3U", f"#PLAYLIST:{name}"]
    for it in items:
        if it.track is None:
            # Underlying file is gone. Leave a comment so the operator sees
            # the gap when reading the file manually, but don't emit the
            # path (most players will warn on a missing file).
            lines.append(f"# missing track #{it.track_id}")
            continue
        length = max(0, round(it.track.length_s))
        artist = it.track.artist or ""
        title = it.track.title or it.track.path.rsplit("/", 1)[-1]
        # `,` is the EXTINF separator; an artist or title with a comma
        # confuses some parsers. Replace with a hyphen rather than escape.
        display = f"{artist} - {title}".replace("\n", " ")
        lines.append(f"#EXTINF:{length},{display}")
        lines.append(it.track.path)
    return "\n".join(lines) + "\n"


def _build_json(playlist: Playlist, items: list[TrackInPlaylist]) -> str:
    """Structured JSON dump intended for round-tripping (re-import later)
    or external tools. Includes the full track row so an importer can
    reconstruct without hitting the library's row-id (which is local)."""
    payload = {
        "playlist": {
            "name": playlist.name,
            "mode_id": playlist.mode_id,
            "category": playlist.category,
            "created_at": playlist.created_at.isoformat()
            if isinstance(playlist.created_at, datetime)
            else str(playlist.created_at),
        },
        "tracks": [
            {
                "position": it.position,
                "path": it.track.path if it.track else None,
                "title": it.track.title if it.track else None,
                "artist": it.track.artist if it.track else None,
                "album": it.track.album if it.track else None,
                "length_s": it.track.length_s if it.track else None,
            }
            for it in items
        ],
    }
    return json.dumps(payload, indent=2, ensure_ascii=False) + "\n"


@router.get("/{playlist_id}/export")
def export_playlist(
    playlist_id: int,
    _: CurrentUser,
    db: DbSession,
    format: Annotated[Literal["m3u", "json"], Query()] = "m3u",
) -> Response:
    """Download the playlist as M3U (default; for VLC, foobar2000, etc.) or
    JSON (structured, includes track metadata for re-import). M3U paths are
    relative to MUSIC_DIR — drop the file at MUSIC_DIR's root to import."""
    pl = _get_playlist(db, playlist_id)
    items = _enrich_items(db, pl)
    safe_name = _safe_filename(pl.name)
    if format == "m3u":
        body = _build_m3u(pl.name, items)
        return Response(
            content=body,
            media_type="application/vnd.apple.mpegurl",
            headers={
                "Content-Disposition": f'attachment; filename="{safe_name}.m3u8"',
            },
        )
    body = _build_json(pl, items)
    return Response(
        content=body,
        media_type="application/json",
        headers={
            "Content-Disposition": f'attachment; filename="{safe_name}.json"',
        },
    )
