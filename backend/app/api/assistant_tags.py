"""Operator-owned playlist tags, kept separate from generated analysis."""

import json
from typing import Literal, NoReturn

from fastapi import APIRouter, HTTPException, Query
from sqlalchemy import func, or_, select

from app.api.deps import CurrentUser, DbSession
from app.assistant.analysis import (
    LOCAL_METADATA_ANALYZER_ID,
    load_current_metadata_profiles,
)
from app.assistant.audio_analysis import CurrentAudioProfile, load_current_audio_profiles
from app.assistant.model_tag_cleanup_job import (
    MODEL_TAG_CLEANUP_JOB_KIND,
    model_tag_cleanup_availability,
    model_tag_cleanup_job_parameters,
)
from app.assistant.model_tagger import MODEL_TAG_ANALYZER_ID
from app.assistant.model_tagging import (
    MODEL_TAGGING_JOB_KIND,
    model_tagging_availability,
    model_tagging_job_parameters,
    resolve_model_tagging_scope,
)
from app.assistant.providers.service import ProviderServiceError
from app.assistant.tag_cleanup import (
    TAG_CLEANUP_SCHEMA_VERSION,
    InvalidTagCleanupSelectionError,
    StaleTagCleanupError,
    TagCleanupSelection,
    apply_reviewed_tag_renames,
    apply_tag_cleanup,
    preview_tag_cleanup,
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
    AudioSignalProfileOut,
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
    ModelTagCleanupApplyRequest,
    ModelTagCleanupAvailability,
    ModelTagCleanupJobResult,
    ModelTagCleanupStartRequest,
    ModelTaggingAvailability,
    ModelTaggingPlanRequest,
    ModelTaggingReviewQuery,
    ModelTaggingStartRequest,
    StarterTagGroupOut,
    TagCleanupApplyRequest,
    TagCleanupApplyResult,
    TagCleanupPreviewOut,
    TagCleanupSuggestionOut,
    TagVocabularyOut,
    TagVocabularyUpdateRequest,
)
from app.assistant.tag_vocabulary import (
    TAG_VOCABULARY_SCHEMA,
    TagVocabularyConflictError,
    TagVocabularyDocument,
    load_tag_vocabulary,
    replace_tag_vocabulary,
)
from app.assistant.tags import (
    TagLimitError,
    TagNotFoundError,
    load_manual_tags,
    manual_tag_usage,
    normalize_manual_tag,
    patch_manual_tags,
    patch_manual_tags_bulk,
    rename_manual_tag,
)
from app.jobs.runner import job_runner
from app.jobs.schemas import BackgroundJobOut, job_out
from app.jobs.service import enqueue_unique_active_job
from app.models.background_job import BackgroundJob
from app.models.track import Track
from app.models.track_user_tag import TrackUserTag

router = APIRouter(prefix="/api/assistant/library-tags", tags=["assistant-tags"])


def _raise_provider_error(error: ProviderServiceError) -> NoReturn:
    raise HTTPException(
        status_code=error.status_code,
        detail={"code": error.code, "message": error.message},
    ) from None


def _track_out(
    track: Track,
    manual_tags: list[str],
    *,
    analysis_tags: list[str],
    analysis_confidence: Literal["high", "medium", "low"] | None,
    analysis_suggestions: list[AnalysisTagSuggestion],
    audio_profile: CurrentAudioProfile | None,
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
        audio_signal=(
            AudioSignalProfileOut(
                analyzer_id=audio_profile.analyzer_id,
                confidence=audio_profile.confidence,
                evidence=list(audio_profile.evidence),
                metrics=audio_profile.metrics,
            )
            if audio_profile is not None
            else None
        ),
    )


def _library_tag_page(
    db: DbSession,
    tracks: list[Track],
    *,
    total: int,
    offset: int,
    limit: int,
    analyzer_ids: tuple[str, ...] | None = None,
) -> LibraryTagPage:
    track_ids = [track.id for track in tracks]
    manual_by_track = load_manual_tags(db, track_ids)
    profiles = load_current_metadata_profiles(db, tracks)
    audio_profiles = load_current_audio_profiles(db, tracks)
    suggestions = load_current_analysis_tag_suggestions(
        db,
        tracks,
        analyzer_ids,
    )
    items: list[LibraryTagTrack] = []
    for track in tracks:
        profile = profiles.get(track.id)
        items.append(
            _track_out(
                track,
                list(manual_by_track.get(track.id, ())),
                analysis_tags=list(profile.moods) if profile is not None else [],
                analysis_confidence=(
                    profile.confidence if profile is not None else None
                ),
                analysis_suggestions=list(suggestions.get(track.id, ())),
                audio_profile=audio_profiles.get(track.id),
            )
        )
    return LibraryTagPage(items=items, total=total, offset=offset, limit=limit)


@router.get("/model-status", response_model=ModelTaggingAvailability)
def model_music_tagging_status(
    _user: CurrentUser,
    db: DbSession,
) -> ModelTaggingAvailability:
    return model_tagging_availability(db)


@router.post("/model-plan", response_model=ModelTaggingAvailability)
def plan_model_music_tagging(
    payload: ModelTaggingPlanRequest,
    _user: CurrentUser,
    db: DbSession,
) -> ModelTaggingAvailability:
    return model_tagging_availability(db, payload.scope, payload.context_policy)


@router.post(
    "/model-jobs",
    response_model=BackgroundJobOut,
    status_code=202,
)
def start_model_music_tagging(
    payload: ModelTaggingStartRequest,
    _user: CurrentUser,
    db: DbSession,
) -> BackgroundJobOut:
    try:
        parameters = model_tagging_job_parameters(
            db,
            force=payload.force,
            scope=payload.scope,
            context_policy=payload.context_policy,
        )
    except ProviderServiceError as exc:
        _raise_provider_error(exc)
    job, created = enqueue_unique_active_job(
        db,
        MODEL_TAGGING_JOB_KIND,
        parameters,
    )
    output = job_out(job)
    if not created and output.parameters != parameters:
        raise HTTPException(
            status_code=409,
            detail={
                "code": "model_tagging_in_progress",
                "message": (
                    "Another model music-tagging job is already running. "
                    "Wait for it to finish or cancel it first."
                ),
            },
        )
    if created:
        job_runner.wake()
    return output


@router.get("/catalog", response_model=ManualTagCatalog)
def tag_catalog(_user: CurrentUser, db: DbSession) -> ManualTagCatalog:
    usage = manual_tag_usage(db)
    vocabulary = load_tag_vocabulary(db)
    return ManualTagCatalog(
        starter_groups=[
            StarterTagGroupOut(
                key=group.key,
                label=group.label,
                tags=[tag.name for tag in group.tags],
            )
            for group in vocabulary.document.groups
        ],
        used_tags=[item.tag for item in usage],
        tag_usage=[
            ManualTagUsage(tag=item.tag, track_count=item.track_count)
            for item in usage
        ],
    )


@router.get("/vocabulary", response_model=TagVocabularyOut)
def get_tag_vocabulary(_user: CurrentUser, db: DbSession) -> TagVocabularyOut:
    snapshot = load_tag_vocabulary(db)
    return TagVocabularyOut(
        schema_version=TAG_VOCABULARY_SCHEMA,
        revision=snapshot.revision,
        fingerprint=snapshot.fingerprint,
        groups=list(snapshot.document.groups),
    )


@router.put("/vocabulary", response_model=TagVocabularyOut)
def update_tag_vocabulary(
    payload: TagVocabularyUpdateRequest,
    _user: CurrentUser,
    db: DbSession,
) -> TagVocabularyOut:
    document = TagVocabularyDocument(
        schema_version=payload.schema_version,
        groups=payload.groups,
    )
    try:
        snapshot = replace_tag_vocabulary(
            db,
            expected_revision=payload.expected_revision,
            document=document,
        )
    except TagVocabularyConflictError as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc
    return TagVocabularyOut(
        schema_version=TAG_VOCABULARY_SCHEMA,
        revision=snapshot.revision,
        fingerprint=snapshot.fingerprint,
        groups=list(snapshot.document.groups),
    )


@router.get(
    "/catalog/model-cleanup-status",
    response_model=ModelTagCleanupAvailability,
)
def model_library_tag_cleanup_status(
    _user: CurrentUser,
    db: DbSession,
) -> ModelTagCleanupAvailability:
    return model_tag_cleanup_availability(db)


@router.post(
    "/catalog/model-cleanup-jobs",
    response_model=BackgroundJobOut,
    status_code=202,
)
def start_model_library_tag_cleanup(
    _payload: ModelTagCleanupStartRequest,
    _user: CurrentUser,
    db: DbSession,
) -> BackgroundJobOut:
    try:
        parameters = model_tag_cleanup_job_parameters(db)
    except ProviderServiceError as exc:
        _raise_provider_error(exc)
    job, created = enqueue_unique_active_job(
        db,
        MODEL_TAG_CLEANUP_JOB_KIND,
        parameters,
    )
    output = job_out(job)
    if not created and output.parameters != parameters:
        raise HTTPException(
            status_code=409,
            detail={
                "code": "model_tag_cleanup_in_progress",
                "message": (
                    "Another model tag-cleanup job is already running. "
                    "Wait for it to finish or cancel it first."
                ),
            },
        )
    if created:
        job_runner.wake()
    return output


@router.post(
    "/catalog/model-cleanup-apply",
    response_model=TagCleanupApplyResult,
)
def apply_model_library_tag_cleanup(
    payload: ModelTagCleanupApplyRequest,
    _user: CurrentUser,
    db: DbSession,
) -> TagCleanupApplyResult:
    job = db.get(BackgroundJob, payload.job_id)
    if job is None or job.kind != MODEL_TAG_CLEANUP_JOB_KIND:
        raise HTTPException(
            status_code=404,
            detail={
                "code": "model_tag_cleanup_job_not_found",
                "message": "Model tag-cleanup proposal not found.",
            },
        )
    if job.status != "succeeded" or job.result_json is None:
        raise HTTPException(
            status_code=409,
            detail={
                "code": "model_tag_cleanup_job_incomplete",
                "message": "Model tag-cleanup proposal is not complete.",
            },
        )
    try:
        result = ModelTagCleanupJobResult.model_validate(
            json.loads(job.result_json)
        )
    except (ValueError, TypeError) as exc:
        raise HTTPException(
            status_code=409,
            detail={
                "code": "model_tag_cleanup_result_invalid",
                "message": "Stored model tag-cleanup proposal is invalid.",
            },
        ) from exc
    if payload.catalog_signature != result.catalog_signature:
        raise HTTPException(
            status_code=422,
            detail={
                "code": "model_tag_cleanup_signature_mismatch",
                "message": "The selected proposal signature does not match its job.",
            },
        )
    if payload.vocabulary_fingerprint != result.vocabulary_fingerprint:
        raise HTTPException(
            status_code=422,
            detail={
                "code": "model_tag_cleanup_vocabulary_mismatch",
                "message": (
                    "The selected vocabulary fingerprint does not match its job."
                ),
            },
        )
    if load_tag_vocabulary(db).fingerprint != result.vocabulary_fingerprint:
        raise HTTPException(
            status_code=409,
            detail={
                "code": "tag_cleanup_stale",
                "message": (
                    "The tag vocabulary changed after this proposal was created. "
                    "Run cleanup again."
                ),
            },
        )
    try:
        outcome = apply_reviewed_tag_renames(
            db,
            result.catalog_signature,
            [
                TagCleanupSelection(source=item.source, target=item.target)
                for item in payload.items
            ],
            allowed_pairs={
                (item.source, item.target) for item in result.suggestions
            },
        )
    except StaleTagCleanupError as exc:
        raise HTTPException(
            status_code=409,
            detail={"code": "tag_cleanup_stale", "message": str(exc)},
        ) from exc
    except InvalidTagCleanupSelectionError as exc:
        raise HTTPException(
            status_code=422,
            detail={"code": "tag_cleanup_invalid_selection", "message": str(exc)},
        ) from exc
    return TagCleanupApplyResult(
        schema_version="assistant-tag-cleanup-apply/v1",
        requested_items=len(payload.items),
        applied=[
            ManualTagRenameResult(
                source=item.source,
                target=item.target,
                affected_tracks=item.affected_tracks,
                merged=item.merged,
            )
            for item in outcome.applied
        ],
        catalog_signature=outcome.catalog_signature,
    )


@router.get("/catalog/cleanup-preview", response_model=TagCleanupPreviewOut)
def preview_library_tag_cleanup(
    _user: CurrentUser,
    db: DbSession,
) -> TagCleanupPreviewOut:
    preview = preview_tag_cleanup(db)
    return TagCleanupPreviewOut(
        schema_version=TAG_CLEANUP_SCHEMA_VERSION,
        catalog_signature=preview.catalog_signature,
        vocabulary_fingerprint=preview.vocabulary_fingerprint,
        suggestions=[
            TagCleanupSuggestionOut(
                id=item.id,
                source=item.source,
                target=item.target,
                reason_code=item.reason_code,
                reason=item.reason,
                source_track_count=item.source_track_count,
                target_track_count=item.target_track_count,
                merged=item.merged,
            )
            for item in preview.suggestions
        ],
    )


@router.post("/catalog/cleanup-apply", response_model=TagCleanupApplyResult)
def apply_library_tag_cleanup(
    payload: TagCleanupApplyRequest,
    _user: CurrentUser,
    db: DbSession,
) -> TagCleanupApplyResult:
    try:
        outcome = apply_tag_cleanup(
            db,
            payload.catalog_signature,
            payload.vocabulary_fingerprint,
            [
                TagCleanupSelection(source=item.source, target=item.target)
                for item in payload.items
            ],
        )
    except StaleTagCleanupError as exc:
        raise HTTPException(
            status_code=409,
            detail={"code": "tag_cleanup_stale", "message": str(exc)},
        ) from exc
    except InvalidTagCleanupSelectionError as exc:
        raise HTTPException(
            status_code=422,
            detail={"code": "tag_cleanup_invalid_selection", "message": str(exc)},
        ) from exc
    return TagCleanupApplyResult(
        schema_version="assistant-tag-cleanup-apply/v1",
        requested_items=len(payload.items),
        applied=[
            ManualTagRenameResult(
                source=item.source,
                target=item.target,
                affected_tracks=item.affected_tracks,
                merged=item.merged,
            )
            for item in outcome.applied
        ],
        catalog_signature=outcome.catalog_signature,
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


@router.post("/query", response_model=LibraryTagPage)
def query_model_library_tags(
    payload: ModelTaggingReviewQuery,
    _user: CurrentUser,
    db: DbSession,
) -> LibraryTagPage:
    scoped = resolve_model_tagging_scope(db, payload.scope)
    reviewable = filter_tracks_by_review_status(
        db,
        scoped,
        payload.review,
        (MODEL_TAG_ANALYZER_ID,),
    )
    ordered = sorted(
        reviewable,
        key=lambda track: (
            (track.display_title or track.title).casefold(),
            track.id,
        ),
    )
    tracks = list(ordered[payload.offset : payload.offset + payload.limit])
    return _library_tag_page(
        db,
        tracks,
        total=len(ordered),
        offset=payload.offset,
        limit=payload.limit,
        analyzer_ids=(MODEL_TAG_ANALYZER_ID,),
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
    return _library_tag_page(
        db,
        tracks,
        total=total,
        offset=offset,
        limit=limit,
    )


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
    audio_profile = load_current_audio_profiles(db, [track]).get(track.id)
    return _track_out(
        track,
        list(manual_tags),
        analysis_tags=list(profile.moods) if profile is not None else [],
        analysis_confidence=profile.confidence if profile is not None else None,
        analysis_suggestions=list(
            load_current_analysis_tag_suggestions(db, [track]).get(track.id, ())
        ),
        audio_profile=audio_profile,
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
