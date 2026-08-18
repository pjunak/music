from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Literal, Protocol

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


@dataclass(frozen=True)
class TrackAnalysisProfile:
    """Analyzer-neutral mood axes available to suggestion engines."""

    energy: float
    brightness: float
    tension: float
    moods: tuple[str, ...]
    evidence: tuple[str, ...]
    confidence: Literal["high", "medium", "low"]


class PlaylistSuggestionEngine(Protocol):
    """Provider-independent boundary used by the authenticated API."""

    engine_id: str

    def suggest(
        self,
        tracks: Sequence[TrackLike],
        request: PlaylistSuggestionRequest,
        profiles: Mapping[int, TrackAnalysisProfile] | None = None,
    ) -> PlaylistSuggestionResponse: ...
