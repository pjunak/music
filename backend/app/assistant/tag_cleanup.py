"""Conservative, review-only cleanup suggestions for operator-owned tags."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Sequence
from dataclasses import dataclass
from typing import Literal

from sqlalchemy import func, select
from sqlalchemy.orm import Session

from app.assistant.tag_lock import tag_write_lock
from app.assistant.tag_vocabulary import (
    TagVocabularySnapshot,
    default_tag_vocabulary_snapshot,
    load_tag_vocabulary,
)
from app.assistant.tags import (
    RenameTagOutcome,
    TagUsage,
    normalize_manual_tag,
)
from app.models.track_user_tag import TrackUserTag

TAG_CLEANUP_SCHEMA_VERSION: Literal["assistant-tag-cleanup-preview/v2"] = (
    "assistant-tag-cleanup-preview/v2"
)


@dataclass(frozen=True)
class TagCleanupSuggestion:
    id: str
    source: str
    target: str
    reason_code: Literal[
        "vocabulary_alias",
        "vocabulary_plural",
        "vocabulary_typo",
    ]
    reason: str
    source_track_count: int
    target_track_count: int
    merged: bool


@dataclass(frozen=True)
class TagCleanupPreview:
    catalog_signature: str
    vocabulary_fingerprint: str
    suggestions: tuple[TagCleanupSuggestion, ...]


@dataclass(frozen=True)
class TagCleanupSelection:
    source: str
    target: str


@dataclass(frozen=True)
class TagCleanupApplyOutcome:
    applied: tuple[RenameTagOutcome, ...]
    catalog_signature: str


@dataclass(frozen=True)
class TagCatalogSnapshot:
    usage: tuple[TagUsage, ...]
    signature: str


class StaleTagCleanupError(ValueError):
    pass


class InvalidTagCleanupSelectionError(ValueError):
    pass


def _is_single_edit(source: str, target: str) -> bool:
    """Return true for one insertion, deletion, replacement, or adjacent swap."""

    if source == target or abs(len(source) - len(target)) > 1:
        return False
    if len(source) == len(target):
        mismatches = [
            index
            for index, (left, right) in enumerate(zip(source, target, strict=True))
            if left != right
        ]
        if len(mismatches) == 1:
            return True
        return (
            len(mismatches) == 2
            and mismatches[1] == mismatches[0] + 1
            and source[mismatches[0]] == target[mismatches[1]]
            and source[mismatches[1]] == target[mismatches[0]]
        )

    shorter, longer = (source, target) if len(source) < len(target) else (target, source)
    short_index = 0
    long_index = 0
    skipped = False
    while short_index < len(shorter) and long_index < len(longer):
        if shorter[short_index] == longer[long_index]:
            short_index += 1
            long_index += 1
            continue
        if skipped:
            return False
        skipped = True
        long_index += 1
    return True


def catalog_signature(usage: Sequence[TagUsage]) -> str:
    payload = {
        "schema": TAG_CLEANUP_SCHEMA_VERSION,
        "usage": [[item.tag, item.track_count] for item in usage],
    }
    encoded = json.dumps(payload, separators=(",", ":"), ensure_ascii=True).encode()
    return hashlib.sha256(encoded).hexdigest()


def _suggestion_id(
    source: str,
    target: str,
    reason_code: str,
    vocabulary_fingerprint: str,
) -> str:
    return hashlib.sha256(
        (
            f"{TAG_CLEANUP_SCHEMA_VERSION}\0{vocabulary_fingerprint}\0"
            f"{source}\0{target}\0{reason_code}"
        ).encode()
    ).hexdigest()


def build_tag_cleanup_preview(
    usage: Sequence[TagUsage],
    vocabulary: TagVocabularySnapshot | None = None,
) -> TagCleanupPreview:
    """Propose only explicit aliases and unambiguous canonical-name fixes."""

    vocabulary = vocabulary or default_tag_vocabulary_snapshot()
    counts = {item.tag: item.track_count for item in usage}
    canonical_tags = tuple(tag.name for tag in vocabulary.entries)
    canonical_set = set(canonical_tags)
    aliases = vocabulary.aliases
    suggestions: list[TagCleanupSuggestion] = []
    for source in sorted(counts):
        if source in canonical_set:
            continue

        alias_target = aliases.get(source)
        plural_target = source[:-1] if source.endswith("s") else ""
        reason_code: Literal[
            "vocabulary_alias",
            "vocabulary_plural",
            "vocabulary_typo",
        ]
        candidates: tuple[str, ...]
        if alias_target is not None:
            candidates = (alias_target.name,)
            reason_code = "vocabulary_alias"
            reason = "Matches an alias defined in the controlled vocabulary."
        elif plural_target in canonical_set:
            candidates = (plural_target,)
            reason_code = "vocabulary_plural"
            reason = "Matches the plural form of a canonical tag."
        else:
            candidates = tuple(
                target for target in canonical_tags if _is_single_edit(source, target)
            )
            reason_code = "vocabulary_typo"
            reason = "One clear spelling edit from a canonical tag."

        if len(candidates) != 1:
            continue
        target = candidates[0]
        target_count = counts.get(target, 0)
        suggestions.append(
            TagCleanupSuggestion(
                id=_suggestion_id(
                    source,
                    target,
                    reason_code,
                    vocabulary.fingerprint,
                ),
                source=source,
                target=target,
                reason_code=reason_code,
                reason=reason,
                source_track_count=counts[source],
                target_track_count=target_count,
                merged=target_count > 0,
            )
        )
    return TagCleanupPreview(
        catalog_signature=catalog_signature(usage),
        vocabulary_fingerprint=vocabulary.fingerprint,
        suggestions=tuple(suggestions),
    )


def _manual_tag_usage_unlocked(db: Session) -> tuple[TagUsage, ...]:
    rows = db.execute(
        select(TrackUserTag.tag, func.count(TrackUserTag.track_id))
        .group_by(TrackUserTag.tag)
        .order_by(TrackUserTag.tag)
    ).all()
    return tuple(TagUsage(tag=tag, track_count=int(count)) for tag, count in rows)


def preview_tag_cleanup(db: Session) -> TagCleanupPreview:
    return build_tag_cleanup_preview(
        tag_catalog_snapshot(db).usage,
        load_tag_vocabulary(db),
    )


def tag_catalog_snapshot(db: Session) -> TagCatalogSnapshot:
    usage = _manual_tag_usage_unlocked(db)
    return TagCatalogSnapshot(usage=usage, signature=catalog_signature(usage))


def _normalize_selections(
    selections: Sequence[TagCleanupSelection],
) -> tuple[TagCleanupSelection, ...]:
    normalized = tuple(
        TagCleanupSelection(
            source=normalize_manual_tag(item.source),
            target=normalize_manual_tag(item.target),
        )
        for item in selections
    )
    sources = {item.source for item in normalized}
    targets = {item.target for item in normalized}
    if len(sources) != len(normalized):
        raise InvalidTagCleanupSelectionError("cleanup sources must be unique")
    if sources & targets:
        raise InvalidTagCleanupSelectionError(
            "cleanup selections cannot depend on another selected rename"
        )
    return normalized


def _apply_reviewed_tag_renames_locked(
    db: Session,
    expected_signature: str,
    current_usage: Sequence[TagUsage],
    selections: tuple[TagCleanupSelection, ...],
    allowed_pairs: set[tuple[str, str]],
) -> TagCleanupApplyOutcome:
    current_signature = catalog_signature(current_usage)
    if current_signature != expected_signature:
        raise StaleTagCleanupError(
            "manual tags changed after this cleanup preview was created"
        )
    if invalid := [
        item
        for item in selections
        if (item.source, item.target) not in allowed_pairs
    ]:
        first = invalid[0]
        raise InvalidTagCleanupSelectionError(
            f"cleanup suggestion is not current: {first.source} -> {first.target}"
        )

    sources = {item.source for item in selections}
    targets = {item.target for item in selections}
    source_rows = list(
        db.scalars(select(TrackUserTag).where(TrackUserTag.tag.in_(sources))).all()
    )
    rows_by_source: dict[str, list[TrackUserTag]] = {}
    for row in source_rows:
        rows_by_source.setdefault(row.tag, []).append(row)
    if missing := sorted(sources - rows_by_source.keys()):
        raise StaleTagCleanupError(f"manual tag no longer exists: {missing[0]}")
    existing_targets = {
        (int(track_id), str(tag))
        for track_id, tag in db.execute(
            select(TrackUserTag.track_id, TrackUserTag.tag).where(
                TrackUserTag.tag.in_(targets)
            )
        ).all()
    }

    outcomes: list[RenameTagOutcome] = []
    for item in selections:
        rows = rows_by_source[item.source]
        target_existed = any(tag == item.target for _, tag in existing_targets)
        for row in rows:
            db.delete(row)
            pair = (row.track_id, item.target)
            if pair not in existing_targets:
                db.add(TrackUserTag(track_id=row.track_id, tag=item.target))
                existing_targets.add(pair)
        outcomes.append(
            RenameTagOutcome(
                source=item.source,
                target=item.target,
                affected_tracks=len(rows),
                merged=target_existed,
            )
        )
    try:
        db.commit()
    except Exception:
        db.rollback()
        raise

    next_signature = catalog_signature(_manual_tag_usage_unlocked(db))
    return TagCleanupApplyOutcome(
        applied=tuple(outcomes),
        catalog_signature=next_signature,
    )


def apply_reviewed_tag_renames(
    db: Session,
    expected_signature: str,
    selections: Sequence[TagCleanupSelection],
    *,
    allowed_pairs: set[tuple[str, str]],
) -> TagCleanupApplyOutcome:
    """Atomically apply an explicitly reviewed subset of a bound proposal."""

    normalized = _normalize_selections(selections)
    with tag_write_lock:
        return _apply_reviewed_tag_renames_locked(
            db,
            expected_signature,
            _manual_tag_usage_unlocked(db),
            normalized,
            allowed_pairs,
        )


def apply_tag_cleanup(
    db: Session,
    expected_signature: str,
    expected_vocabulary_fingerprint: str,
    selections: Sequence[TagCleanupSelection],
) -> TagCleanupApplyOutcome:
    """Apply explicitly selected current suggestions in one transaction."""

    normalized = _normalize_selections(selections)
    with tag_write_lock:
        current_usage = _manual_tag_usage_unlocked(db)
        vocabulary = load_tag_vocabulary(db)
        if vocabulary.fingerprint != expected_vocabulary_fingerprint:
            raise StaleTagCleanupError(
                "tag vocabulary changed after this cleanup preview was created"
            )
        current = build_tag_cleanup_preview(current_usage, vocabulary)
        allowed_pairs = {
            (suggestion.source, suggestion.target)
            for suggestion in current.suggestions
        }
        return _apply_reviewed_tag_renames_locked(
            db,
            expected_signature,
            current_usage=current_usage,
            selections=normalized,
            allowed_pairs=allowed_pairs,
        )
