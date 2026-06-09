"""Playlist domain logic: position arithmetic, manual mutators.

Playlists are now manual-only — see docs/FUTURE.md for the smart-playlist
plan. The position invariant: items occupy contiguous, 0-indexed integer
positions. Insert/delete/move all maintain that.

The DB can still punch holes in it behind our back: ``PlaylistItem.track_id``
is ``ondelete=CASCADE`` with SQLite FK enforcement on, so the library indexer
deleting a vanished Track drops its items at the SQLite level, bypassing the
shift logic and leaving gaps (e.g. [0, 2, 3]). So every read/write first
``_repack``s the playlist back to contiguous before trusting the positions.

The shifts use a two-pass negation pattern so SQLite's PK uniqueness
constraint on (playlist_id, position) doesn't blow up mid-update —
positions are first flipped negative (always unique within a playlist),
then flipped back to positive after the slot is free.
"""
from __future__ import annotations

from sqlalchemy import func, select, update
from sqlalchemy.orm import Session

from app.models.playlist import Playlist, PlaylistItem

# Parks the moved item far outside the negated-real-position range so the
# flip-back pass can exclude it without colliding with any shifted row.
_PARKED_POSITION = -1_000_000


class PlaylistError(Exception):
    """Domain-level error — API layer maps to appropriate HTTP status."""


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


def _repack(db: Session, playlist_id: int) -> bool:
    """Heal a playlist's item positions back to contiguous 0..N-1, in order.

    The position arithmetic below assumes contiguity, but a CASCADE delete of a
    referenced Track row (see module docstring) can leave gaps that no shift
    helper ever saw. Re-pack before trusting the positions. Returns True when it
    actually moved a row, so a read caller can decide whether to persist the heal.

    Same two-pass negation as the shift helpers so the (playlist_id, position)
    PK stays unique mid-update.
    """
    items = list(
        db.scalars(
            select(PlaylistItem)
            .where(PlaylistItem.playlist_id == playlist_id)
            .order_by(PlaylistItem.position)
        ).all()
    )
    if all(it.position == i for i, it in enumerate(items)):
        return False
    for i, it in enumerate(items):
        it.position = -(i + 1)
    db.flush()
    for it in items:
        it.position = -it.position - 1
    db.flush()
    return True


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
    db: Session, playlist: Playlist, track_id: int, position: int | None = None
) -> PlaylistItem:
    _repack(db, playlist.id)
    length = _length(db, playlist.id)
    target = length if position is None else position
    if target < 0 or target > length:
        raise PositionOutOfRange(f"position must be in [0, {length}]")

    if target < length:
        _shift_up(db, playlist.id, target)

    item = PlaylistItem(playlist_id=playlist.id, position=target, track_id=track_id)
    db.add(item)
    db.commit()
    db.refresh(item)
    return item


def remove_track(db: Session, playlist: Playlist, position: int) -> None:
    _repack(db, playlist.id)
    item = db.get(PlaylistItem, (playlist.id, position))
    if item is None:
        raise PositionOutOfRange(f"no item at position {position}")

    db.delete(item)
    db.flush()
    _shift_down(db, playlist.id, position)
    db.commit()


def move_track(db: Session, playlist: Playlist, from_pos: int, to_pos: int) -> None:
    if from_pos == to_pos:
        return

    _repack(db, playlist.id)
    length = _length(db, playlist.id)
    if from_pos < 0 or from_pos >= length:
        raise PositionOutOfRange(f"from_position {from_pos} out of range")
    if to_pos < 0 or to_pos >= length:
        raise PositionOutOfRange(f"to_position {to_pos} out of range")

    item = db.get(PlaylistItem, (playlist.id, from_pos))
    assert item is not None  # range checked above

    # Pull the moved item out of position so the shift can free up the target slot.
    item.position = _PARKED_POSITION
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
            PlaylistItem.position != _PARKED_POSITION,
        )
        .values(position=-PlaylistItem.position)
    )
    db.flush()

    item.position = to_pos
    db.commit()


def list_items(db: Session, playlist: Playlist) -> list[PlaylistItem]:
    # Self-heal any cascade gap so callers always see contiguous positions;
    # persist it here since `get_db` doesn't commit on a read path.
    if _repack(db, playlist.id):
        db.commit()
    return list(
        db.scalars(
            select(PlaylistItem)
            .where(PlaylistItem.playlist_id == playlist.id)
            .order_by(PlaylistItem.position)
        ).all()
    )
