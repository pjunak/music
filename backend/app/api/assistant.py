"""Authenticated local-first Assistant endpoints."""
from fastapi import APIRouter
from sqlalchemy import func, select

from app.api.deps import CurrentUser, DbSession
from app.assistant.analysis import (
    ANALYSIS_JOB_KIND,
    LOCAL_METADATA_ANALYZER_ID,
    load_current_metadata_profiles,
)
from app.assistant.engine import PlaylistSuggestionEngine
from app.assistant.local import local_metadata_playlist_engine
from app.assistant.schemas import (
    LibraryAnalysisStartRequest,
    LibraryAnalysisSummary,
    PlaylistSuggestionRequest,
    PlaylistSuggestionResponse,
)
from app.jobs.runner import job_runner
from app.jobs.schemas import BackgroundJobOut, job_out
from app.jobs.service import enqueue_unique_active_job
from app.models.track import Track
from app.models.track_analysis import TrackAnalysis

router = APIRouter(prefix="/api/assistant", tags=["assistant"])
playlist_suggestion_engine: PlaylistSuggestionEngine = local_metadata_playlist_engine


@router.post("/playlists/suggest", response_model=PlaylistSuggestionResponse)
def suggest_playlist(
    payload: PlaylistSuggestionRequest,
    _user: CurrentUser,
    db: DbSession,
) -> PlaylistSuggestionResponse:
    tracks = list(db.scalars(select(Track).order_by(Track.id)).all())
    profiles = load_current_metadata_profiles(db, tracks)
    return playlist_suggestion_engine.suggest(tracks, payload, profiles)


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
    library_tracks = db.scalar(select(func.count()).select_from(Track)) or 0
    rows = db.execute(
        select(
            TrackAnalysis.confidence,
            func.count(TrackAnalysis.track_id),
        )
        .where(TrackAnalysis.analyzer_id == LOCAL_METADATA_ANALYZER_ID)
        .group_by(TrackAnalysis.confidence)
    ).all()
    confidence_counts = {str(confidence): int(count) for confidence, count in rows}
    analyzed_tracks = sum(confidence_counts.values())
    last_updated_at = db.scalar(
        select(func.max(TrackAnalysis.updated_at)).where(
            TrackAnalysis.analyzer_id == LOCAL_METADATA_ANALYZER_ID
        )
    )
    return LibraryAnalysisSummary(
        analyzer=LOCAL_METADATA_ANALYZER_ID,
        library_tracks=library_tracks,
        analyzed_tracks=analyzed_tracks,
        high_confidence=confidence_counts.get("high", 0),
        medium_confidence=confidence_counts.get("medium", 0),
        low_confidence=confidence_counts.get("low", 0),
        last_updated_at=last_updated_at,
    )
