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
from app.assistant.audio_analysis import (
    CurrentAudioProfile,
    load_current_audio_profiles,
)
from app.assistant.model_tagger import MODEL_TAG_ANALYZER_ID
from app.assistant.model_tagging import (
    MODEL_TAGGING_ROLE_ID,
    model_tag_source_signature,
)
from app.assistant.providers.service import current_role_runtime_fingerprint
from app.assistant.tag_lock import tag_write_lock
from app.assistant.tag_vocabulary import load_tag_vocabulary
from app.assistant.tags import MAX_TAGS_PER_TRACK, TagLimitError, normalize_manual_tag
from app.models.base import utcnow
from app.models.track import Track
from app.models.track_analysis import TrackAnalysis
from app.models.track_analysis_tag_review import TrackAnalysisTagReview
from app.models.track_user_tag import TrackUserTag

ReviewStatus = Literal["pending", "accepted", "rejected"]
ReviewFailureCode = Literal["not_found", "stale", "tag_limit"]
_REVIEWABLE_ANALYZERS = (LOCAL_METADATA_ANALYZER_ID, MODEL_TAG_ANALYZER_ID)


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


@dataclass(frozen=True)
class AnalysisTagReviewTarget:
    track_id: int
    tag: str
    analyzer_id: str
    source_signature: str


@dataclass(frozen=True)
class AnalysisTagReviewApplied:
    track_id: int
    tag: str
    analyzer_id: str
    source_signature: str
    decision: ReviewStatus


@dataclass(frozen=True)
class AnalysisTagReviewFailure:
    track_id: int
    tag: str
    analyzer_id: str
    source_signature: str
    code: ReviewFailureCode
    error: str


@dataclass(frozen=True)
class BulkAnalysisTagReviewOutcome:
    requested_items: int
    applied: tuple[AnalysisTagReviewApplied, ...]
    failures: tuple[AnalysisTagReviewFailure, ...]


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
    analyzer_ids: Sequence[str] | None = None,
) -> Mapping[int, tuple[AnalysisTagSuggestion, ...]]:
    """Load reviewable tags from every current, approved analysis profile."""

    track_by_id = {track.id: track for track in tracks}
    if not track_by_id:
        return {}
    requested_analyzers = tuple(analyzer_ids or _REVIEWABLE_ANALYZERS)
    approved_analyzers = tuple(
        analyzer_id
        for analyzer_id in requested_analyzers
        if analyzer_id in _REVIEWABLE_ANALYZERS
    )
    if not approved_analyzers:
        return {}
    rows = list(
        db.scalars(
            select(TrackAnalysis).where(
                TrackAnalysis.analyzer_id.in_(approved_analyzers),
                TrackAnalysis.track_id.in_(track_by_id),
            ).order_by(TrackAnalysis.track_id, TrackAnalysis.analyzer_id)
        ).all()
    )
    reviews = {
        (review.track_id, review.analyzer_id, review.tag): review
        for review in db.scalars(
            select(TrackAnalysisTagReview).where(
                TrackAnalysisTagReview.track_id.in_(track_by_id),
                TrackAnalysisTagReview.analyzer_id.in_(approved_analyzers),
            )
        ).all()
    }
    model_role_fingerprint = current_role_runtime_fingerprint(
        db,
        MODEL_TAGGING_ROLE_ID,
    )
    vocabulary_fingerprint = load_tag_vocabulary(db).fingerprint
    audio_profiles = load_current_audio_profiles(db, list(track_by_id.values()))
    suggestions: dict[int, list[AnalysisTagSuggestion]] = {}
    for row in rows:
        track = track_by_id.get(row.track_id)
        if (
            track is None
            or row.source_signature
            != _current_source_signature(
                track,
                row.analyzer_id,
                model_role_fingerprint,
                vocabulary_fingerprint,
                audio_profiles.get(track.id),
            )
        ):
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
        suggestions.setdefault(row.track_id, []).extend(track_suggestions)
    return {
        track_id: tuple(track_suggestions)
        for track_id, track_suggestions in suggestions.items()
    }


def _current_source_signature(
    track: Track,
    analyzer_id: str,
    model_role_fingerprint: str | None,
    vocabulary_fingerprint: str,
    audio_profile: CurrentAudioProfile | None,
) -> str | None:
    if analyzer_id == LOCAL_METADATA_ANALYZER_ID:
        return track_source_signature(track)
    if analyzer_id == MODEL_TAG_ANALYZER_ID:
        if model_role_fingerprint is None:
            return None
        return model_tag_source_signature(
            track,
            model_role_fingerprint,
            vocabulary_fingerprint,
            audio_profile,
        )
    return None


def filter_tracks_by_review_status(
    db: Session,
    tracks: Sequence[Track],
    status: ReviewStatus,
    analyzer_ids: Sequence[str] | None = None,
) -> tuple[Track, ...]:
    """Return tracks with at least one current suggestion in ``status``."""

    matched: list[Track] = []
    for start in range(0, len(tracks), 500):
        chunk = tracks[start : start + 500]
        suggestions = load_current_analysis_tag_suggestions(
            db,
            chunk,
            analyzer_ids,
        )
        matched.extend(
            track
            for track in chunk
            if any(
                suggestion.status == status
                for suggestion in suggestions.get(track.id, ())
            )
        )
    return tuple(matched)


def _failure(
    target: AnalysisTagReviewTarget,
    code: ReviewFailureCode,
    error: str,
) -> AnalysisTagReviewFailure:
    return AnalysisTagReviewFailure(
        track_id=target.track_id,
        tag=target.tag,
        analyzer_id=target.analyzer_id,
        source_signature=target.source_signature,
        code=code,
        error=error,
    )


def review_analysis_tags_bulk(
    db: Session,
    targets: Sequence[AnalysisTagReviewTarget],
    *,
    decision: ReviewStatus,
) -> BulkAnalysisTagReviewOutcome:
    """Apply one explicit decision to selected suggestions in one transaction."""

    if decision not in {"pending", "accepted", "rejected"}:
        raise ValueError("invalid analysis tag review decision")
    canonical: list[AnalysisTagReviewTarget] = []
    seen: set[tuple[int, str, str, str]] = set()
    for target in targets:
        normalized = AnalysisTagReviewTarget(
            track_id=target.track_id,
            tag=normalize_manual_tag(target.tag),
            analyzer_id=target.analyzer_id,
            source_signature=target.source_signature,
        )
        key = (
            normalized.track_id,
            normalized.analyzer_id,
            normalized.source_signature,
            normalized.tag,
        )
        if key not in seen:
            seen.add(key)
            canonical.append(normalized)
    if not canonical:
        return BulkAnalysisTagReviewOutcome(0, (), ())

    track_ids = {target.track_id for target in canonical}
    with tag_write_lock:
        tracks = {
            track.id: track
            for track in db.scalars(
                select(Track).where(Track.id.in_(track_ids))
            ).all()
        }
        analyses = {
            (row.track_id, row.analyzer_id): row
            for row in db.scalars(
                select(TrackAnalysis).where(TrackAnalysis.track_id.in_(track_ids))
            ).all()
        }
        manual_rows = list(
            db.scalars(
                select(TrackUserTag).where(TrackUserTag.track_id.in_(track_ids))
            ).all()
        )
        manual_tags: dict[int, set[str]] = {track_id: set() for track_id in track_ids}
        for manual in manual_rows:
            manual_tags[manual.track_id].add(manual.tag)
        reviews = {
            (review.track_id, review.analyzer_id, review.tag): review
            for review in db.scalars(
                select(TrackAnalysisTagReview).where(
                    TrackAnalysisTagReview.track_id.in_(track_ids)
                )
            ).all()
        }

        valid: list[AnalysisTagReviewTarget] = []
        failures: list[AnalysisTagReviewFailure] = []
        profile_tags: dict[tuple[int, str], tuple[str, ...] | None] = {}
        model_role_fingerprint = current_role_runtime_fingerprint(
            db,
            MODEL_TAGGING_ROLE_ID,
        )
        vocabulary_fingerprint = load_tag_vocabulary(db).fingerprint
        audio_profiles = load_current_audio_profiles(db, list(tracks.values()))
        for target in canonical:
            track = tracks.get(target.track_id)
            row = analyses.get((target.track_id, target.analyzer_id))
            if track is None:
                failures.append(_failure(target, "not_found", "Track not found"))
                continue
            if row is None or target.analyzer_id not in _REVIEWABLE_ANALYZERS:
                failures.append(
                    _failure(target, "not_found", "Analysis profile not found")
                )
                continue
            if row.source_signature != target.source_signature:
                failures.append(
                    _failure(
                        target,
                        "stale",
                        "Analysis changed; refresh before reviewing this tag",
                    )
                )
                continue
            if row.source_signature != _current_source_signature(
                track,
                target.analyzer_id,
                model_role_fingerprint,
                vocabulary_fingerprint,
                audio_profiles.get(track.id),
            ):
                failures.append(
                    _failure(
                        target,
                        "stale",
                        "Track or analyzer settings changed; rerun analysis before reviewing this tag",
                    )
                )
                continue
            row_key = (row.track_id, row.analyzer_id)
            tags = profile_tags.setdefault(row_key, _profile_tags(row))
            if tags is None or target.tag not in tags:
                failures.append(
                    _failure(
                        target,
                        "not_found",
                        "Tag is not present in the current analysis profile",
                    )
                )
                continue
            valid.append(target)

        overflow_tracks: set[int] = set()
        if decision == "accepted":
            additions: dict[int, set[str]] = {}
            for target in valid:
                if target.tag not in manual_tags[target.track_id]:
                    additions.setdefault(target.track_id, set()).add(target.tag)
            overflow_tracks = {
                track_id
                for track_id, tags in additions.items()
                if len(manual_tags[track_id]) + len(tags) > MAX_TAGS_PER_TRACK
            }

        applied: list[AnalysisTagReviewApplied] = []
        try:
            for target in valid:
                if (
                    decision == "accepted"
                    and target.track_id in overflow_tracks
                    and target.tag not in manual_tags[target.track_id]
                ):
                    failures.append(
                        _failure(
                            target,
                            "tag_limit",
                            (
                                "selected suggestions would exceed the "
                                f"{MAX_TAGS_PER_TRACK}-tag limit"
                            ),
                        )
                    )
                    continue
                review_key = (target.track_id, target.analyzer_id, target.tag)
                review = reviews.get(review_key)
                if decision == "pending":
                    if review is not None:
                        db.delete(review)
                        reviews.pop(review_key)
                else:
                    if (
                        decision == "accepted"
                        and target.tag not in manual_tags[target.track_id]
                    ):
                        db.add(
                            TrackUserTag(track_id=target.track_id, tag=target.tag)
                        )
                        manual_tags[target.track_id].add(target.tag)
                    if review is None:
                        review = TrackAnalysisTagReview(
                            track_id=target.track_id,
                            analyzer_id=target.analyzer_id,
                            tag=target.tag,
                        )
                        db.add(review)
                        reviews[review_key] = review
                    review.source_signature = target.source_signature
                    review.decision = decision
                    review.reviewed_at = utcnow()
                applied.append(
                    AnalysisTagReviewApplied(
                        track_id=target.track_id,
                        tag=target.tag,
                        analyzer_id=target.analyzer_id,
                        source_signature=target.source_signature,
                        decision=decision,
                    )
                )
            db.commit()
        except Exception:
            db.rollback()
            raise

    return BulkAnalysisTagReviewOutcome(
        requested_items=len(canonical),
        applied=tuple(applied),
        failures=tuple(failures),
    )


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

    target = AnalysisTagReviewTarget(
        track_id=track_id,
        tag=tag,
        analyzer_id=analyzer_id,
        source_signature=source_signature,
    )
    result = review_analysis_tags_bulk(db, [target], decision=decision)
    if result.failures:
        failure = result.failures[0]
        if failure.code == "stale":
            raise StaleAnalysisSuggestionError(failure.error)
        if failure.code == "tag_limit":
            raise TagLimitError(failure.error)
        raise AnalysisSuggestionNotFoundError(failure.error)
    manual_tags = tuple(
        db.scalars(
            select(TrackUserTag.tag)
            .where(TrackUserTag.track_id == track_id)
            .order_by(TrackUserTag.tag)
        ).all()
    )

    return AnalysisTagReviewOutcome(
        track_id=track_id,
        tag=normalize_manual_tag(tag),
        analyzer_id=analyzer_id,
        source_signature=source_signature,
        decision=decision,
        manual_tags=tuple(sorted(manual_tags)),
    )
