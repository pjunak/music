"""Planning and atomic commit service shared by every Authoring import source."""
from __future__ import annotations

import logging
import os
import tempfile
from collections import Counter
from collections.abc import Callable
from pathlib import Path
from threading import Lock

import yaml
from fastapi import HTTPException, status
from sqlalchemy import select
from sqlalchemy.orm import Session

from app.authoring.schemas import (
    AuthoringImportIssue,
    AuthoringImportItem,
    AuthoringImportMode,
    AuthoringImportPreview,
    AuthoringImportResult,
    AuthoringImportSelection,
    ImportIssueSeverity,
    ImportItemStatus,
)
from app.authoring.sources import (
    ImportBundle,
    ImportResource,
    PlaylistPayload,
)
from app.models.playlist import Playlist, PlaylistItem
from app.models.track import Track
from app.modes import loader as modes_loader
from app.modes.loader import CueSpec, InterruptSpec, SoundboardManifest
from app.presets.loader import PresetManifest

logger = logging.getLogger(__name__)

# A commit spans SQLite and several mode files. Serializing this short critical
# section lets us re-plan immediately before writing and keeps imports
# create-only even when two requests race.
_import_lock = Lock()


def mode_or_404(mode_id: str) -> modes_loader.ModeManifest:
    mode = modes_loader.get_mode(mode_id)
    if mode is None:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail=f"mode '{mode_id}' not loaded",
        )
    return mode


def _selection_key(selection: AuthoringImportSelection) -> str:
    return f"{selection.kind}:{selection.resource_id}"


def _issue(
    code: str,
    severity: ImportIssueSeverity,
    message: str,
    *,
    related: ImportResource | None = None,
) -> AuthoringImportIssue:
    return AuthoringImportIssue(
        code=code,
        severity=severity,
        message=message,
        related_item=(
            AuthoringImportSelection(
                kind=related.kind,
                resource_id=related.resource_id,
            )
            if related
            else None
        ),
    )


def _soundboard_contains(soundboard: SoundboardManifest, item_path: str) -> bool:
    return any(
        item.file == item_path
        for category in soundboard.categories
        for item in category.items
    )


def _dependency_issues(
    resource: ImportResource,
    bundle: ImportBundle,
    target: modes_loader.ModeManifest,
    target_playlist_names: set[str],
) -> list[AuthoringImportIssue]:
    """Describe references that require another selection or cannot resolve."""

    source_playlists: dict[str, list[ImportResource]] = {}
    source_presets: dict[str, ImportResource] = {}
    source_soundboards: dict[str, ImportResource] = {}
    for candidate in bundle.resources:
        if candidate.kind == "playlist" and isinstance(
            candidate.payload, PlaylistPayload
        ):
            source_playlists.setdefault(candidate.payload.name, []).append(candidate)
        elif candidate.kind == "preset":
            source_presets[candidate.resource_id] = candidate
        elif candidate.kind == "soundboard":
            source_soundboards[candidate.resource_id] = candidate

    issues: list[AuthoringImportIssue] = []

    def require_playlist(name: str) -> None:
        if name in target_playlist_names:
            return
        candidates = source_playlists.get(name, [])
        if len(candidates) == 1:
            issues.append(
                _issue(
                    "dependency_selection_required",
                    "warning",
                    f"Also select playlist '{name}'.",
                    related=candidates[0],
                )
            )
        elif len(candidates) > 1:
            issues.append(
                _issue(
                    "ambiguous_dependency",
                    "error",
                    f"Playlist reference '{name}' matches multiple source playlists.",
                )
            )
        else:
            issues.append(
                _issue(
                    "missing_dependency",
                    "error",
                    f"Referenced playlist '{name}' is not in the target or import document.",
                )
            )

    def require_preset(preset_id: str) -> None:
        if preset_id in target.presets:
            return
        candidate = source_presets.get(preset_id)
        if candidate:
            issues.append(
                _issue(
                    "dependency_selection_required",
                    "warning",
                    f"Also select EQ preset '{preset_id}'.",
                    related=candidate,
                )
            )
        else:
            issues.append(
                _issue(
                    "missing_dependency",
                    "error",
                    f"Referenced EQ preset '{preset_id}' is not in the target or import document.",
                )
            )

    def require_soundboard(soundboard_id: str, item_path: str) -> None:
        target_board = target.soundboards.get(soundboard_id)
        if target_board:
            if _soundboard_contains(target_board, item_path):
                return
            issues.append(
                _issue(
                    "missing_dependency",
                    "error",
                    (
                        f"Target soundboard '{soundboard_id}' does not contain "
                        f"sound '{item_path}', and its ID is already occupied."
                    ),
                )
            )
            return
        candidate = source_soundboards.get(soundboard_id)
        if (
            candidate
            and isinstance(candidate.payload, SoundboardManifest)
            and _soundboard_contains(candidate.payload, item_path)
        ):
            issues.append(
                _issue(
                    "dependency_selection_required",
                    "warning",
                    f"Also select soundboard '{soundboard_id}'.",
                    related=candidate,
                )
            )
            return
        issues.append(
            _issue(
                "missing_dependency",
                "error",
                f"Sound '{item_path}' is not available in soundboard '{soundboard_id}'.",
            )
        )

    def require_sound_path(item_path: str) -> None:
        if any(
            _soundboard_contains(soundboard, item_path)
            for soundboard in target.soundboards.values()
        ):
            return
        matches = [
            candidate
            for candidate in source_soundboards.values()
            if isinstance(candidate.payload, SoundboardManifest)
            and _soundboard_contains(candidate.payload, item_path)
        ]
        if len(matches) == 1:
            if matches[0].resource_id in target.soundboards:
                issues.append(
                    _issue(
                        "missing_dependency",
                        "error",
                        (
                            f"Target soundboard '{matches[0].resource_id}' does not "
                            f"contain sound '{item_path}', and its ID is already occupied."
                        ),
                    )
                )
                return
            issues.append(
                _issue(
                    "dependency_selection_required",
                    "warning",
                    f"Also select soundboard '{matches[0].resource_id}'.",
                    related=matches[0],
                )
            )
        elif len(matches) > 1:
            issues.append(
                _issue(
                    "ambiguous_dependency",
                    "error",
                    f"Sound reference '{item_path}' matches multiple source soundboards.",
                )
            )
        else:
            issues.append(
                _issue(
                    "missing_dependency",
                    "error",
                    f"Referenced sound '{item_path}' is not in the target or import document.",
                )
            )

    if isinstance(resource.payload, InterruptSpec):
        if resource.payload.playlist:
            require_playlist(resource.payload.playlist)
        elif resource.payload.soundboard_item:
            require_sound_path(resource.payload.soundboard_item)
    elif isinstance(resource.payload, CueSpec):
        if resource.payload.preset:
            require_preset(resource.payload.preset)
        if resource.payload.playlist:
            require_playlist(resource.payload.playlist)
        for sfx in resource.payload.sfx:
            require_soundboard(sfx.soundboard, sfx.item)
        for loop in resource.payload.loops:
            require_soundboard(loop.soundboard, loop.item)

    return issues


def build_preview(
    db: Session,
    target_mode_id: str,
    bundle: ImportBundle,
) -> AuthoringImportPreview:
    target = mode_or_404(target_mode_id)
    if bundle.source.type == "mode" and bundle.source.id == target_mode_id:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="source and target modes must be different",
        )

    target_playlist_names = set(
        db.scalars(
            select(Playlist.name).where(Playlist.mode_id == target_mode_id)
        ).all()
    )
    playlist_name_counts = Counter(
        resource.payload.name
        for resource in bundle.resources
        if resource.kind == "playlist"
        and isinstance(resource.payload, PlaylistPayload)
    )
    interrupt_name_counts = Counter(
        resource.name
        for resource in bundle.resources
        if resource.kind == "interrupt"
    )
    target_interrupt_names = {interrupt.name for interrupt in target.interrupts}
    library_paths = set(db.scalars(select(Track.path)).all())

    items: list[AuthoringImportItem] = []
    for resource in bundle.resources:
        issues = list(resource.issues)
        conflict_reason: str | None = None

        if resource.kind == "playlist" and isinstance(
            resource.payload, PlaylistPayload
        ):
            if playlist_name_counts[resource.payload.name] > 1:
                issues.append(
                    _issue(
                        "duplicate_source_name",
                        "error",
                        "Another source playlist has the same name.",
                    )
                )
            elif resource.payload.name in target_playlist_names:
                conflict_reason = (
                    "A playlist with this name already exists in the target mode."
                )
            missing = [
                track.missing_label
                for track in resource.payload.tracks
                if track.path is None or track.path not in library_paths
            ]
            if missing:
                issues.append(
                    _issue(
                        "missing_tracks",
                        "warning",
                        f"{len(missing)} track reference(s) are unavailable and will be omitted.",
                    )
                )
        elif resource.kind == "soundboard":
            if resource.resource_id in target.soundboards:
                conflict_reason = (
                    "A soundboard with this ID already exists in the target mode."
                )
        elif resource.kind == "interrupt":
            if interrupt_name_counts[resource.name] > 1:
                issues.append(
                    _issue(
                        "duplicate_source_name",
                        "error",
                        "Another source interrupt has the same name.",
                    )
                )
            elif resource.name in target_interrupt_names:
                conflict_reason = (
                    "An interrupt with this name already exists in the target mode."
                )
        elif resource.kind == "preset":
            if resource.resource_id in target.presets:
                conflict_reason = (
                    "An EQ preset with this ID already exists in the target mode."
                )
        elif resource.resource_id in target.cues:
            conflict_reason = "A cue with this ID already exists in the target mode."

        issues.extend(
            _dependency_issues(
                resource,
                bundle,
                target,
                target_playlist_names,
            )
        )
        first_error = next(
            (issue.message for issue in issues if issue.severity == "error"),
            None,
        )
        if conflict_reason:
            item_status: ImportItemStatus = "conflict"
            reason = conflict_reason
            issues.append(_issue("target_conflict", "error", conflict_reason))
        elif first_error:
            item_status = "invalid"
            reason = first_error
        else:
            item_status = "ready"
            reason = None

        items.append(
            AuthoringImportItem(
                kind=resource.kind,
                resource_id=resource.resource_id,
                name=resource.name,
                summary=resource.summary,
                status=item_status,
                reason=reason,
                issues=issues,
            )
        )

    return AuthoringImportPreview(
        source=bundle.source,
        source_mode=(
            AuthoringImportMode(id=bundle.source.id, name=bundle.source.name)
            if bundle.source.type == "mode"
            else None
        ),
        target_mode=AuthoringImportMode(id=target.id, name=target.name),
        items=items,
    )


def _write_yaml(path: Path, payload: dict) -> None:
    """Atomically create or replace one validated Authoring YAML document."""

    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temp_name = tempfile.mkstemp(
        dir=path.parent,
        prefix=f".{path.name}.",
        suffix=".tmp",
        text=True,
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
    """Atomically restore exact pre-import bytes during rollback."""

    fd, temp_name = tempfile.mkstemp(
        dir=path.parent,
        prefix=f".{path.name}.",
        suffix=".rollback",
        text=False,
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


def _target_manifest(
    mode: modes_loader.ModeManifest,
) -> tuple[Path, dict, bytes]:
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
    for field_name in ("playlist_categories", "interrupts"):
        raw.setdefault(field_name, [])
        if not isinstance(raw[field_name], list):
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=(
                    f"manifest.yaml field '{field_name}' for mode "
                    f"'{mode.id}' is not a list"
                ),
            )
    return (path, raw, original)


def _selected_resources(
    preview: AuthoringImportPreview,
    bundle: ImportBundle,
    selections: list[AuthoringImportSelection],
) -> tuple[list[ImportResource], list[AuthoringImportItem]]:
    preview_by_key = {
        _selection_key(
            AuthoringImportSelection(kind=item.kind, resource_id=item.resource_id)
        ): item
        for item in preview.items
    }
    resources_by_key = bundle.by_key()
    requested_keys: list[str] = []
    seen: set[str] = set()
    for selection in selections:
        key = _selection_key(selection)
        if key in seen:
            continue
        if key not in preview_by_key or key not in resources_by_key:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail=f"source resource '{key}' is no longer available",
            )
        requested_keys.append(key)
        seen.add(key)

    selected_ready_keys = {
        key for key in requested_keys if preview_by_key[key].status == "ready"
    }
    for key in selected_ready_keys:
        item = preview_by_key[key]
        for issue in item.issues:
            if issue.code != "dependency_selection_required" or not issue.related_item:
                continue
            dependency_key = _selection_key(issue.related_item)
            if dependency_key not in selected_ready_keys:
                raise HTTPException(
                    status_code=status.HTTP_400_BAD_REQUEST,
                    detail=(
                        f"'{item.name}' requires {issue.related_item.kind} "
                        f"'{issue.related_item.resource_id}' to be selected and ready"
                    ),
                )

    imported = [resources_by_key[key] for key in requested_keys if key in selected_ready_keys]
    skipped = [
        preview_by_key[key] for key in requested_keys if key not in selected_ready_keys
    ]
    return imported, skipped


def commit_bundle(
    db: Session,
    target_mode_id: str,
    bundle_factory: Callable[[], ImportBundle],
    selections: list[AuthoringImportSelection],
) -> AuthoringImportResult:
    """Re-plan and atomically create a selection in the target mode."""

    with _import_lock:
        bundle = bundle_factory()
        preview = build_preview(db, target_mode_id, bundle)
        imported_resources, skipped = _selected_resources(
            preview,
            bundle,
            selections,
        )
        if not imported_resources:
            return AuthoringImportResult(
                imported=[],
                skipped=skipped,
                missing_track_paths=[],
            )

        target = mode_or_404(target_mode_id)
        if target.root_dir is None:
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"mode '{target.id}' has no resolved root directory",
            )

        manifest_path, manifest, original_manifest = _target_manifest(target)
        manifest_changed = False
        file_payloads: list[tuple[Path, dict]] = []
        missing_track_paths: list[str] = []

        library_tracks = {
            track.path: track
            for track in db.scalars(select(Track).order_by(Track.id)).all()
        }
        for resource in imported_resources:
            if isinstance(resource.payload, PlaylistPayload):
                cloned = Playlist(
                    name=resource.payload.name,
                    mode_id=target.id,
                    category=resource.payload.category,
                )
                db.add(cloned)
                db.flush()
                next_position = 0
                for track_ref in resource.payload.tracks:
                    track = (
                        library_tracks.get(track_ref.path)
                        if track_ref.path is not None
                        else None
                    )
                    if track is None:
                        missing_track_paths.append(track_ref.missing_label)
                        continue
                    db.add(
                        PlaylistItem(
                            playlist_id=cloned.id,
                            position=next_position,
                            track_id=track.id,
                        )
                    )
                    next_position += 1
                category = resource.payload.category
                if category and category not in manifest["playlist_categories"]:
                    manifest["playlist_categories"].append(category)
                    manifest_changed = True
            elif isinstance(resource.payload, SoundboardManifest):
                file_payloads.append(
                    (
                        target.root_dir
                        / "soundboards"
                        / f"{resource.resource_id}.yaml",
                        resource.payload.model_dump(exclude_none=True),
                    )
                )
            elif isinstance(resource.payload, InterruptSpec):
                manifest["interrupts"].append(
                    resource.payload.model_dump(exclude_none=True)
                )
                manifest_changed = True
            elif isinstance(resource.payload, PresetManifest):
                file_payloads.append(
                    (
                        target.root_dir / "presets" / f"{resource.resource_id}.yaml",
                        resource.payload.model_dump(exclude_none=True),
                    )
                )
            elif isinstance(resource.payload, CueSpec):
                file_payloads.append(
                    (
                        target.root_dir / "cues" / f"{resource.resource_id}.yaml",
                        resource.payload.model_dump(exclude_none=True),
                    )
                )

        created_files: list[Path] = []
        wrote_manifest = False
        try:
            db.flush()
            for output_path, document in file_payloads:
                # Other Authoring endpoints do not share this lock. A final
                # existence check preserves the create-only contract.
                if output_path.exists():
                    raise HTTPException(
                        status_code=status.HTTP_409_CONFLICT,
                        detail=(
                            "target resource appeared during import: "
                            f"{output_path.name}"
                        ),
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
            rollback_errors: list[str] = []
            for created_path in reversed(created_files):
                try:
                    created_path.unlink(missing_ok=True)
                except OSError as rollback_error:
                    rollback_errors.append(
                        f"could not remove {created_path.name}: {rollback_error}"
                    )
            if wrote_manifest:
                try:
                    _write_bytes(manifest_path, original_manifest)
                except OSError as rollback_error:
                    rollback_errors.append(
                        f"could not restore manifest.yaml: {rollback_error}"
                    )
            try:
                modes_loader.reload_mode(target.id)
            except Exception as rollback_error:
                logger.exception("failed to reload target mode after import rollback")
                rollback_errors.append(
                    f"could not reload target mode: {rollback_error}"
                )
            if rollback_errors:
                logger.error(
                    "authoring import rollback incomplete: %s",
                    "; ".join(rollback_errors),
                )
                raise HTTPException(
                    status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                    detail=(
                        "authoring import failed and rollback was incomplete; "
                        "check server logs before retrying"
                    ),
                ) from exc
            if isinstance(exc, HTTPException):
                raise
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=(
                    "authoring import failed and was rolled back: "
                    f"{type(exc).__name__}: {exc}"
                ),
            ) from exc

        imported_items_by_key = {
            f"{item.kind}:{item.resource_id}": item for item in preview.items
        }
        return AuthoringImportResult(
            imported=[
                imported_items_by_key[resource.key]
                for resource in imported_resources
            ],
            skipped=skipped,
            missing_track_paths=missing_track_paths,
        )
