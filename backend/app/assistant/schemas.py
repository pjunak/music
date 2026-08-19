from __future__ import annotations

from datetime import datetime
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator


class StrictAssistantModel(BaseModel):
    model_config = ConfigDict(extra="forbid")


class PlaylistSuggestionRequest(StrictAssistantModel):
    prompt: str = Field(min_length=2, max_length=500)
    target_minutes: int = Field(default=60, ge=5, le=600)
    candidate_limit: int = Field(default=40, ge=5, le=100)
    min_bpm: int | None = Field(default=None, ge=1, le=999)
    max_bpm: int | None = Field(default=None, ge=1, le=999)
    include_unknown_bpm: bool = True
    exclude_track_ids: list[int] = Field(default_factory=list, max_length=5000)

    @field_validator("prompt", mode="before")
    @classmethod
    def normalize_prompt(cls, value: object) -> object:
        return value.strip() if isinstance(value, str) else value

    @model_validator(mode="after")
    def valid_bpm_range(self) -> PlaylistSuggestionRequest:
        if (
            self.min_bpm is not None
            and self.max_bpm is not None
            and self.min_bpm > self.max_bpm
        ):
            raise ValueError("min_bpm cannot be greater than max_bpm")
        return self


class PlaylistIntent(StrictAssistantModel):
    matched_moods: list[str]
    search_terms: list[str]
    energy: float = Field(ge=0.0, le=1.0)
    brightness: float = Field(ge=0.0, le=1.0)
    tension: float = Field(ge=0.0, le=1.0)


class PlaylistCandidate(StrictAssistantModel):
    track_id: int
    path: str
    title: str
    display_title: str
    artist: str
    album: str
    origin: str
    genre: str
    manual_tags: list[str]
    analysis_tags: list[str]
    length_s: float = Field(ge=0.0)
    bpm: int | None
    match_score: float = Field(ge=0.0, le=1.0)
    confidence: Literal["high", "medium", "low"]
    reasons: list[str]
    default_selected: bool


class PlaylistSuggestionResponse(StrictAssistantModel):
    engine: str = Field(min_length=1, max_length=128)
    library_tracks: int
    eligible_tracks: int
    intent: PlaylistIntent
    candidates: list[PlaylistCandidate]


class LibraryAnalysisStartRequest(StrictAssistantModel):
    force: bool = False


class LibraryAnalysisSummary(StrictAssistantModel):
    analyzer: str
    library_tracks: int = Field(ge=0)
    analyzed_tracks: int = Field(ge=0)
    failed_tracks: int = Field(ge=0)
    stale_tracks: int = Field(ge=0)
    high_confidence: int = Field(ge=0)
    medium_confidence: int = Field(ge=0)
    low_confidence: int = Field(ge=0)
    last_updated_at: datetime | None
