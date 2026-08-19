"""Authenticated local-first Assistant endpoints."""
from typing import NoReturn

from fastapi import APIRouter, HTTPException
from sqlalchemy import select

from app.api.deps import CurrentUser, DbSession
from app.assistant.analysis import (
    ANALYSIS_JOB_KIND,
    load_current_metadata_profiles,
    metadata_analysis_summary,
)
from app.assistant.audio_analysis import (
    AUDIO_ANALYSIS_JOB_KIND,
    audio_analysis_summary,
    load_current_audio_profiles,
)
from app.assistant.engine import PlaylistSuggestionEngine
from app.assistant.local import local_playlist_planner
from app.assistant.model_suggestions import (
    MODEL_PLAYLIST_SUGGESTION_JOB_KIND,
    model_playlist_availability,
    model_suggestion_job_parameters,
)
from app.assistant.providers.service import ProviderServiceError
from app.assistant.schemas import (
    LibraryAnalysisStartRequest,
    LibraryAnalysisSummary,
    ModelPlaylistAvailability,
    ModelPlaylistSuggestionStartRequest,
    PlaylistSuggestionRequest,
    PlaylistSuggestionResponse,
)
from app.assistant.tags import load_manual_tags
from app.jobs.runner import job_runner
from app.jobs.schemas import BackgroundJobOut, job_out
from app.jobs.service import enqueue_unique_active_job
from app.models.track import Track

router = APIRouter(prefix="/api/assistant", tags=["assistant"])
playlist_suggestion_engine: PlaylistSuggestionEngine = local_playlist_planner


def _raise_provider_error(error: ProviderServiceError) -> NoReturn:
    raise HTTPException(
        status_code=error.status_code,
        detail={"code": error.code, "message": error.message},
    ) from None


@router.post("/playlists/suggest", response_model=PlaylistSuggestionResponse)
def suggest_playlist(
    payload: PlaylistSuggestionRequest,
    _user: CurrentUser,
    db: DbSession,
) -> PlaylistSuggestionResponse:
    tracks = list(db.scalars(select(Track).order_by(Track.id)).all())
    profiles = load_current_metadata_profiles(db, tracks)
    signal_profiles = load_current_audio_profiles(db, tracks)
    manual_tags = load_manual_tags(db, [track.id for track in tracks])
    return playlist_suggestion_engine.suggest(
        tracks,
        payload,
        profiles=profiles,
        manual_tags=manual_tags,
        signal_profiles=signal_profiles,
    )


@router.get(
    "/playlists/model-status",
    response_model=ModelPlaylistAvailability,
)
def model_playlist_status(
    _user: CurrentUser,
    db: DbSession,
) -> ModelPlaylistAvailability:
    return model_playlist_availability(db)


@router.post(
    "/playlists/model-suggestions/jobs",
    response_model=BackgroundJobOut,
    status_code=202,
)
def start_model_playlist_suggestion(
    payload: ModelPlaylistSuggestionStartRequest,
    _user: CurrentUser,
    db: DbSession,
) -> BackgroundJobOut:
    try:
        parameters = model_suggestion_job_parameters(db, payload.request)
    except ProviderServiceError as exc:
        _raise_provider_error(exc)
    job, created = enqueue_unique_active_job(
        db,
        MODEL_PLAYLIST_SUGGESTION_JOB_KIND,
        parameters,
    )
    output = job_out(job)
    if not created and output.parameters != parameters:
        raise HTTPException(
            status_code=409,
            detail={
                "code": "model_suggestion_in_progress",
                "message": (
                    "Another model playlist suggestion is already running. "
                    "Wait for it to finish or cancel it first."
                ),
            },
        )
    if created:
        job_runner.wake()
    return output


@router.post(
    "/library-analysis/jobs",
    response_model=BackgroundJobOut,
    status_code=202,
)
def start_library_analysis(
    payload: LibraryAnalysisStartRequest,
    _user: CurrentUser,
    db: DbSession,
) -> BackgroundJobOut:
    job, created = enqueue_unique_active_job(
        db,
        ANALYSIS_JOB_KIND,
        {"force": payload.force},
    )
    if created:
        job_runner.wake()
    return job_out(job)


@router.get("/library-analysis/summary", response_model=LibraryAnalysisSummary)
def library_analysis_summary(
    _user: CurrentUser,
    db: DbSession,
) -> LibraryAnalysisSummary:
    return LibraryAnalysisSummary.model_validate(metadata_analysis_summary(db))


@router.post(
    "/library-audio-analysis/jobs",
    response_model=BackgroundJobOut,
    status_code=202,
)
def start_library_audio_analysis(
    payload: LibraryAnalysisStartRequest,
    _user: CurrentUser,
    db: DbSession,
) -> BackgroundJobOut:
    job, created = enqueue_unique_active_job(
        db,
        AUDIO_ANALYSIS_JOB_KIND,
        {"force": payload.force},
    )
    if created:
        job_runner.wake()
    return job_out(job)


@router.get(
    "/library-audio-analysis/summary",
    response_model=LibraryAnalysisSummary,
)
def library_audio_analysis_summary(
    _user: CurrentUser,
    db: DbSession,
) -> LibraryAnalysisSummary:
    return LibraryAnalysisSummary.model_validate(audio_analysis_summary(db))
