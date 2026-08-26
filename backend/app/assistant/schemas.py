from datetime import datetime
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

from app.assistant.model_eq import EqPresetDraft
from app.assistant.providers.schemas import ProviderUsageSummary
from app.assistant.tag_schemas import ModelTaggingScope


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
    energy_curve: Literal["steady", "rising", "falling", "arc"] = "steady"

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


class PlaylistAudioSignal(StrictAssistantModel):
    analyzer_id: str
    energy: float = Field(ge=0.0, le=1.0)
    brightness: float = Field(ge=0.0, le=1.0)
    tension: float = Field(ge=0.0, le=1.0)
    tempo_bpm: float | None = Field(default=None, gt=0.0, le=999.0)
    confidence: Literal["high", "medium", "low"]


class PlaylistPlan(StrictAssistantModel):
    energy_curve: Literal["steady", "rising", "falling", "arc"]
    selected_tracks: int = Field(ge=0)
    selected_duration_s: float = Field(ge=0.0)
    audio_profile_tracks: int = Field(ge=0)


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
    sequence_position: int | None = Field(default=None, ge=1)
    planning_energy: float = Field(ge=0.0, le=1.0)
    audio_signal: PlaylistAudioSignal | None


class PlaylistSuggestionResponse(StrictAssistantModel):
    engine: str = Field(min_length=1, max_length=128)
    library_tracks: int
    eligible_tracks: int
    intent: PlaylistIntent
    plan: PlaylistPlan
    candidates: list[PlaylistCandidate]


MODEL_PLAYLIST_DISCLOSURE_VERSION: Literal[
    "assistant-playlist-model-disclosure/v2"
] = "assistant-playlist-model-disclosure/v2"


class ModelPlaylistDisclosure(StrictAssistantModel):
    version: Literal["assistant-playlist-model-disclosure/v2"]
    shared_with_provider: list[str]
    never_shared: list[str]
    maximum_candidates: int = Field(ge=1, le=100)
    may_incur_cost: bool


class ModelPlaylistAvailability(StrictAssistantModel):
    available: bool
    reason_code: str | None
    role_id: Literal["playlist_planner"]
    connection_name: str | None
    model_id: str | None
    quality_evaluation_id: Literal["playlist-quality-v1"]
    job_kind: str
    disclosure: ModelPlaylistDisclosure


class ModelPlaylistSuggestionStartRequest(StrictAssistantModel):
    request: PlaylistSuggestionRequest
    disclosure_version: Literal["assistant-playlist-model-disclosure/v2"]
    consent: Literal[True]


class ModelPlaylistSuggestionJobResult(StrictAssistantModel):
    schema_version: Literal["assistant-playlist-suggestion-job-result/v1"]
    disclosure_version: Literal["assistant-playlist-model-disclosure/v2"]
    role_id: Literal["playlist_planner"]
    role_fingerprint: str = Field(pattern=r"^[a-f0-9]{64}$")
    suggestion: PlaylistSuggestionResponse
    usage: ProviderUsageSummary


MODEL_EQ_DISCLOSURE_VERSION: Literal["assistant-eq-draft-disclosure/v2"] = (
    "assistant-eq-draft-disclosure/v2"
)


class EqDraftRequest(StrictAssistantModel):
    name: str = Field(min_length=1, max_length=128)
    goal: str = Field(min_length=2, max_length=1000)

    @field_validator("name", "goal", mode="before")
    @classmethod
    def normalize_text(cls, value: object) -> object:
        return value.strip() if isinstance(value, str) else value


class ModelEqDisclosure(StrictAssistantModel):
    version: Literal["assistant-eq-draft-disclosure/v2"]
    shared_with_provider: list[str]
    never_shared: list[str]
    may_incur_cost: bool


class ModelEqAvailability(StrictAssistantModel):
    available: bool
    reason_code: str | None
    role_id: Literal["eq_assistant"]
    connection_name: str | None
    model_id: str | None
    quality_evaluation_id: Literal["eq-quality-v1"]
    job_kind: str
    disclosure: ModelEqDisclosure


class ModelEqDraftStartRequest(StrictAssistantModel):
    request: EqDraftRequest
    disclosure_version: Literal["assistant-eq-draft-disclosure/v2"]
    consent: Literal[True]


class ModelEqDraftJobResult(StrictAssistantModel):
    schema_version: Literal["assistant-eq-draft-job-result/v1"]
    disclosure_version: Literal["assistant-eq-draft-disclosure/v2"]
    role_id: Literal["eq_assistant"]
    role_fingerprint: str = Field(pattern=r"^[a-f0-9]{64}$")
    engine_id: Literal["model-graphic-eq/v2"]
    draft: EqPresetDraft
    usage: ProviderUsageSummary


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


class LibraryContextStartRequest(StrictAssistantModel):
    force: bool = False
    scope: ModelTaggingScope = Field(default_factory=ModelTaggingScope)


class VoiceAnalyzerStatus(StrictAssistantModel):
    analyzer_id: Literal["essentia-musicnn-voice/v1"]
    status: Literal["not_configured", "ready", "unavailable"]
    reason: Literal[
        "model_missing",
        "model_unreadable",
        "unsupported_model",
        "runtime_missing",
    ] | None
    model_filename: str
    model_sha256: str


class LibraryContextSummary(StrictAssistantModel):
    analyzer: Literal["local-context/v2"]
    voice_analyzer: VoiceAnalyzerStatus
    library_tracks: int = Field(ge=0)
    analyzed_tracks: int = Field(ge=0)
    full_tracks: int = Field(ge=0)
    partial_tracks: int = Field(ge=0)
    missing_tracks: int = Field(ge=0)
    failed_tracks: int = Field(ge=0)
    stale_tracks: int = Field(ge=0)
    high_confidence: int = Field(ge=0)
    medium_confidence: int = Field(ge=0)
    low_confidence: int = Field(ge=0)
    last_updated_at: datetime | None


class TrackContextDetail(StrictAssistantModel):
    track_id: int = Field(gt=0)
    title: str
    artist: str
    status: Literal["full", "partial", "missing", "stale", "failed"]
    analyzer_id: Literal["local-context/v2"]
    confidence: Literal["high", "medium", "low"] | None
    updated_at: datetime | None
    summary: dict[str, object] | None
    timeline: list[dict[str, float]]
    sections: list[dict[str, object]]
    technical: dict[str, object] | None
    stages: dict[str, object] | None
    error: str | None
