from __future__ import annotations

import threading
import unicodedata
from collections.abc import Mapping, Sequence
from dataclasses import dataclass

from sqlalchemy import func, select
from sqlalchemy.orm import Session

from app.models.track import Track
from app.models.track_user_tag import TrackUserTag

MAX_TAGS_PER_TRACK = 32
MAX_TAG_LENGTH = 64
_tag_write_lock = threading.Lock()


@dataclass(frozen=True)
class StarterTagGroup:
    key: str
    label: str
    tags: tuple[str, ...]


@dataclass(frozen=True)
class TagUsage:
    tag: str
    track_count: int


@dataclass(frozen=True)
class BulkTagFailure:
    track_id: int
    error: str


@dataclass(frozen=True)
class BulkTagOutcome:
    requested_tracks: int
    matched_tracks: int
    changed_track_ids: tuple[int, ...]
    missing_track_ids: tuple[int, ...]
    failures: tuple[BulkTagFailure, ...]


@dataclass(frozen=True)
class RenameTagOutcome:
    source: str
    target: str
    affected_tracks: int
    merged: bool


DND_STARTER_TAG_GROUPS: tuple[StarterTagGroup, ...] = (
    StarterTagGroup(
        "setting",
        "Setting",
        (
            "medieval",
            "tavern",
            "dungeon",
            "castle",
            "village",
            "forest",
            "wilderness",
            "temple",
            "ruins",
            "seafaring",
        ),
    ),
    StarterTagGroup(
        "scene",
        "Scene",
        (
            "dancing",
            "feast",
            "travel",
            "exploration",
            "combat",
            "stealth",
            "investigation",
            "rest",
        ),
    ),
    StarterTagGroup(
        "mood",
        "Mood",
        (
            "festive",
            "heroic",
            "mysterious",
            "tense",
            "dark",
            "calm",
            "eerie",
            "melancholy",
            "romantic",
        ),
    ),
)


class TagLimitError(ValueError):
    pass


class TagNotFoundError(ValueError):
    pass


def normalize_manual_tag(value: str) -> str:
    """Return the canonical comparison/storage form for a manual tag."""

    normalized = " ".join(unicodedata.normalize("NFKC", value).split()).casefold()
    if not normalized:
        raise ValueError("tags cannot be blank")
    if len(normalized) > MAX_TAG_LENGTH:
        raise ValueError(f"tags cannot exceed {MAX_TAG_LENGTH} characters")
    if any(unicodedata.category(char).startswith("C") for char in normalized):
        raise ValueError("tags cannot contain control characters")
    return normalized


def normalize_manual_tags(values: Sequence[str]) -> tuple[str, ...]:
    normalized = tuple(dict.fromkeys(normalize_manual_tag(value) for value in values))
    if len(normalized) > MAX_TAGS_PER_TRACK:
        raise TagLimitError(
            f"a track cannot have more than {MAX_TAGS_PER_TRACK} manual tags"
        )
    return normalized


def load_manual_tags(
    db: Session,
    track_ids: Sequence[int],
) -> Mapping[int, tuple[str, ...]]:
    if not track_ids:
        return {}
    rows = db.execute(
        select(TrackUserTag.track_id, TrackUserTag.tag)
        .where(TrackUserTag.track_id.in_(track_ids))
        .order_by(TrackUserTag.track_id, TrackUserTag.tag)
    ).all()
    grouped: dict[int, list[str]] = {}
    for track_id, tag in rows:
        grouped.setdefault(track_id, []).append(tag)
    return {track_id: tuple(tags) for track_id, tags in grouped.items()}


def manual_tag_usage(db: Session) -> tuple[TagUsage, ...]:
    rows = db.execute(
        select(TrackUserTag.tag, func.count(TrackUserTag.track_id))
        .group_by(TrackUserTag.tag)
        .order_by(TrackUserTag.tag)
    ).all()
    return tuple(TagUsage(tag=tag, track_count=int(count)) for tag, count in rows)


def _normalized_changes(
    add: Sequence[str],
    remove: Sequence[str],
) -> tuple[set[str], set[str]]:
    add_tags = set(normalize_manual_tags(add))
    remove_tags = set(normalize_manual_tags(remove))
    if overlap := add_tags & remove_tags:
        raise ValueError(f"tags cannot be added and removed together: {min(overlap)}")
    return add_tags, remove_tags


def _commit_or_rollback(db: Session) -> None:
    try:
        db.commit()
    except Exception:
        db.rollback()
        raise


def _apply_result(
    db: Session,
    track_id: int,
    rows: Sequence[TrackUserTag],
    result: set[str],
) -> None:
    current = {row.tag for row in rows}
    for row in rows:
        if row.tag not in result:
            db.delete(row)
    for tag in result - current:
        db.add(TrackUserTag(track_id=track_id, tag=tag))


def patch_manual_tags_bulk(
    db: Session,
    track_ids: Sequence[int],
    *,
    add: Sequence[str],
    remove: Sequence[str],
) -> BulkTagOutcome:
    """Apply one tag delta to many tracks and report every skipped target."""

    requested = tuple(dict.fromkeys(track_ids))
    add_tags, remove_tags = _normalized_changes(add, remove)
    with _tag_write_lock:
        existing_ids = set(
            db.scalars(select(Track.id).where(Track.id.in_(requested))).all()
        )
        rows = list(
            db.scalars(
                select(TrackUserTag).where(TrackUserTag.track_id.in_(existing_ids))
            ).all()
        )
        rows_by_track: dict[int, list[TrackUserTag]] = {}
        for row in rows:
            rows_by_track.setdefault(row.track_id, []).append(row)

        changed: list[int] = []
        failures: list[BulkTagFailure] = []
        for track_id in sorted(existing_ids):
            track_rows = rows_by_track.get(track_id, [])
            current = {row.tag for row in track_rows}
            result = (current - remove_tags) | add_tags
            if len(result) > MAX_TAGS_PER_TRACK:
                failures.append(
                    BulkTagFailure(
                        track_id=track_id,
                        error=(
                            f"track would exceed the {MAX_TAGS_PER_TRACK}-tag limit"
                        ),
                    )
                )
                continue
            if result != current:
                _apply_result(db, track_id, track_rows, result)
                changed.append(track_id)
        _commit_or_rollback(db)

    missing = tuple(sorted(set(requested) - existing_ids))
    return BulkTagOutcome(
        requested_tracks=len(requested),
        matched_tracks=len(existing_ids),
        changed_track_ids=tuple(changed),
        missing_track_ids=missing,
        failures=tuple(failures),
    )


def patch_manual_tags(
    db: Session,
    track_id: int,
    *,
    add: Sequence[str],
    remove: Sequence[str],
) -> tuple[str, ...]:
    """Apply an idempotent delta so concurrent additions are not overwritten."""

    add_tags, remove_tags = _normalized_changes(add, remove)
    with _tag_write_lock:
        rows = list(
            db.scalars(
                select(TrackUserTag).where(TrackUserTag.track_id == track_id)
            ).all()
        )
        current = {row.tag for row in rows}
        result = (current - remove_tags) | add_tags
        if len(result) > MAX_TAGS_PER_TRACK:
            raise TagLimitError(
                f"a track cannot have more than {MAX_TAGS_PER_TRACK} manual tags"
            )

        _apply_result(db, track_id, rows, result)
        _commit_or_rollback(db)
    return tuple(sorted(result))


def rename_manual_tag(db: Session, source: str, target: str) -> RenameTagOutcome:
    """Rename a library-wide tag, merging duplicate per-track rows atomically."""

    source_tag = normalize_manual_tag(source)
    target_tag = normalize_manual_tag(target)
    if source_tag == target_tag:
        raise ValueError("source and target tags must be different")

    with _tag_write_lock:
        source_rows = list(
            db.scalars(
                select(TrackUserTag).where(TrackUserTag.tag == source_tag)
            ).all()
        )
        if not source_rows:
            raise TagNotFoundError(f"manual tag not found: {source_tag}")
        target_track_ids = set(
            db.scalars(
                select(TrackUserTag.track_id).where(TrackUserTag.tag == target_tag)
            ).all()
        )
        for row in source_rows:
            db.delete(row)
            if row.track_id not in target_track_ids:
                db.add(TrackUserTag(track_id=row.track_id, tag=target_tag))
        _commit_or_rollback(db)
    return RenameTagOutcome(
        source=source_tag,
        target=target_tag,
        affected_tracks=len(source_rows),
        merged=bool(target_track_ids),
    )
