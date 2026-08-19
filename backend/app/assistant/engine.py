from __future__ import annotations

import re
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


class TrackSignalProfile(Protocol):
    """Current signal evidence supplied independently from semantic tags."""

    @property
    def analyzer_id(self) -> str: ...

    @property
    def energy(self) -> float: ...

    @property
    def brightness(self) -> float: ...

    @property
    def tension(self) -> float: ...

    @property
    def tempo_bpm(self) -> float | None: ...

    @property
    def confidence(self) -> Literal["high", "medium", "low"]: ...


class PlaylistSuggestionEngine(Protocol):
    """Provider-independent boundary used by the authenticated API."""

    engine_id: str

    def suggest(
        self,
        tracks: Sequence[TrackLike],
        request: PlaylistSuggestionRequest,
        profiles: Mapping[int, TrackAnalysisProfile] | None = None,
        manual_tags: Mapping[int, Sequence[str]] | None = None,
        signal_profiles: Mapping[int, TrackSignalProfile] | None = None,
    ) -> PlaylistSuggestionResponse: ...


class SuggestionEngineError(RuntimeError):
    """An engine failure that is safe to expose in synthetic evaluation output."""

    def __init__(self, code: str) -> None:
        safe_code = code if re.fullmatch(r"[a-z0-9_]{1,64}", code) else "engine_failure"
        super().__init__(safe_code)
        self.code = safe_code
