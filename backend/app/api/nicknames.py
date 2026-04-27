from fastapi import APIRouter, HTTPException, status
from pydantic import BaseModel, Field
from sqlalchemy import select

from app.api.deps import CurrentUser, DbSession
from app.library import beets_adapter
from app.models.nickname import GlobalNickname, ModeNickname, PlaylistNickname
from app.models.playlist import Playlist
from app.modes import loader as modes_loader

router = APIRouter(prefix="/api/nicknames", tags=["nicknames"])


class NicknamePayload(BaseModel):
    display_name: str = Field(min_length=1, max_length=512)


class ScopedNickname(BaseModel):
    scope_id: str | int
    display_name: str


class NicknameOverview(BaseModel):
    beets_id: int
    title: str
    global_: str | None = Field(None, alias="global")
    modes: list[ScopedNickname]
    playlists: list[ScopedNickname]

    model_config = {"populate_by_name": True}


def _ensure_track_exists(beets_id: int) -> None:
    if beets_adapter.get_track(beets_id) is None:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="track not in library"
        )


def _ensure_mode_exists(mode_id: str) -> None:
    if modes_loader.get_mode(mode_id) is None:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST, detail=f"unknown mode: {mode_id}"
        )


def _ensure_playlist_exists(db: DbSession, playlist_id: int) -> None:
    if db.get(Playlist, playlist_id) is None:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="playlist not found"
        )


@router.get("/{beets_id}", response_model=NicknameOverview)
def get_overview(beets_id: int, _: CurrentUser, db: DbSession) -> NicknameOverview:
    track = beets_adapter.get_track(beets_id)
    if track is None:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="track not in library"
        )

    glob = db.get(GlobalNickname, beets_id)
    modes = db.scalars(
        select(ModeNickname).where(ModeNickname.beets_id == beets_id)
    ).all()
    playlists = db.scalars(
        select(PlaylistNickname).where(PlaylistNickname.beets_id == beets_id)
    ).all()

    return NicknameOverview(
        beets_id=beets_id,
        title=track.title,
        **{"global": glob.display_name if glob else None},
        modes=[ScopedNickname(scope_id=m.mode_id, display_name=m.display_name) for m in modes],
        playlists=[
            ScopedNickname(scope_id=p.playlist_id, display_name=p.display_name)
            for p in playlists
        ],
    )


@router.put("/{beets_id}/global", status_code=status.HTTP_204_NO_CONTENT)
def set_global(
    beets_id: int, payload: NicknamePayload, _: CurrentUser, db: DbSession
) -> None:
    _ensure_track_exists(beets_id)
    existing = db.get(GlobalNickname, beets_id)
    if existing is None:
        db.add(GlobalNickname(beets_id=beets_id, display_name=payload.display_name))
    else:
        existing.display_name = payload.display_name
    db.commit()


@router.delete("/{beets_id}/global", status_code=status.HTTP_204_NO_CONTENT)
def delete_global(beets_id: int, _: CurrentUser, db: DbSession) -> None:
    existing = db.get(GlobalNickname, beets_id)
    if existing is not None:
        db.delete(existing)
        db.commit()


@router.put("/{beets_id}/modes/{mode_id}", status_code=status.HTTP_204_NO_CONTENT)
def set_mode(
    beets_id: int, mode_id: str, payload: NicknamePayload, _: CurrentUser, db: DbSession
) -> None:
    _ensure_track_exists(beets_id)
    _ensure_mode_exists(mode_id)
    existing = db.get(ModeNickname, (beets_id, mode_id))
    if existing is None:
        db.add(ModeNickname(beets_id=beets_id, mode_id=mode_id, display_name=payload.display_name))
    else:
        existing.display_name = payload.display_name
    db.commit()


@router.delete("/{beets_id}/modes/{mode_id}", status_code=status.HTTP_204_NO_CONTENT)
def delete_mode(beets_id: int, mode_id: str, _: CurrentUser, db: DbSession) -> None:
    existing = db.get(ModeNickname, (beets_id, mode_id))
    if existing is not None:
        db.delete(existing)
        db.commit()


@router.put("/{beets_id}/playlists/{playlist_id}", status_code=status.HTTP_204_NO_CONTENT)
def set_playlist(
    beets_id: int,
    playlist_id: int,
    payload: NicknamePayload,
    _: CurrentUser,
    db: DbSession,
) -> None:
    _ensure_track_exists(beets_id)
    _ensure_playlist_exists(db, playlist_id)
    existing = db.get(PlaylistNickname, (beets_id, playlist_id))
    if existing is None:
        db.add(
            PlaylistNickname(
                beets_id=beets_id, playlist_id=playlist_id, display_name=payload.display_name
            )
        )
    else:
        existing.display_name = payload.display_name
    db.commit()


@router.delete("/{beets_id}/playlists/{playlist_id}", status_code=status.HTTP_204_NO_CONTENT)
def delete_playlist(
    beets_id: int, playlist_id: int, _: CurrentUser, db: DbSession
) -> None:
    existing = db.get(PlaylistNickname, (beets_id, playlist_id))
    if existing is not None:
        db.delete(existing)
        db.commit()
