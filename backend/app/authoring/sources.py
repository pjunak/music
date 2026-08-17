"""Adapters that normalize modes and JSON documents into one import bundle."""
from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import PurePosixPath
from typing import TypeAlias

from sqlalchemy import select
from sqlalchemy.orm import Session

from app.authoring.schemas import (
    AuthoringImportDocumentV1,
    AuthoringImportIssue,
    AuthoringImportSource,
    AuthoringResourceKind,
)
from app.models.playlist import Playlist, PlaylistItem
from app.models.track import Track
from app.modes import loader as modes_loader
from app.modes.loader import CueSpec, InterruptSpec, SoundboardManifest
from app.presets.loader import PresetManifest


@dataclass(frozen=True)
class PlaylistTrackRef:
    path: str | None
    missing_label: str


@dataclass(frozen=True)
class PlaylistPayload:
    name: str
    category: str | None
    tracks: tuple[PlaylistTrackRef, ...]


ResourcePayload: TypeAlias = (
    PlaylistPayload | SoundboardManifest | InterruptSpec | PresetManifest | CueSpec
)


@dataclass(frozen=True)
class ImportResource:
    kind: AuthoringResourceKind
    resource_id: str
    name: str
    summary: str
    payload: ResourcePayload
    issues: tuple[AuthoringImportIssue, ...] = field(default_factory=tuple)

    @property
    def key(self) -> str:
        return f"{self.kind}:{self.resource_id}"


@dataclass(frozen=True)
class ImportBundle:
    source: AuthoringImportSource
    resources: tuple[ImportResource, ...]

    def by_key(self) -> dict[str, ImportResource]:
        return {resource.key: resource for resource in self.resources}


def _plural(count: int, singular: str) -> str:
    return f"{count} {singular}{'' if count == 1 else 's'}"


def _path_issue(path: str, *, label: str) -> AuthoringImportIssue | None:
    parsed = PurePosixPath(path)
    if (
        "\\" in path
        or any(ord(character) < 32 for character in path)
        or parsed.is_absolute()
        or (parsed.parts and ":" in parsed.parts[0])
        or any(part in {"", ".", ".."} for part in parsed.parts)
        or str(parsed) != path
    ):
        return AuthoringImportIssue(
            code="invalid_path",
            severity="error",
            message=f"{label} must be a canonical relative path using forward slashes: {path}",
        )
    return None


def _playlist_track_refs(db: Session, playlist_id: int) -> tuple[PlaylistTrackRef, ...]:
    rows = db.execute(
        select(PlaylistItem.track_id, Track.path)
        .outerjoin(Track, Track.id == PlaylistItem.track_id)
        .where(PlaylistItem.playlist_id == playlist_id)
        .order_by(PlaylistItem.position)
    ).all()
    return tuple(
        PlaylistTrackRef(
            path=row.path,
            missing_label=row.path or f"track-id:{row.track_id}",
        )
        for row in rows
    )


def bundle_from_mode(db: Session, mode_id: str) -> ImportBundle:
    mode = modes_loader.get_mode(mode_id)
    if mode is None:
        raise LookupError(f"mode '{mode_id}' not loaded")

    resources: list[ImportResource] = []
    playlists = db.scalars(
        select(Playlist)
        .where(Playlist.mode_id == mode_id)
        .order_by(Playlist.name, Playlist.id)
    ).all()
    for playlist in playlists:
        tracks = _playlist_track_refs(db, playlist.id)
        summary = _plural(len(tracks), "track")
        if playlist.category:
            summary += f" · {playlist.category}"
        resources.append(
            ImportResource(
                kind="playlist",
                resource_id=str(playlist.id),
                name=playlist.name,
                summary=summary,
                payload=PlaylistPayload(
                    name=playlist.name,
                    category=playlist.category,
                    tracks=tracks,
                ),
            )
        )

    for soundboard_id, soundboard in sorted(mode.soundboards.items()):
        resources.append(
            ImportResource(
                kind="soundboard",
                resource_id=soundboard_id,
                name=soundboard.name or soundboard_id,
                summary=_plural(
                    sum(len(category.items) for category in soundboard.categories),
                    "sound",
                ),
                payload=soundboard,
            )
        )
    for index, interrupt in enumerate(mode.interrupts):
        detail = (
            f"Playlist · {interrupt.playlist}"
            if interrupt.playlist
            else f"Sound · {interrupt.soundboard_item or 'missing reference'}"
        )
        resources.append(
            ImportResource(
                kind="interrupt",
                resource_id=str(index),
                name=interrupt.name,
                summary=detail,
                payload=interrupt,
            )
        )
    for preset_id, preset in sorted(mode.presets.items()):
        resources.append(
            ImportResource(
                kind="preset",
                resource_id=preset_id,
                name=preset.name,
                summary=_plural(len(preset.effects), "effect"),
                payload=preset,
            )
        )
    for cue_id, cue in sorted(mode.cues.items()):
        actions = sum(
            (1 if cue.preset else 0, 1 if cue.playlist else 0, len(cue.sfx), len(cue.loops))
        )
        resources.append(
            ImportResource(
                kind="cue",
                resource_id=cue_id,
                name=cue.name,
                summary=_plural(actions, "action"),
                payload=cue,
            )
        )

    return ImportBundle(
        source=AuthoringImportSource(type="mode", id=mode.id, name=mode.name),
        resources=tuple(resources),
    )


def bundle_from_document(
    document: AuthoringImportDocumentV1, source_name: str | None
) -> ImportBundle:
    resources: list[ImportResource] = []

    for index, playlist in enumerate(document.playlists):
        issues = tuple(
            issue
            for path in playlist.tracks
            if (issue := _path_issue(path, label="Playlist track path")) is not None
        )
        resources.append(
            ImportResource(
                kind="playlist",
                resource_id=str(index),
                name=playlist.name,
                summary=(
                    _plural(len(playlist.tracks), "track")
                    + (f" · {playlist.category}" if playlist.category else "")
                ),
                payload=PlaylistPayload(
                    name=playlist.name,
                    category=playlist.category,
                    tracks=tuple(
                        PlaylistTrackRef(path=path, missing_label=path)
                        for path in playlist.tracks
                    ),
                ),
                issues=issues,
            )
        )

    for soundboard in document.soundboards:
        issues = tuple(
            issue
            for category in soundboard.categories
            for item in category.items
            if (issue := _path_issue(item.file, label="Soundboard item path")) is not None
        )
        soundboard_payload = SoundboardManifest.model_validate(
            soundboard.model_dump(exclude_none=True)
        )
        resources.append(
            ImportResource(
                kind="soundboard",
                resource_id=soundboard.id,
                name=soundboard.name or soundboard.id,
                summary=_plural(
                    sum(len(category.items) for category in soundboard.categories),
                    "sound",
                ),
                payload=soundboard_payload,
                issues=issues,
            )
        )

    for index, interrupt in enumerate(document.interrupts):
        interrupt_payload = InterruptSpec.model_validate(
            interrupt.model_dump(exclude_none=True)
        )
        detail = (
            f"Playlist · {interrupt.playlist}"
            if interrupt.playlist
            else f"Sound · {interrupt.soundboard_item}"
        )
        issues = ()
        if interrupt.soundboard_item:
            issue = _path_issue(interrupt.soundboard_item, label="Interrupt sound path")
            issues = (issue,) if issue else ()
        resources.append(
            ImportResource(
                kind="interrupt",
                resource_id=str(index),
                name=interrupt.name,
                summary=detail,
                payload=interrupt_payload,
                issues=issues,
            )
        )

    for preset in document.presets:
        preset_payload = PresetManifest.model_validate(
            preset.model_dump(exclude_none=True)
        )
        preset_issues: list[AuthoringImportIssue] = []
        for effect in preset_payload.effects:
            try:
                effect.validate_type()
            except ValueError as exc:
                preset_issues.append(
                    AuthoringImportIssue(
                        code="unsupported_effect",
                        severity="error",
                        message=str(exc),
                    )
                )
        resources.append(
            ImportResource(
                kind="preset",
                resource_id=preset.id,
                name=preset.name,
                summary=_plural(len(preset.effects), "effect"),
                payload=preset_payload,
                issues=tuple(preset_issues),
            )
        )

    for cue in document.cues:
        cue_payload = CueSpec.model_validate(cue.model_dump(exclude_none=True))
        cue_issues = tuple(
            issue
            for ref in [*cue.sfx, *cue.loops]
            if (issue := _path_issue(ref.item, label="Cue sound path")) is not None
        )
        actions = sum(
            (1 if cue.preset else 0, 1 if cue.playlist else 0, len(cue.sfx), len(cue.loops))
        )
        resources.append(
            ImportResource(
                kind="cue",
                resource_id=cue.id,
                name=cue.name,
                summary=_plural(actions, "action"),
                payload=cue_payload,
                issues=cue_issues,
            )
        )

    label = document.name or source_name or "JSON document"
    return ImportBundle(
        source=AuthoringImportSource(
            type="document", id=document.schema_version, name=label
        ),
        resources=tuple(resources),
    )
