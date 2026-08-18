from __future__ import annotations

import unicodedata
from collections.abc import Mapping, Sequence
from dataclasses import dataclass

from sqlalchemy import select
from sqlalchemy.orm import Session

from app.models.track_user_tag import TrackUserTag

MAX_TAGS_PER_TRACK = 32
MAX_TAG_LENGTH = 64


@dataclass(frozen=True)
class StarterTagGroup:
    key: str
    label: str
    tags: tuple[str, ...]


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


def used_manual_tags(db: Session) -> tuple[str, ...]:
    return tuple(
        db.scalars(select(TrackUserTag.tag).distinct().order_by(TrackUserTag.tag)).all()
    )


def patch_manual_tags(
    db: Session,
    track_id: int,
    *,
    add: Sequence[str],
    remove: Sequence[str],
) -> tuple[str, ...]:
    """Apply an idempotent delta so concurrent additions are not overwritten."""

    add_tags = set(normalize_manual_tags(add))
    remove_tags = set(normalize_manual_tags(remove))
    if overlap := add_tags & remove_tags:
        raise ValueError(f"tags cannot be added and removed together: {min(overlap)}")

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

    for row in rows:
        if row.tag not in result:
            db.delete(row)
    for tag in result - current:
        db.add(TrackUserTag(track_id=track_id, tag=tag))
    db.commit()
    return tuple(sorted(result))
