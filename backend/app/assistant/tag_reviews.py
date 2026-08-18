from __future__ import annotations

import json
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Literal, cast

from sqlalchemy import select
from sqlalchemy.orm import Session

from app.assistant.analysis import (
    LOCAL_METADATA_ANALYZER_ID,
    track_source_signature,
)
from app.assistant.tag_lock import tag_write_lock
from app.assistant.tags import MAX_TAGS_PER_TRACK, TagLimitError, normalize_manual_tag
from app.models.base import utcnow
from app.models.track import Track
from app.models.track_analysis import TrackAnalysis
from app.models.track_analysis_tag_review import TrackAnalysisTagReview
from app.models.track_user_tag import TrackUserTag

ReviewStatus = Literal["pending", "accepted", "rejected"]


@dataclass(frozen=True)
class AnalysisTagSuggestion:
    tag: str
    analyzer_id: str
    source_signature: str
    confidence: Literal["high", "medium", "low"]
    evidence: tuple[str, ...]
    status: ReviewStatus


@dataclass(frozen=True)
class AnalysisTagReviewOutcome:
    track_id: int
    tag: str
    analyzer_id: str
    source_signature: str
    decision: ReviewStatus
    manual_tags: tuple[str, ...]


class AnalysisSuggestionNotFoundError(ValueError):
    pass


class StaleAnalysisSuggestionError(ValueError):
    pass


def _string_tuple(value: str) -> tuple[str, ...] | None:
    try:
        parsed = json.loads(value)
    except json.JSONDecodeError:
        return None
    if not isinstance(parsed, list) or not all(isinstance(item, str) for item in parsed):
        return None
    return tuple(parsed)


def _profile_tags(row: TrackAnalysis) -> tuple[str, ...] | None:
    raw_tags = _string_tuple(row.moods_json)
    if raw_tags is None:
        return None
    normalized: list[str] = []
    for value in raw_tags:
        try:
            tag = normalize_manual_tag(value)
        except ValueError:
            continue
        if tag not in normalized:
            normalized.append(tag)
    return tuple(normalized)


def load_current_analysis_tag_suggestions(
    db: Session,
    tracks: Sequence[Track],
) -> Mapping[int, tuple[AnalysisTagSuggestion, ...]]:
    """Load reviewable tags from current, valid local analysis profiles.

    The returned contract already carries an analyzer ID and source signature,
    so later analyzers can join this surface after defining their own current-
    profile validation without changing review semantics.
    """

    track_by_id = {track.id: track for track in tracks}
    if not track_by_id:
        return {}
    rows = list(
        db.scalars(
            select(TrackAnalysis).where(
                TrackAnalysis.analyzer_id == LOCAL_METADATA_ANALYZER_ID,
                TrackAnalysis.track_id.in_(track_by_id),
            )
        ).all()
    )
    reviews = {
        (review.track_id, review.analyzer_id, review.tag): review
        for review in db.scalars(
            select(TrackAnalysisTagReview).where(
                TrackAnalysisTagReview.track_id.in_(track_by_id),
                TrackAnalysisTagReview.analyzer_id == LOCAL_METADATA_ANALYZER_ID,
            )
        ).all()
    }
    suggestions: dict[int, tuple[AnalysisTagSuggestion, ...]] = {}
    for row in rows:
        track = track_by_id.get(row.track_id)
        if track is None or row.source_signature != track_source_signature(track):
            continue
        tags = _profile_tags(row)
        evidence = _string_tuple(row.evidence_json)
        if tags is None or evidence is None:
            continue
        if row.confidence not in {"high", "medium", "low"}:
            continue
        confidence = cast(
            "Literal['high', 'medium', 'low']",
            row.confidence,
        )
        track_suggestions: list[AnalysisTagSuggestion] = []
        for tag in tags:
            review = reviews.get((row.track_id, row.analyzer_id, tag))
            status: ReviewStatus = "pending"
            if (
                review is not None
                and review.source_signature == row.source_signature
                and review.decision in {"accepted", "rejected"}
            ):
                status = cast("ReviewStatus", review.decision)
            track_suggestions.append(
                AnalysisTagSuggestion(
                    tag=tag,
                    analyzer_id=row.analyzer_id,
                    source_signature=row.source_signature,
                    confidence=confidence,
                    evidence=evidence,
                    status=status,
                )
            )
        suggestions[row.track_id] = tuple(track_suggestions)
    return suggestions


def review_analysis_tag(
    db: Session,
    track_id: int,
    *,
    analyzer_id: str,
    source_signature: str,
    tag: str,
    decision: ReviewStatus,
) -> AnalysisTagReviewOutcome:
    """Persist one explicit decision and atomically promote accepted tags."""

    normalized_tag = normalize_manual_tag(tag)
    if decision not in {"pending", "accepted", "rejected"}:
        raise ValueError("invalid analysis tag review decision")

    with tag_write_lock:
        track = db.get(Track, track_id)
        if track is None:
            raise AnalysisSuggestionNotFoundError("Track not found")
        row = db.get(TrackAnalysis, (track_id, analyzer_id))
        if row is None or analyzer_id != LOCAL_METADATA_ANALYZER_ID:
            raise AnalysisSuggestionNotFoundError("Analysis profile not found")
        if row.source_signature != source_signature:
            raise StaleAnalysisSuggestionError(
                "Analysis changed; refresh before reviewing this tag"
            )
        if row.source_signature != track_source_signature(track):
            raise StaleAnalysisSuggestionError(
                "Track metadata changed; rerun analysis before reviewing this tag"
            )
        profile_tags = _profile_tags(row)
        if profile_tags is None or normalized_tag not in profile_tags:
            raise AnalysisSuggestionNotFoundError(
                "Tag is not present in the current analysis profile"
            )

        manual_rows = list(
            db.scalars(
                select(TrackUserTag).where(TrackUserTag.track_id == track_id)
            ).all()
        )
        manual_tags = {manual.tag for manual in manual_rows}
        review = db.get(
            TrackAnalysisTagReview,
            (track_id, analyzer_id, normalized_tag),
        )
        try:
            if decision == "pending":
                if review is not None:
                    db.delete(review)
            else:
                if decision == "accepted" and normalized_tag not in manual_tags:
                    if len(manual_tags) >= MAX_TAGS_PER_TRACK:
                        raise TagLimitError(
                            f"a track cannot have more than {MAX_TAGS_PER_TRACK} manual tags"
                        )
                    db.add(TrackUserTag(track_id=track_id, tag=normalized_tag))
                    manual_tags.add(normalized_tag)
                if review is None:
                    review = TrackAnalysisTagReview(
                        track_id=track_id,
                        analyzer_id=analyzer_id,
                        tag=normalized_tag,
                    )
                    db.add(review)
                review.source_signature = source_signature
                review.decision = decision
                review.reviewed_at = utcnow()
            db.commit()
        except Exception:
            db.rollback()
            raise

    return AnalysisTagReviewOutcome(
        track_id=track_id,
        tag=normalized_tag,
        analyzer_id=analyzer_id,
        source_signature=source_signature,
        decision=decision,
        manual_tags=tuple(sorted(manual_tags)),
    )
