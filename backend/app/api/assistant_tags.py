"""Operator-owned playlist tags, kept separate from generated analysis."""

from typing import Literal

from fastapi import APIRouter, HTTPException, Query
from sqlalchemy import func, or_, select

from app.api.deps import CurrentUser, DbSession
from app.assistant.analysis import (
    LOCAL_METADATA_ANALYZER_ID,
    load_current_metadata_profiles,
)
from app.assistant.tag_reviews import (
    AnalysisSuggestionNotFoundError,
    AnalysisTagReviewTarget,
    AnalysisTagSuggestion,
    StaleAnalysisSuggestionError,
    filter_tracks_by_review_status,
    load_current_analysis_tag_suggestions,
    review_analysis_tag,
    review_analysis_tags_bulk,
)
from app.assistant.tag_schemas import (
    AnalysisTagReviewRequest,
    AnalysisTagReviewResult,
    AnalysisTagSuggestionOut,
    BulkAnalysisTagReviewApplied,
    BulkAnalysisTagReviewFailure,
    BulkAnalysisTagReviewRequest,
    BulkAnalysisTagReviewResult,
    BulkManualTagFailure,
    BulkManualTagPatch,
    BulkManualTagResult,
    LibraryTagPage,
    LibraryTagTrack,
    ManualTagCatalog,
    ManualTagPatch,
    ManualTagRenameRequest,
    ManualTagRenameResult,
    ManualTagUsage,
    StarterTagGroupOut,
)
from app.assistant.tags import (
    DND_STARTER_TAG_GROUPS,
    TagLimitError,
    TagNotFoundError,
    load_manual_tags,
    manual_tag_usage,
    normalize_manual_tag,
    patch_manual_tags,
    patch_manual_tags_bulk,
    rename_manual_tag,
)
from app.models.track import Track
from app.models.track_user_tag import TrackUserTag

router = APIRouter(prefix="/api/assistant/library-tags", tags=["assistant-tags"])


def _track_out(
    track: Track,
    manual_tags: list[str],
    *,
    analysis_tags: list[str],
    analysis_confidence: Literal["high", "medium", "low"] | None,
    analysis_suggestions: list[AnalysisTagSuggestion],
) -> LibraryTagTrack:
    return LibraryTagTrack(
        track_id=track.id,
        path=track.path,
        title=track.title,
        display_title=track.display_title,
        artist=track.artist,
        album=track.album,
        manual_tags=manual_tags,
        analysis_analyzer=(
            LOCAL_METADATA_ANALYZER_ID if analysis_confidence is not None else None
        ),
        analysis_tags=analysis_tags,
        analysis_confidence=analysis_confidence,
        analysis_suggestions=[
            AnalysisTagSuggestionOut(
                tag=suggestion.tag,
                analyzer_id=suggestion.analyzer_id,
                source_signature=suggestion.source_signature,
                confidence=suggestion.confidence,
                evidence=list(suggestion.evidence),
                status=suggestion.status,
            )
            for suggestion in analysis_suggestions
        ],
    )


@router.get("/catalog", response_model=ManualTagCatalog)
def tag_catalog(_user: CurrentUser, db: DbSession) -> ManualTagCatalog:
    usage = manual_tag_usage(db)
    return ManualTagCatalog(
        starter_groups=[
            StarterTagGroupOut(
                key=group.key,
                label=group.label,
                tags=list(group.tags),
            )
            for group in DND_STARTER_TAG_GROUPS
        ],
        used_tags=[item.tag for item in usage],
        tag_usage=[
            ManualTagUsage(tag=item.tag, track_count=item.track_count)
            for item in usage
        ],
    )


@router.post("/catalog/rename", response_model=ManualTagRenameResult)
def rename_library_tag(
    payload: ManualTagRenameRequest,
    _user: CurrentUser,
    db: DbSession,
) -> ManualTagRenameResult:
    try:
        outcome = rename_manual_tag(db, payload.source, payload.target)
    except TagNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc
    return ManualTagRenameResult(
        source=outcome.source,
        target=outcome.target,
        affected_tracks=outcome.affected_tracks,
        merged=outcome.merged,
    )


@router.post("/bulk", response_model=BulkManualTagResult)
def update_library_tags_bulk(
    payload: BulkManualTagPatch,
    _user: CurrentUser,
    db: DbSession,
) -> BulkManualTagResult:
    outcome = patch_manual_tags_bulk(
        db,
        payload.track_ids,
        add=payload.add,
        remove=payload.remove,
    )
    return BulkManualTagResult(
        requested_tracks=outcome.requested_tracks,
        matched_tracks=outcome.matched_tracks,
        changed_track_ids=list(outcome.changed_track_ids),
        missing_track_ids=list(outcome.missing_track_ids),
        failures=[
            BulkManualTagFailure(track_id=item.track_id, error=item.error)
            for item in outcome.failures
        ],
    )


@router.post(
    "/analysis-tags/reviews/bulk",
    response_model=BulkAnalysisTagReviewResult,
)
def update_analysis_tag_reviews_bulk(
    payload: BulkAnalysisTagReviewRequest,
    _user: CurrentUser,
    db: DbSession,
) -> BulkAnalysisTagReviewResult:
    outcome = review_analysis_tags_bulk(
        db,
        [
            AnalysisTagReviewTarget(
                track_id=item.track_id,
                tag=item.tag,
                analyzer_id=item.analyzer_id,
                source_signature=item.source_signature,
            )
            for item in payload.items
        ],
        decision=payload.decision,
    )
    return BulkAnalysisTagReviewResult(
        requested_items=outcome.requested_items,
        applied=[
            BulkAnalysisTagReviewApplied(
                track_id=item.track_id,
                tag=item.tag,
                analyzer_id=item.analyzer_id,
                source_signature=item.source_signature,
                decision=payload.decision,
            )
            for item in outcome.applied
        ],
        failures=[
            BulkAnalysisTagReviewFailure(
                track_id=item.track_id,
                tag=item.tag,
                analyzer_id=item.analyzer_id,
                source_signature=item.source_signature,
                code=item.code,
                error=item.error,
            )
            for item in outcome.failures
        ],
    )


@router.get("", response_model=LibraryTagPage)
def list_library_tags(
    _user: CurrentUser,
    db: DbSession,
    search: str = Query(default="", max_length=128),
    tag: str | None = Query(default=None, max_length=64),
    review: Literal["pending", "accepted", "rejected"] | None = Query(default=None),
    offset: int = Query(default=0, ge=0),
    limit: int = Query(default=50, ge=1, le=100),
) -> LibraryTagPage:
    filters = []
    search = search.strip()
    if search:
        filters.append(
            or_(
                Track.display_title.contains(search, autoescape=True),
                Track.title.contains(search, autoescape=True),
                Track.artist.contains(search, autoescape=True),
                Track.album.contains(search, autoescape=True),
                Track.path.contains(search, autoescape=True),
            )
        )

    normalized_tag: str | None = None
    if tag is not None:
        try:
            normalized_tag = normalize_manual_tag(tag)
        except ValueError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    query = select(Track)
    count_query = select(func.count()).select_from(Track)
    if normalized_tag is not None:
        query = query.join(TrackUserTag).where(TrackUserTag.tag == normalized_tag)
        count_query = count_query.join(TrackUserTag).where(
            TrackUserTag.tag == normalized_tag
        )
    if filters:
        query = query.where(*filters)
        count_query = count_query.where(*filters)

    if review is not None:
        reviewable = filter_tracks_by_review_status(
            db,
            list(db.scalars(query).all()),
            review,
        )
        ordered = sorted(
            reviewable,
            key=lambda track: (
                (track.display_title or track.title).casefold(),
                track.id,
            ),
        )
        total = len(ordered)
        tracks = ordered[offset : offset + limit]
    else:
        total = int(db.scalar(count_query) or 0)
        tracks = list(
            db.scalars(
                query.order_by(
                    func.lower(
                        func.coalesce(func.nullif(Track.display_title, ""), Track.title)
                    ),
                    Track.id,
                )
                .offset(offset)
                .limit(limit)
            ).all()
        )
    track_ids = [track.id for track in tracks]
    manual_by_track = load_manual_tags(db, track_ids)
    profiles = load_current_metadata_profiles(db, tracks)
    suggestions = load_current_analysis_tag_suggestions(db, tracks)
    items: list[LibraryTagTrack] = []
    for track in tracks:
        profile = profiles.get(track.id)
        items.append(
            _track_out(
                track,
                list(manual_by_track.get(track.id, ())),
                analysis_tags=list(profile.moods) if profile is not None else [],
                analysis_confidence=(profile.confidence if profile is not None else None),
                analysis_suggestions=list(suggestions.get(track.id, ())),
            )
        )
    return LibraryTagPage(items=items, total=total, offset=offset, limit=limit)


@router.patch("/{track_id}", response_model=LibraryTagTrack)
def update_library_tags(
    track_id: int,
    payload: ManualTagPatch,
    _user: CurrentUser,
    db: DbSession,
) -> LibraryTagTrack:
    track = db.get(Track, track_id)
    if track is None:
        raise HTTPException(status_code=404, detail="Track not found")
    try:
        manual_tags = patch_manual_tags(
            db,
            track_id,
            add=payload.add,
            remove=payload.remove,
        )
    except (TagLimitError, ValueError) as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc
    profile = load_current_metadata_profiles(db, [track]).get(track.id)
    return _track_out(
        track,
        list(manual_tags),
        analysis_tags=list(profile.moods) if profile is not None else [],
        analysis_confidence=profile.confidence if profile is not None else None,
        analysis_suggestions=list(
            load_current_analysis_tag_suggestions(db, [track]).get(track.id, ())
        ),
    )


@router.put("/{track_id}/analysis-tags/review", response_model=AnalysisTagReviewResult)
def update_analysis_tag_review(
    track_id: int,
    payload: AnalysisTagReviewRequest,
    _user: CurrentUser,
    db: DbSession,
) -> AnalysisTagReviewResult:
    try:
        outcome = review_analysis_tag(
            db,
            track_id,
            analyzer_id=payload.analyzer_id,
            source_signature=payload.source_signature,
            tag=payload.tag,
            decision=payload.decision,
        )
    except AnalysisSuggestionNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    except StaleAnalysisSuggestionError as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc
    except (TagLimitError, ValueError) as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc
    return AnalysisTagReviewResult(
        track_id=outcome.track_id,
        tag=outcome.tag,
        analyzer_id=outcome.analyzer_id,
        source_signature=outcome.source_signature,
        decision=outcome.decision,
        manual_tags=list(outcome.manual_tags),
    )
