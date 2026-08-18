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


class LibraryTagPage(StrictTagModel):
    items: list[LibraryTagTrack]
    total: int = Field(ge=0)
    offset: int = Field(ge=0)
    limit: int = Field(ge=1)
