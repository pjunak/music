"""Review-first imports between mode-scoped Authoring collections.

The first import contract is deliberately create-only.  A preview reports
name/id collisions and the commit endpoint re-runs that preview while holding
the import lock, so an existing target resource is never overwritten by a
stale review.  Playlists live in SQLite while the other resources live in the
target mode directory; commit therefore stages every change, validates the
reloaded mode, and rolls both stores back if either side fails.
"""
from __future__ import annotations

import logging
import os
import tempfile
from pathlib import Path
from threading import Lock
from typing import Literal

import yaml
from fastapi import APIRouter, HTTPException, status
from pydantic import BaseModel, Field
from sqlalchemy import select

from app.api.deps import CurrentUser, DbSession
from app.models.playlist import Playlist, PlaylistItem
from app.models.track import Track
from app.modes import loader as modes_loader

logger = logging.getLogger(__name__)

router = APIRouter(prefix="/api/authoring/import", tags=["authoring"])
_import_lock = Lock()

AuthoringResourceKind = Literal[
    "playlist", "soundboard", "interrupt", "preset", "cue"
]
ImportItemStatus = Literal["ready", "conflict"]


class AuthoringImportMode(BaseModel):
    id: str
    name: str


class AuthoringImportItem(BaseModel):
    kind: AuthoringResourceKind
    resource_id: str
    name: str
    summary: str
    status: ImportItemStatus
    reason: str | None = None


class AuthoringImportPreviewRequest(BaseModel):
    source_mode_id: str = Field(min_length=1, max_length=64)
    target_mode_id: str = Field(min_length=1, max_length=64)


class AuthoringImportPreview(BaseModel):
    source_mode: AuthoringImportMode
    target_mode: AuthoringImportMode
    items: list[AuthoringImportItem]


class AuthoringImportSelection(BaseModel):
    kind: AuthoringResourceKind
    resource_id: str = Field(min_length=1, max_length=64)


class AuthoringImportCommitRequest(AuthoringImportPreviewRequest):
    items: list[AuthoringImportSelection] = Field(min_length=1, max_length=500)


class AuthoringImportResult(BaseModel):
    imported: list[AuthoringImportItem]
    skipped: list[AuthoringImportItem]
    missing_track_paths: list[str]


def _mode_or_404(mode_id: str) -> modes_loader.ModeManifest:
    mode = modes_loader.get_mode(mode_id)
    if mode is None:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail=f"mode '{mode_id}' not loaded",
        )
    return mode


def _validate_mode_pair(
    source_mode_id: str, target_mode_id: str
) -> tuple[modes_loader.ModeManifest, modes_loader.ModeManifest]:
    if source_mode_id == target_mode_id:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="source and target modes must be different",
        )
    return (_mode_or_404(source_mode_id), _mode_or_404(target_mode_id))


def _mode_ref(mode: modes_loader.ModeManifest) -> AuthoringImportMode:
    return AuthoringImportMode(id=mode.id, name=mode.name)


def _plural(count: int, singular: str) -> str:
    return f"{count} {singular}{'' if count == 1 else 's'}"


def _playlist_track_refs(
    db: DbSession, playlist_id: int
) -> list[tuple[int, int, str | None]]:
    """Return source playlist rows in playback order without healing/mutating it."""

    rows = db.execute(
        select(PlaylistItem.position, PlaylistItem.track_id, Track.path)
        .outerjoin(Track, Track.id == PlaylistItem.track_id)
        .where(PlaylistItem.playlist_id == playlist_id)
        .order_by(PlaylistItem.position)
    ).all()
    return [(row.position, row.track_id, row.path) for row in rows]


def _candidate(
    *,
    kind: AuthoringResourceKind,
    resource_id: str,
    name: str,
    summary: str,
    conflict: bool,
    reason: str,
) -> AuthoringImportItem:
    return AuthoringImportItem(
        kind=kind,
        resource_id=resource_id,
        name=name,
        summary=summary,
        status="conflict" if conflict else "ready",
        reason=reason if conflict else None,
    )


def _build_preview(
    db: DbSession, source_mode_id: str, target_mode_id: str
) -> AuthoringImportPreview:
    source, target = _validate_mode_pair(source_mode_id, target_mode_id)
    items: list[AuthoringImportItem] = []

    target_playlist_names = set(
        db.scalars(
            select(Playlist.name).where(Playlist.mode_id == target_mode_id)
        ).all()
    )
    seen_source_playlist_names: set[str] = set()
    source_playlists = list(
        db.scalars(
            select(Playlist)
            .where(Playlist.mode_id == source_mode_id)
            .order_by(Playlist.name, Playlist.id)
        ).all()
    )
    for playlist in source_playlists:
        track_count = len(_playlist_track_refs(db, playlist.id))
        detail = _plural(track_count, "track")
        if playlist.category:
            detail += f" · {playlist.category}"
        duplicate_in_source = playlist.name in seen_source_playlist_names
        conflict = playlist.name in target_playlist_names or duplicate_in_source
        reason = (
            "Another source playlist has the same name."
            if duplicate_in_source
            else "A playlist with this name already exists in the target mode."
        )
        items.append(
            _candidate(
                kind="playlist",
                resource_id=str(playlist.id),
                name=playlist.name,
                summary=detail,
                conflict=conflict,
                reason=reason,
            )
        )
        seen_source_playlist_names.add(playlist.name)

    for soundboard_id, soundboard in sorted(source.soundboards.items()):
        item_count = sum(len(category.items) for category in soundboard.categories)
        items.append(
            _candidate(
                kind="soundboard",
                resource_id=soundboard_id,
                name=soundboard.name or soundboard_id,
                summary=_plural(item_count, "sound"),
                conflict=soundboard_id in target.soundboards,
                reason="A soundboard with this ID already exists in the target mode.",
            )
        )

    target_interrupt_names = {interrupt.name for interrupt in target.interrupts}
    seen_source_interrupt_names: set[str] = set()
    for index, interrupt in enumerate(source.interrupts):
        duplicate_in_source = interrupt.name in seen_source_interrupt_names
        conflict = interrupt.name in target_interrupt_names or duplicate_in_source
        if interrupt.playlist:
            detail = f"Playlist · {interrupt.playlist}"
        else:
            detail = f"Sound · {interrupt.soundboard_item or 'missing reference'}"
        reason = (
            "Another source interrupt has the same name."
            if duplicate_in_source
            else "An interrupt with this name already exists in the target mode."
        )
        items.append(
            _candidate(
                kind="interrupt",
                resource_id=str(index),
                name=interrupt.name,
                summary=detail,
                conflict=conflict,
                reason=reason,
            )
        )
        seen_source_interrupt_names.add(interrupt.name)

    for preset_id, preset in sorted(source.presets.items()):
        items.append(
            _candidate(
                kind="preset",
                resource_id=preset_id,
                name=preset.name,
                summary=_plural(len(preset.effects), "effect"),
                conflict=preset_id in target.presets,
                reason="An EQ preset with this ID already exists in the target mode.",
            )
        )

    for cue_id, cue in sorted(source.cues.items()):
        parts = sum(
            (
                1 if cue.preset else 0,
                1 if cue.playlist else 0,
                len(cue.sfx),
                len(cue.loops),
            )
        )
        items.append(
            _candidate(
                kind="cue",
                resource_id=cue_id,
                name=cue.name,
                summary=_plural(parts, "action"),
                conflict=cue_id in target.cues,
                reason="A cue with this ID already exists in the target mode.",
            )
        )

    return AuthoringImportPreview(
        source_mode=_mode_ref(source),
        target_mode=_mode_ref(target),
        items=items,
    )


def _selection_key(kind: AuthoringResourceKind, resource_id: str) -> str:
    return f"{kind}:{resource_id}"


def _write_yaml(path: Path, payload: dict) -> None:
    """Atomically create/replace one validated Authoring YAML document."""

    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temp_name = tempfile.mkstemp(
        dir=path.parent, prefix=f".{path.name}.", suffix=".tmp", text=True
    )
    temp_path = Path(temp_name)
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as temp_file:
            yaml.safe_dump(payload, temp_file, sort_keys=False)
            temp_file.flush()
            os.fsync(temp_file.fileno())
        os.replace(temp_path, path)
    except BaseException:
        temp_path.unlink(missing_ok=True)
        raise


def _write_bytes(path: Path, payload: bytes) -> None:
    """Atomically restore an exact pre-import file during rollback."""

    fd, temp_name = tempfile.mkstemp(
        dir=path.parent, prefix=f".{path.name}.", suffix=".rollback", text=False
    )
    temp_path = Path(temp_name)
    try:
        with os.fdopen(fd, "wb") as temp_file:
            temp_file.write(payload)
            temp_file.flush()
            os.fsync(temp_file.fileno())
        os.replace(temp_path, path)
    except BaseException:
        temp_path.unlink(missing_ok=True)
        raise


def _target_manifest(mode: modes_loader.ModeManifest) -> tuple[Path, dict, bytes]:
    if mode.root_dir is None:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"mode '{mode.id}' has no resolved root directory",
        )
    path = mode.root_dir / "manifest.yaml"
    original = path.read_bytes()
    raw = yaml.safe_load(original.decode("utf-8")) or {}
    if not isinstance(raw, dict):
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"manifest.yaml for mode '{mode.id}' is not a mapping",
        )
    raw.setdefault("playlist_categories", [])
    raw.setdefault("interrupts", [])
    return (path, raw, original)


@router.post("/preview", response_model=AuthoringImportPreview)
def preview_authoring_import(
    payload: AuthoringImportPreviewRequest,
    _user: CurrentUser,
    db: DbSession,
) -> AuthoringImportPreview:
    return _build_preview(db, payload.source_mode_id, payload.target_mode_id)


@router.post("/commit", response_model=AuthoringImportResult)
def commit_authoring_import(
    payload: AuthoringImportCommitRequest,
    _user: CurrentUser,
    db: DbSession,
) -> AuthoringImportResult:
    with _import_lock:
        preview = _build_preview(db, payload.source_mode_id, payload.target_mode_id)
        available = {
            _selection_key(item.kind, item.resource_id): item for item in preview.items
        }

        requested_keys: list[str] = []
        seen_keys: set[str] = set()
        for selection in payload.items:
            key = _selection_key(selection.kind, selection.resource_id)
            if key in seen_keys:
                continue
            if key not in available:
                raise HTTPException(
                    status_code=status.HTTP_400_BAD_REQUEST,
                    detail=f"source resource '{key}' is no longer available",
                )
            seen_keys.add(key)
            requested_keys.append(key)

        requested = [available[key] for key in requested_keys]
        imported = [item for item in requested if item.status == "ready"]
        skipped = [item for item in requested if item.status == "conflict"]
        if not imported:
            return AuthoringImportResult(
                imported=[], skipped=skipped, missing_track_paths=[]
            )

        source, target = _validate_mode_pair(
            payload.source_mode_id, payload.target_mode_id
        )
        if target.root_dir is None:
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"mode '{target.id}' has no resolved root directory",
            )

        source_playlists = {
            str(playlist.id): playlist
            for playlist in db.scalars(
                select(Playlist).where(Playlist.mode_id == source.id)
            ).all()
        }
        file_payloads: list[tuple[Path, dict]] = []
        manifest_path, manifest, original_manifest = _target_manifest(target)
        manifest_changed = False
        missing_track_paths: list[str] = []

        for item in imported:
            if item.kind == "playlist":
                source_playlist = source_playlists[item.resource_id]
                cloned = Playlist(
                    name=source_playlist.name,
                    mode_id=target.id,
                    category=source_playlist.category,
                )
                db.add(cloned)
                db.flush()
                next_position = 0
                for _position, source_track_id, track_path in _playlist_track_refs(
                    db, source_playlist.id
                ):
                    if track_path is None:
                        missing_track_paths.append(f"track-id:{source_track_id}")
                        continue
                    target_track = db.scalar(select(Track).where(Track.path == track_path))
                    if target_track is None:
                        missing_track_paths.append(track_path)
                        continue
                    db.add(
                        PlaylistItem(
                            playlist_id=cloned.id,
                            position=next_position,
                            track_id=target_track.id,
                        )
                    )
                    next_position += 1
                if (
                    source_playlist.category
                    and source_playlist.category
                    not in manifest["playlist_categories"]
                ):
                    manifest["playlist_categories"].append(source_playlist.category)
                    manifest_changed = True
            elif item.kind == "soundboard":
                soundboard = source.soundboards[item.resource_id]
                file_payloads.append(
                    (
                        target.root_dir / "soundboards" / f"{item.resource_id}.yaml",
                        soundboard.model_dump(exclude_none=True),
                    )
                )
            elif item.kind == "interrupt":
                interrupt = source.interrupts[int(item.resource_id)]
                manifest["interrupts"].append(interrupt.model_dump(exclude_none=True))
                manifest_changed = True
            elif item.kind == "preset":
                preset = source.presets[item.resource_id]
                file_payloads.append(
                    (
                        target.root_dir / "presets" / f"{item.resource_id}.yaml",
                        preset.model_dump(exclude_none=True),
                    )
                )
            else:
                cue = source.cues[item.resource_id]
                file_payloads.append(
                    (
                        target.root_dir / "cues" / f"{item.resource_id}.yaml",
                        cue.model_dump(exclude_none=True),
                    )
                )

        created_files: list[Path] = []
        wrote_manifest = False
        try:
            db.flush()
            for output_path, document in file_payloads:
                # The preview is re-built under the import lock, but the
                # ordinary per-resource Authoring endpoints do not share this
                # lock. Refuse a late filesystem collision instead of ever
                # turning the create-only import into an overwrite.
                if output_path.exists():
                    raise HTTPException(
                        status_code=status.HTTP_409_CONFLICT,
                        detail=f"target resource appeared during import: {output_path.name}",
                    )
                _write_yaml(output_path, document)
                created_files.append(output_path)
            if manifest_changed:
                _write_yaml(manifest_path, manifest)
                wrote_manifest = True

            modes_loader.reload_mode(target.id)
            db.commit()
        except Exception as exc:
            db.rollback()
            for created_path in reversed(created_files):
                created_path.unlink(missing_ok=True)
            if wrote_manifest:
                _write_bytes(manifest_path, original_manifest)
            try:
                modes_loader.reload_mode(target.id)
            except Exception:
                logger.exception("failed to reload target mode after import rollback")
            if isinstance(exc, HTTPException):
                raise
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"authoring import failed and was rolled back: {type(exc).__name__}: {exc}",
            ) from exc

        return AuthoringImportResult(
            imported=imported,
            skipped=skipped,
            missing_track_paths=missing_track_paths,
        )
