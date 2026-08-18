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


class StarterTagGroupOut(StrictTagModel):
    key: str
    label: str
    tags: list[str]


class ManualTagCatalog(StrictTagModel):
    starter_groups: list[StarterTagGroupOut]
    used_tags: list[str]


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
