from __future__ import annotations

from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

from app.assistant.providers.schemas import ProviderUsageSummary
from app.assistant.tags import normalize_manual_tags


class StrictTagModel(BaseModel):
    model_config = ConfigDict(extra="forbid")


class ManualTagPatch(StrictTagModel):
    add: list[str] = Field(default_factory=list)
    remove: list[str] = Field(default_factory=list)

    @field_validator("add", "remove")
    @classmethod
    def normalize_tags(cls, value: list[str]) -> list[str]:
        return list(normalize_manual_tags(value))

    @model_validator(mode="after")
    def disjoint_changes(self) -> ManualTagPatch:
        if overlap := set(self.add) & set(self.remove):
            raise ValueError(
                f"tags cannot be added and removed together: {min(overlap)}"
            )
        return self


class BulkManualTagPatch(ManualTagPatch):
    track_ids: list[int] = Field(min_length=1, max_length=5000)

    @field_validator("track_ids")
    @classmethod
    def normalize_track_ids(cls, value: list[int]) -> list[int]:
        if any(track_id <= 0 for track_id in value):
            raise ValueError("track_ids must contain only positive IDs")
        return list(dict.fromkeys(value))


class BulkManualTagFailure(StrictTagModel):
    track_id: int
    error: str


class BulkManualTagResult(StrictTagModel):
    requested_tracks: int = Field(ge=0)
    matched_tracks: int = Field(ge=0)
    changed_track_ids: list[int]
    missing_track_ids: list[int]
    failures: list[BulkManualTagFailure]


class StarterTagGroupOut(StrictTagModel):
    key: str
    label: str
    tags: list[str]


class ManualTagUsage(StrictTagModel):
    tag: str
    track_count: int = Field(ge=1)


class ManualTagCatalog(StrictTagModel):
    starter_groups: list[StarterTagGroupOut]
    used_tags: list[str]
    tag_usage: list[ManualTagUsage]


class ManualTagRenameRequest(StrictTagModel):
    source: str
    target: str

    @field_validator("source", "target")
    @classmethod
    def normalize_tag(cls, value: str) -> str:
        return normalize_manual_tags([value])[0]

    @model_validator(mode="after")
    def different_tags(self) -> ManualTagRenameRequest:
        if self.source == self.target:
            raise ValueError("source and target tags must be different")
        return self


class ManualTagRenameResult(StrictTagModel):
    source: str
    target: str
    affected_tracks: int = Field(ge=1)
    merged: bool


class TagCleanupSuggestionOut(StrictTagModel):
    id: str = Field(pattern=r"^[a-f0-9]{64}$")
    source: str
    target: str
    reason_code: Literal["starter_plural", "starter_typo"]
    reason: str
    source_track_count: int = Field(ge=1)
    target_track_count: int = Field(ge=0)
    merged: bool


class TagCleanupPreviewOut(StrictTagModel):
    schema_version: Literal["assistant-tag-cleanup-preview/v1"]
    catalog_signature: str = Field(pattern=r"^[a-f0-9]{64}$")
    suggestions: list[TagCleanupSuggestionOut]


class TagCleanupSelectionIn(StrictTagModel):
    source: str
    target: str

    @field_validator("source", "target")
    @classmethod
    def normalize_tag(cls, value: str) -> str:
        return normalize_manual_tags([value])[0]

    @model_validator(mode="after")
    def different_tags(self) -> TagCleanupSelectionIn:
        if self.source == self.target:
            raise ValueError("source and target tags must be different")
        return self


class TagCleanupApplyRequest(StrictTagModel):
    catalog_signature: str = Field(pattern=r"^[a-f0-9]{64}$")
    items: list[TagCleanupSelectionIn] = Field(min_length=1, max_length=100)

    @model_validator(mode="after")
    def unique_sources(self) -> TagCleanupApplyRequest:
        sources = {item.source for item in self.items}
        if len(sources) != len(self.items):
            raise ValueError("cleanup sources must be unique")
        return self


class TagCleanupApplyResult(StrictTagModel):
    schema_version: Literal["assistant-tag-cleanup-apply/v1"]
    requested_items: int = Field(ge=1)
    applied: list[ManualTagRenameResult]
    catalog_signature: str = Field(pattern=r"^[a-f0-9]{64}$")


MODEL_TAG_CLEANUP_DISCLOSURE_VERSION: Literal[
    "assistant-model-tag-cleanup-disclosure/v2"
] = "assistant-model-tag-cleanup-disclosure/v2"


class ModelTagCleanupDisclosure(StrictTagModel):
    version: Literal["assistant-model-tag-cleanup-disclosure/v2"]
    shared_with_provider: list[str]
    never_shared: list[str]
    maximum_tags: int = Field(ge=1, le=500)
    may_incur_cost: bool


class ModelTagCleanupAvailability(StrictTagModel):
    available: bool
    reason_code: str | None
    role_id: Literal["tag_cleanup"]
    connection_name: str | None
    model_id: str | None
    quality_evaluation_id: Literal["tag-cleanup-quality-v1"]
    job_kind: str
    catalog_signature: str = Field(pattern=r"^[a-f0-9]{64}$")
    manual_tags: int = Field(ge=0)
    estimated_provider_requests: int = Field(ge=0, le=1)
    disclosure: ModelTagCleanupDisclosure


class ModelTagCleanupStartRequest(StrictTagModel):
    disclosure_version: Literal["assistant-model-tag-cleanup-disclosure/v2"]
    consent: Literal[True]


class ModelTagCleanupSuggestionOut(StrictTagModel):
    id: str = Field(pattern=r"^[a-f0-9]{64}$")
    source: str
    target: str
    origin: Literal["local-rule", "model"]
    confidence: Literal["high", "medium", "low"]
    reason: str = Field(min_length=1, max_length=512)
    source_track_count: int = Field(ge=1)
    target_track_count: int = Field(ge=0)
    merged: bool


class ModelTagCleanupJobResult(StrictTagModel):
    schema_version: Literal["assistant-model-tag-cleanup-job-result/v2"]
    disclosure_version: Literal["assistant-model-tag-cleanup-disclosure/v2"]
    role_id: Literal["tag_cleanup"]
    role_fingerprint: str = Field(pattern=r"^[a-f0-9]{64}$")
    engine_id: Literal["model-tag-cleanup/v2"]
    catalog_signature: str = Field(pattern=r"^[a-f0-9]{64}$")
    catalog_tags: int = Field(ge=1, le=500)
    suggestions: list[ModelTagCleanupSuggestionOut] = Field(max_length=100)
    usage: ProviderUsageSummary


class ModelTagCleanupApplyRequest(TagCleanupApplyRequest):
    job_id: str = Field(min_length=1, max_length=32)


class AnalysisTagSuggestionOut(StrictTagModel):
    tag: str
    analyzer_id: str
    source_signature: str
    confidence: Literal["high", "medium", "low"]
    evidence: list[str]
    status: Literal["pending", "accepted", "rejected"]


class AnalysisTagReviewTargetIn(StrictTagModel):
    tag: str
    analyzer_id: str = Field(min_length=1, max_length=128)
    source_signature: str = Field(min_length=1, max_length=128)

    @field_validator("tag")
    @classmethod
    def normalize_tag(cls, value: str) -> str:
        return normalize_manual_tags([value])[0]


class AnalysisTagReviewRequest(AnalysisTagReviewTargetIn):
    decision: Literal["pending", "accepted", "rejected"]


class AnalysisTagReviewResult(StrictTagModel):
    track_id: int
    tag: str
    analyzer_id: str
    source_signature: str
    decision: Literal["pending", "accepted", "rejected"]
    manual_tags: list[str]


class BulkAnalysisTagReviewItem(AnalysisTagReviewTargetIn):
    track_id: int = Field(gt=0)


class BulkAnalysisTagReviewRequest(StrictTagModel):
    items: list[BulkAnalysisTagReviewItem] = Field(min_length=1, max_length=1000)
    decision: Literal["accepted", "rejected"]

    @model_validator(mode="after")
    def unique_items(self) -> BulkAnalysisTagReviewRequest:
        keys = {
            (item.track_id, item.analyzer_id, item.source_signature, item.tag)
            for item in self.items
        }
        if len(keys) != len(self.items):
            raise ValueError("items must not contain duplicate suggestions")
        return self


class AnalysisTagReviewTargetOut(StrictTagModel):
    track_id: int
    tag: str
    analyzer_id: str
    source_signature: str


class BulkAnalysisTagReviewApplied(AnalysisTagReviewTargetOut):
    decision: Literal["accepted", "rejected"]


class BulkAnalysisTagReviewFailure(AnalysisTagReviewTargetOut):
    code: Literal["not_found", "stale", "tag_limit"]
    error: str


class BulkAnalysisTagReviewResult(StrictTagModel):
    requested_items: int = Field(ge=0)
    applied: list[BulkAnalysisTagReviewApplied]
    failures: list[BulkAnalysisTagReviewFailure]


class AudioSignalProfileOut(StrictTagModel):
    analyzer_id: str
    confidence: Literal["high", "medium", "low"]
    evidence: list[str]
    metrics: dict[str, str | int | float | None]


class LibraryTagTrack(StrictTagModel):
    track_id: int
    path: str
    title: str
    display_title: str
    artist: str
    album: str
    manual_tags: list[str]
    analysis_analyzer: str | None
    analysis_tags: list[str]
    analysis_confidence: Literal["high", "medium", "low"] | None
    analysis_suggestions: list[AnalysisTagSuggestionOut]
    audio_signal: AudioSignalProfileOut | None


class LibraryTagPage(StrictTagModel):
    items: list[LibraryTagTrack]
    total: int = Field(ge=0)
    offset: int = Field(ge=0)
    limit: int = Field(ge=1)


MODEL_TAGGING_DISCLOSURE_VERSION: Literal[
    "assistant-model-music-tagging-disclosure/v3"
] = "assistant-model-music-tagging-disclosure/v3"


class ModelTaggingDisclosure(StrictTagModel):
    version: Literal["assistant-model-music-tagging-disclosure/v3"]
    shared_with_provider: list[str]
    never_shared: list[str]
    allowed_tags: list[str]
    tracks_per_request: int = Field(ge=1, le=20)
    may_incur_cost: bool


class ModelTaggingAvailability(StrictTagModel):
    available: bool
    reason_code: str | None
    role_id: Literal["music_tagger"]
    connection_name: str | None
    model_id: str | None
    quality_evaluation_id: Literal["music-tagging-quality-v1"]
    job_kind: str
    library_tracks: int = Field(ge=0)
    tracks_with_audio_evidence: int = Field(ge=0)
    current_profiles: int = Field(ge=0)
    tracks_needing_tags: int = Field(ge=0)
    estimated_provider_requests: int = Field(ge=0)
    disclosure: ModelTaggingDisclosure


class ModelTaggingStartRequest(StrictTagModel):
    force: bool = False
    disclosure_version: Literal["assistant-model-music-tagging-disclosure/v3"]
    consent: Literal[True]


class ModelTaggingJobResult(StrictTagModel):
    schema_version: Literal["assistant-model-music-tagging-job-result/v3"]
    disclosure_version: Literal["assistant-model-music-tagging-disclosure/v3"]
    role_id: Literal["music_tagger"]
    role_fingerprint: str = Field(pattern=r"^[a-f0-9]{64}$")
    analyzer_id: Literal["model-evidence-tagger/v3"]
    library_tracks: int = Field(ge=0)
    updated_profiles: int = Field(ge=0)
    unchanged_profiles: int = Field(ge=0)
    skipped_changed_tracks: int = Field(ge=0)
    usage: ProviderUsageSummary
