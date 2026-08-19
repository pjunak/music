"""Importing this module registers all ORM mappers on Base.metadata, so
`Base.metadata.create_all` in the lifespan picks up every table without
each caller having to import each model individually."""
from app.models.assistant_model_role import AssistantModelRole
from app.models.assistant_provider_connection import AssistantProviderConnection
from app.models.auth_session import AuthSession
from app.models.background_job import BackgroundJob
from app.models.base import Base
from app.models.cleanup_batch import CleanupBatch
from app.models.cleanup_lookup import CleanupNameLookup
from app.models.playback_state import PlaybackState
from app.models.playlist import Playlist, PlaylistItem
from app.models.track import Track
from app.models.track_analysis import TrackAnalysis
from app.models.track_analysis_failure import TrackAnalysisFailure
from app.models.track_analysis_tag_review import TrackAnalysisTagReview
from app.models.track_user_tag import TrackUserTag
from app.models.user import User

__all__ = [
    "AssistantModelRole",
    "AssistantProviderConnection",
    "AuthSession",
    "BackgroundJob",
    "Base",
    "CleanupBatch",
    "CleanupNameLookup",
    "PlaybackState",
    "Playlist",
    "PlaylistItem",
    "Track",
    "TrackAnalysis",
    "TrackAnalysisFailure",
    "TrackAnalysisTagReview",
    "TrackUserTag",
    "User",
]
