from __future__ import annotations

from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

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


class LibraryTagPage(StrictTagModel):
    items: list[LibraryTagTrack]
    total: int = Field(ge=0)
    offset: int = Field(ge=0)
    limit: int = Field(ge=1)
