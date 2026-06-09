"""Importing this module registers all ORM mappers on Base.metadata, so
`Base.metadata.create_all` in the lifespan picks up every table without
each caller having to import each model individually."""
from app.models.auth_session import AuthSession
from app.models.base import Base
from app.models.cleanup_batch import CleanupBatch
from app.models.playback_state import PlaybackState
from app.models.playlist import Playlist, PlaylistItem
from app.models.track import Track
from app.models.user import User

__all__ = [
    "AuthSession",
    "Base",
    "CleanupBatch",
    "PlaybackState",
    "Playlist",
    "PlaylistItem",
    "Track",
    "User",
]
