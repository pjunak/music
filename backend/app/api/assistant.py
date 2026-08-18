"""Authenticated local-first Assistant endpoints."""
from fastapi import APIRouter
from sqlalchemy import select

from app.api.deps import CurrentUser, DbSession
from app.assistant.engine import PlaylistSuggestionEngine
from app.assistant.local import local_metadata_playlist_engine
from app.assistant.schemas import (
    PlaylistSuggestionRequest,
    PlaylistSuggestionResponse,
)
from app.models.track import Track

router = APIRouter(prefix="/api/assistant", tags=["assistant"])
playlist_suggestion_engine: PlaylistSuggestionEngine = local_metadata_playlist_engine


@router.post("/playlists/suggest", response_model=PlaylistSuggestionResponse)
def suggest_playlist(
    payload: PlaylistSuggestionRequest,
    _user: CurrentUser,
    db: DbSession,
) -> PlaylistSuggestionResponse:
    tracks = list(db.scalars(select(Track).order_by(Track.id)).all())
    return playlist_suggestion_engine.suggest(tracks, payload)
