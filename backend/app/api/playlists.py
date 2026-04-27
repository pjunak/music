from datetime import datetime

from fastapi import APIRouter, HTTPException, Query, status
from pydantic import BaseModel, Field, field_validator
from sqlalchemy import select

from app.api.deps import CurrentUser, DbSession
from app.domain import nicknames as nicknames_domain
from app.domain import playlists as playlists_domain
from app.library import beets_adapter
from app.library.beets_adapter import Track
from app.models.playlist import Playlist
from app.modes import loader as modes_loader

router = APIRouter(prefix="/api/playlists", tags=["playlists"])


# --- request / response models ---------------------------------------------


class PlaylistCreate(BaseModel):
    name: str = Field(min_length=1, max_length=256)
    source: str = Field(pattern="^(manual|smart)$")
    mode_id: str | None = Field(None, max_length=64)
    category: str | None = Field(None, max_length=64)
    rules_json: dict | None = None

    @field_validator("rules_json")
    @classmethod
    def _validate_rules(cls, v: dict | None) -> dict | None:
        if v is None:
            return v
        # If present at all, accept any dict shape — the smart resolver only
        # reads `query` and `limit`, others are forward-compatible payload.
        return v


class PlaylistUpdate(BaseModel):
    name: str | None = Field(None, min_length=1, max_length=256)
    mode_id: str | None = Field(None, max_length=64)
    category: str | None = Field(None, max_length=64)
    rules_json: dict | None = None


class PlaylistMeta(BaseModel):
    id: int
    name: str
    mode_id: str | None
    category: str | None
    source: str
    rules_json: dict | None
    created_at: datetime
    updated_at: datetime

    model_config = {"from_attributes": True}


class TrackInPlaylist(BaseModel):
    position: int
    beets_id: int
    display_name: str | None
    track: Track | None


class TrackAddRequest(BaseModel):
    beets_id: int
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


def _validate_smart_rules(source: str, rules_json: dict | None) -> None:
    if source != "smart":
        return
    if not rules_json or not str(rules_json.get("query") or "").strip():
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="smart playlist requires rules_json.query",
        )


def _enrich_tracks(
    db: DbSession,
    playlist: Playlist,
    rows: list[tuple[int, int]],
) -> list[TrackInPlaylist]:
    """rows: list of (position, beets_id). Return TrackInPlaylist with
    display names resolved through the precedence chain and full Track
    metadata fetched from Beets in one batch each."""
    if not rows:
        return []
    beets_ids = [bid for _, bid in rows]
    name_map = nicknames_domain.resolve_names(
        db, beets_ids, mode_id=playlist.mode_id, playlist_id=playlist.id
    )
    track_map = {bid: beets_adapter.get_track(bid) for bid in set(beets_ids)}
    return [
        TrackInPlaylist(
            position=pos,
            beets_id=bid,
            display_name=name_map.get(bid),
            track=track_map.get(bid),
        )
        for pos, bid in rows
    ]


# --- playlist CRUD ----------------------------------------------------------


@router.post("", response_model=PlaylistMeta, status_code=status.HTTP_201_CREATED)
def create_playlist(payload: PlaylistCreate, _: CurrentUser, db: DbSession) -> PlaylistMeta:
    _validate_mode(payload.mode_id)
    _validate_smart_rules(payload.source, payload.rules_json)

    pl = Playlist(
        name=payload.name,
        source=payload.source,
        mode_id=payload.mode_id,
        category=payload.category,
        rules_json=payload.rules_json,
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
    source: str | None = Query(None, pattern="^(manual|smart)$"),
    include_global: bool = Query(True, description="When filtering by mode_id, also include global (mode_id=null) playlists."),
) -> list[PlaylistMeta]:
    stmt = select(Playlist).order_by(Playlist.created_at.desc())
    if mode_id is not None:
        stmt = (
            stmt.where((Playlist.mode_id == mode_id) | (Playlist.mode_id.is_(None)))
            if include_global
            else stmt.where(Playlist.mode_id == mode_id)
        )
    if category is not None:
        stmt = stmt.where(Playlist.category == category)
    if source is not None:
        stmt = stmt.where(Playlist.source == source)
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
    if payload.name is not None:
        pl.name = payload.name
    if payload.mode_id is not None:
        _validate_mode(payload.mode_id)
        pl.mode_id = payload.mode_id
    if payload.category is not None:
        pl.category = payload.category
    if payload.rules_json is not None:
        if pl.source == "smart" and not str(payload.rules_json.get("query") or "").strip():
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="smart playlist requires rules_json.query",
            )
        pl.rules_json = payload.rules_json
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
    if pl.source == "manual":
        items = playlists_domain.list_manual_items(db, pl)
        rows = [(it.position, it.beets_id) for it in items]
        return _enrich_tracks(db, pl, rows)
    smart_tracks = playlists_domain.resolve_smart_tracks(pl)
    rows = [(idx, t.beets_id) for idx, t in enumerate(smart_tracks)]
    return _enrich_tracks(db, pl, rows)


@router.post(
    "/{playlist_id}/tracks", response_model=TrackInPlaylist, status_code=status.HTTP_201_CREATED
)
def add_track(
    playlist_id: int, payload: TrackAddRequest, _: CurrentUser, db: DbSession
) -> TrackInPlaylist:
    pl = _get_playlist(db, playlist_id)
    if beets_adapter.get_track(payload.beets_id) is None:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="track not in library"
        )
    try:
        item = playlists_domain.add_track(db, pl, payload.beets_id, payload.position)
    except playlists_domain.NotManual as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e
    except playlists_domain.PositionOutOfRange as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e

    enriched = _enrich_tracks(db, pl, [(item.position, item.beets_id)])
    return enriched[0]


@router.delete("/{playlist_id}/tracks/{position}", status_code=status.HTTP_204_NO_CONTENT)
def remove_track(
    playlist_id: int, position: int, _: CurrentUser, db: DbSession
) -> None:
    pl = _get_playlist(db, playlist_id)
    try:
        playlists_domain.remove_track(db, pl, position)
    except playlists_domain.NotManual as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e
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
    except playlists_domain.NotManual as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e
    except playlists_domain.PositionOutOfRange as e:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(e)) from e
