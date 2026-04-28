"""Importing this module registers all ORM mappers on Base.metadata.

Alembic's env.py imports `register_all` so autogenerate sees every table.
"""
from app.models.auth_session import AuthSession
from app.models.base import Base
from app.models.playback_state import PlaybackState
from app.models.playlist import Playlist, PlaylistItem
from app.models.track import Track
from app.models.user import User

register_all = True

__all__ = [
    "AuthSession",
    "Base",
    "PlaybackState",
    "Playlist",
    "PlaylistItem",
    "Track",
    "User",
    "register_all",
]
