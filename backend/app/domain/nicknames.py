"""Resolve a track's display name through the precedence chain.

Order, most specific first:
    playlist nickname  >  mode nickname  >  global nickname  >  Beets title

The single-track ``resolve_name`` is the canonical contract — every UI that
shows a track name should resolve through it. ``resolve_names`` is the bulk
form for rendering lists; it issues at most one query per scope plus one
Beets lookup per still-unresolved id.

Returns ``None`` when neither nicknames nor Beets know the id (orphaned
reference — the caller decides what to show).
"""
from __future__ import annotations

from sqlalchemy import select
from sqlalchemy.orm import Session

from app.library import beets_adapter
from app.models.nickname import GlobalNickname, ModeNickname, PlaylistNickname


def resolve_name(
    db: Session,
    beets_id: int,
    *,
    mode_id: str | None = None,
    playlist_id: int | None = None,
) -> str | None:
    if playlist_id is not None:
        row = db.get(PlaylistNickname, (beets_id, playlist_id))
        if row is not None:
            return row.display_name
    if mode_id is not None:
        row = db.get(ModeNickname, (beets_id, mode_id))
        if row is not None:
            return row.display_name
    row = db.get(GlobalNickname, beets_id)
    if row is not None:
        return row.display_name
    track = beets_adapter.get_track(beets_id)
    return track.title if track is not None else None


def resolve_names(
    db: Session,
    beets_ids: list[int],
    *,
    mode_id: str | None = None,
    playlist_id: int | None = None,
) -> dict[int, str | None]:
    if not beets_ids:
        return {}

    result: dict[int, str | None] = {}
    remaining: set[int] = set(beets_ids)

    if playlist_id is not None and remaining:
        rows = db.scalars(
            select(PlaylistNickname).where(
                PlaylistNickname.playlist_id == playlist_id,
                PlaylistNickname.beets_id.in_(remaining),
            )
        ).all()
        for row in rows:
            result[row.beets_id] = row.display_name
            remaining.discard(row.beets_id)

    if mode_id is not None and remaining:
        rows = db.scalars(
            select(ModeNickname).where(
                ModeNickname.mode_id == mode_id,
                ModeNickname.beets_id.in_(remaining),
            )
        ).all()
        for row in rows:
            result[row.beets_id] = row.display_name
            remaining.discard(row.beets_id)

    if remaining:
        rows = db.scalars(
            select(GlobalNickname).where(GlobalNickname.beets_id.in_(remaining))
        ).all()
        for row in rows:
            result[row.beets_id] = row.display_name
            remaining.discard(row.beets_id)

    for beets_id in remaining:
        track = beets_adapter.get_track(beets_id)
        result[beets_id] = track.title if track is not None else None

    return result
