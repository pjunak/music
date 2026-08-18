from __future__ import annotations

from collections.abc import Sequence
from typing import Protocol

from app.assistant.schemas import PlaylistSuggestionRequest, PlaylistSuggestionResponse


class TrackLike(Protocol):
    """The read-only library data available to every suggestion engine."""

    id: int
    path: str
    title: str
    display_title: str
    artist: str
    album: str
    origin: str
    genre: str
    length_s: float
    bpm: int | None


class PlaylistSuggestionEngine(Protocol):
    """Provider-independent boundary used by the authenticated API."""

    engine_id: str

    def suggest(
        self,
        tracks: Sequence[TrackLike],
        request: PlaylistSuggestionRequest,
    ) -> PlaylistSuggestionResponse: ...
