"""Public contracts for review-first Authoring imports.

The document format is deliberately versioned and JSON-shaped.  These models
validate its structural contract; source adapters and the planner add semantic
checks that are more useful as per-item review issues than as one opaque 422.
"""
from typing import Annotated, Any, Literal

from pydantic import BaseModel, ConfigDict, Field, model_validator

SLUG_PATTERN = r"^[a-z0-9][a-z0-9_-]*$"
Slug = Annotated[str, Field(min_length=1, max_length=64, pattern=SLUG_PATTERN)]

AuthoringResourceKind = Literal[
    "playlist", "soundboard", "interrupt", "preset", "cue"
]
ImportItemStatus = Literal["ready", "conflict", "invalid"]
ImportIssueSeverity = Literal["warning", "error"]


class StrictImportModel(BaseModel):
    model_config = ConfigDict(extra="forbid")


class AuthoringImportMode(StrictImportModel):
    id: str
    name: str


class AuthoringImportSource(StrictImportModel):
    type: Literal["mode", "document"]
    id: str
    name: str


class AuthoringImportSelection(StrictImportModel):
    kind: AuthoringResourceKind
    resource_id: str = Field(min_length=1, max_length=128)


class AuthoringImportIssue(StrictImportModel):
    code: str
    severity: ImportIssueSeverity
    message: str
    related_item: AuthoringImportSelection | None = None


class AuthoringImportItem(StrictImportModel):
    kind: AuthoringResourceKind
    resource_id: str
    name: str
    summary: str
    status: ImportItemStatus
    reason: str | None = None
    issues: list[AuthoringImportIssue] = Field(default_factory=list)


class AuthoringImportPreview(StrictImportModel):
    source: AuthoringImportSource
    # Kept for clients of the original mode-to-mode endpoint. New consumers
    # should use `source`, which also represents JSON documents.
    source_mode: AuthoringImportMode | None = None
    target_mode: AuthoringImportMode
    items: list[AuthoringImportItem]


class AuthoringImportResult(StrictImportModel):
    imported: list[AuthoringImportItem]
    skipped: list[AuthoringImportItem]
    missing_track_paths: list[str]


class AuthoringImportPreviewRequest(StrictImportModel):
    source_mode_id: str = Field(min_length=1, max_length=64)
    target_mode_id: str = Field(min_length=1, max_length=64)


class AuthoringImportCommitRequest(AuthoringImportPreviewRequest):
    items: list[AuthoringImportSelection] = Field(min_length=1, max_length=500)


class ImportPlaylist(StrictImportModel):
    name: str = Field(min_length=1, max_length=256)
    category: str | None = Field(default=None, max_length=64)
    tracks: list[Annotated[str, Field(min_length=1, max_length=1024)]] = Field(
        default_factory=list, max_length=10_000
    )


class ImportSoundboardItem(StrictImportModel):
    file: str = Field(min_length=1, max_length=1024)
    name: str = Field(min_length=1, max_length=128)
    icon: str | None = Field(default=None, max_length=16)
    hotkey: str | None = Field(default=None, max_length=16)


class ImportSoundboardCategory(StrictImportModel):
    id: Slug
    name: str = Field(min_length=1, max_length=128)
    items: list[ImportSoundboardItem] = Field(default_factory=list, max_length=1000)


class ImportSoundboard(StrictImportModel):
    id: Slug
    name: str | None = Field(default=None, max_length=128)
    categories: list[ImportSoundboardCategory] = Field(
        default_factory=list, max_length=100
    )

    @model_validator(mode="after")
    def unique_category_ids(self) -> ImportSoundboard:
        ids = [category.id for category in self.categories]
        if len(ids) != len(set(ids)):
            raise ValueError(f"soundboard '{self.id}' has duplicate category IDs")
        return self


class ImportInterrupt(StrictImportModel):
    name: str = Field(min_length=1, max_length=128)
    playlist: str | None = Field(default=None, max_length=256)
    soundboard_item: str | None = Field(default=None, max_length=1024)
    fade_in_ms: int = Field(default=0, ge=0, le=60_000)
    fade_out_ms: int = Field(default=0, ge=0, le=60_000)
    return_to_ambient: bool = True
    duck_to: float | None = Field(default=None, ge=0.0, le=1.0)

    @model_validator(mode="after")
    def exactly_one_source(self) -> ImportInterrupt:
        if bool(self.playlist) == bool(self.soundboard_item):
            raise ValueError(
                "interrupt must reference exactly one of playlist or soundboard_item"
            )
        return self


class ImportEffect(BaseModel):
    """Phase 1 preserves current effect parameters; phase 2 makes them strict."""

    type: str = Field(min_length=1, max_length=64)
    model_config = ConfigDict(extra="allow")


class ImportPreset(StrictImportModel):
    id: Slug
    name: str = Field(min_length=1, max_length=128)
    description: str | None = Field(default=None, max_length=2000)
    effects: list[ImportEffect] = Field(default_factory=list, max_length=32)
    crossfade_ms: int | None = Field(default=None, ge=0, le=60_000)


class ImportCueSfx(StrictImportModel):
    soundboard: Slug
    item: str = Field(min_length=1, max_length=1024)
    volume: float = Field(default=1.0, ge=0.0, le=1.0)


class ImportCueLoop(ImportCueSfx):
    interval_s: float = Field(ge=1.0, le=3600.0)


class ImportCue(StrictImportModel):
    id: Slug
    name: str = Field(min_length=1, max_length=128)
    description: str | None = Field(default=None, max_length=2000)
    preset: Slug | None = None
    playlist: str | None = Field(default=None, max_length=256)
    start_index: int = Field(default=0, ge=0, le=100_000)
    start_ms: int = Field(default=0, ge=0)
    sfx: list[ImportCueSfx] = Field(default_factory=list, max_length=500)
    loops: list[ImportCueLoop] = Field(default_factory=list, max_length=500)


class AuthoringImportDocumentV1(StrictImportModel):
    schema_version: Literal["authoring-import/v1"] = Field(alias="schema")
    name: str | None = Field(default=None, max_length=128)
    playlists: list[ImportPlaylist] = Field(default_factory=list, max_length=500)
    soundboards: list[ImportSoundboard] = Field(default_factory=list, max_length=500)
    interrupts: list[ImportInterrupt] = Field(default_factory=list, max_length=500)
    presets: list[ImportPreset] = Field(default_factory=list, max_length=500)
    cues: list[ImportCue] = Field(default_factory=list, max_length=500)

    @model_validator(mode="after")
    def bounded_and_unambiguous(self) -> AuthoringImportDocumentV1:
        resource_count = sum(
            len(items)
            for items in (
                self.playlists,
                self.soundboards,
                self.interrupts,
                self.presets,
                self.cues,
            )
        )
        if resource_count == 0:
            raise ValueError("document contains no Authoring resources")
        if resource_count > 500:
            raise ValueError("document contains more than 500 Authoring resources")
        if sum(len(playlist.tracks) for playlist in self.playlists) > 20_000:
            raise ValueError("document contains more than 20,000 playlist track references")
        sound_count = sum(
            len(category.items)
            for soundboard in self.soundboards
            for category in soundboard.categories
        )
        if sound_count > 20_000:
            raise ValueError("document contains more than 20,000 soundboard items")
        cue_action_count = sum(
            len(cue.sfx) + len(cue.loops) for cue in self.cues
        )
        if cue_action_count > 20_000:
            raise ValueError("document contains more than 20,000 cue sound actions")

        duplicate_checks = (
            ("playlist names", [item.name for item in self.playlists]),
            ("interrupt names", [item.name for item in self.interrupts]),
            ("soundboard IDs", [item.id for item in self.soundboards]),
            ("preset IDs", [item.id for item in self.presets]),
            ("cue IDs", [item.id for item in self.cues]),
        )
        for label, values in duplicate_checks:
            if len(values) != len(set(values)):
                raise ValueError(f"document contains duplicate {label}")
        return self


class AuthoringDocumentPreviewRequest(StrictImportModel):
    target_mode_id: str = Field(min_length=1, max_length=64)
    source_name: str | None = Field(default=None, max_length=255)
    document: AuthoringImportDocumentV1


class AuthoringDocumentCommitRequest(AuthoringDocumentPreviewRequest):
    items: list[AuthoringImportSelection] = Field(min_length=1, max_length=500)


def public_document_schema() -> dict[str, Any]:
    """Return the exact v1 JSON Schema for docs and future tool adapters."""

    return AuthoringImportDocumentV1.model_json_schema(by_alias=True)
