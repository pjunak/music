"""Playlist domain logic: position arithmetic, manual mutators, smart resolution.

The position invariant: a manual playlist's items occupy contiguous,
0-indexed integer positions. Insert/delete/move all maintain that.

The shifts use a two-pass negation pattern so SQLite's PK uniqueness
constraint on (playlist_id, position) doesn't blow up mid-update —
positions are first flipped negative (always unique within a playlist),
then flipped back to positive after the slot is free.
"""
from __future__ import annotations

from sqlalchemy import func, select, update
from sqlalchemy.orm import Session

from app.library import beets_adapter
from app.library.beets_adapter import Track
from app.models.playlist import Playlist, PlaylistItem


class PlaylistError(Exception):
    """Domain-level error — API layer maps to appropriate HTTP status."""


class NotManual(PlaylistError):
    pass


class PositionOutOfRange(PlaylistError):
    pass


def _length(db: Session, playlist_id: int) -> int:
    return int(
        db.scalar(
            select(func.count()).select_from(PlaylistItem).where(
                PlaylistItem.playlist_id == playlist_id
            )
        )
        or 0
    )


def _shift_up(db: Session, playlist_id: int, from_position: int) -> None:
    """Make room at ``from_position`` by bumping every position >= it up by 1."""
    db.execute(
        update(PlaylistItem)
        .where(
            PlaylistItem.playlist_id == playlist_id,
            PlaylistItem.position >= from_position,
        )
        .values(position=-(PlaylistItem.position + 1))
    )
    db.flush()
    db.execute(
        update(PlaylistItem)
        .where(PlaylistItem.playlist_id == playlist_id, PlaylistItem.position < 0)
        .values(position=-PlaylistItem.position)
    )
    db.flush()


def _shift_down(db: Session, playlist_id: int, after_position: int) -> None:
    """Close a gap at ``after_position`` by bumping every position > it down by 1."""
    db.execute(
        update(PlaylistItem)
        .where(
            PlaylistItem.playlist_id == playlist_id,
            PlaylistItem.position > after_position,
        )
        .values(position=-(PlaylistItem.position - 1))
    )
    db.flush()
    db.execute(
        update(PlaylistItem)
        .where(PlaylistItem.playlist_id == playlist_id, PlaylistItem.position < 0)
        .values(position=-PlaylistItem.position)
    )
    db.flush()


def add_track(
    db: Session, playlist: Playlist, beets_id: int, position: int | None = None
) -> PlaylistItem:
    if playlist.source != "manual":
        raise NotManual("cannot add tracks to a smart playlist")

    length = _length(db, playlist.id)
    target = length if position is None else position
    if target < 0 or target > length:
        raise PositionOutOfRange(f"position must be in [0, {length}]")

    if target < length:
        _shift_up(db, playlist.id, target)

    item = PlaylistItem(playlist_id=playlist.id, position=target, beets_id=beets_id)
    db.add(item)
    db.commit()
    db.refresh(item)
    return item


def remove_track(db: Session, playlist: Playlist, position: int) -> None:
    if playlist.source != "manual":
        raise NotManual("cannot remove tracks from a smart playlist")

    item = db.get(PlaylistItem, (playlist.id, position))
    if item is None:
        raise PositionOutOfRange(f"no item at position {position}")

    db.delete(item)
    db.flush()
    _shift_down(db, playlist.id, position)
    db.commit()


def move_track(db: Session, playlist: Playlist, from_pos: int, to_pos: int) -> None:
    if playlist.source != "manual":
        raise NotManual("cannot move tracks in a smart playlist")
    if from_pos == to_pos:
        return

    length = _length(db, playlist.id)
    if from_pos < 0 or from_pos >= length:
        raise PositionOutOfRange(f"from_position {from_pos} out of range")
    if to_pos < 0 or to_pos >= length:
        raise PositionOutOfRange(f"to_position {to_pos} out of range")

    item = db.get(PlaylistItem, (playlist.id, from_pos))
    assert item is not None  # range checked above

    # Pull the moved item out of position so the shift can free up the target slot.
    item.position = -1_000_000
    db.flush()

    if from_pos < to_pos:
        # Items in (from_pos, to_pos] shift down by 1.
        db.execute(
            update(PlaylistItem)
            .where(
                PlaylistItem.playlist_id == playlist.id,
                PlaylistItem.position > from_pos,
                PlaylistItem.position <= to_pos,
            )
            .values(position=-(PlaylistItem.position - 1))
        )
    else:
        # Items in [to_pos, from_pos) shift up by 1.
        db.execute(
            update(PlaylistItem)
            .where(
                PlaylistItem.playlist_id == playlist.id,
                PlaylistItem.position >= to_pos,
                PlaylistItem.position < from_pos,
            )
            .values(position=-(PlaylistItem.position + 1))
        )
    db.flush()
    db.execute(
        update(PlaylistItem)
        .where(
            PlaylistItem.playlist_id == playlist.id,
            PlaylistItem.position < 0,
            PlaylistItem.position > -500_000,
        )
        .values(position=-PlaylistItem.position)
    )
    db.flush()

    item.position = to_pos
    db.commit()


def list_manual_items(db: Session, playlist: Playlist) -> list[PlaylistItem]:
    return list(
        db.scalars(
            select(PlaylistItem)
            .where(PlaylistItem.playlist_id == playlist.id)
            .order_by(PlaylistItem.position)
        ).all()
    )


def resolve_smart_tracks(playlist: Playlist) -> list[Track]:
    if playlist.source != "smart":
        raise PlaylistError("playlist is not smart")
    rules = playlist.rules_json or {}
    query = str(rules.get("query") or "")
    limit = rules.get("limit")
    return beets_adapter.search(query, limit=int(limit) if limit else None)
