"""Durable, versioned factual context for indexed library recordings."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from typing import Literal, cast

from sqlalchemy import delete, select
from sqlalchemy.orm import Session

from app.assistant.audio_context import (
    CONTEXT_ANALYZER_ID,
    AudioContextDocument,
    analyze_audio_context,
)
from app.assistant.audio_signal import AudioSignalError
from app.assistant.context_workers import (
    ContextAnalysisResult,
    ContextAnalysisTask,
    analyze_tracks_in_processes,
)
from app.assistant.tag_schemas import ModelTaggingScope
from app.assistant.voice_analysis import voice_analyzer_signature
from app.core.config import get_settings
from app.core.db import SessionLocal
from app.jobs.registry import JobExecutionContext, register_job_handler
from app.library import index as library_index
from app.models.base import utcnow
from app.models.track import Track
from app.models.track_analysis_failure import TrackAnalysisFailure
from app.models.track_context import TrackContext

LIBRARY_CONTEXT_JOB_KIND = "assistant.library-context-analysis"
LOCAL_CONTEXT_ANALYZER_ID: Literal["local-context/v1"] = "local-context/v1"
_FAILURE_SAMPLE_LIMIT = 20


@dataclass(frozen=True)
class CurrentTrackContext:
    analyzer_id: Literal["local-context/v1"]
    source_signature: str
    completeness: Literal["full", "partial"]
    confidence: Literal["high", "medium", "low"]
    summary: dict[str, object]
    timeline: list[dict[str, float]]
    sections: list[dict[str, object]]
    technical: dict[str, object]
    stages: dict[str, object]


def context_source_signature(track: Track) -> str:
    analyzer_signature = voice_analyzer_signature()
    source_facts: list[object] = [track.path, track.size_bytes, track.mtime]
    if analyzer_signature is not None:
        source_facts.append(analyzer_signature)
    payload = json.dumps(
        source_facts,
        ensure_ascii=False,
        separators=(",", ":"),
    ).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def resolve_context_scope(db: Session, scope: ModelTaggingScope) -> list[Track]:
    if scope.type == "tracks":
        rows = {
            track.id: track
            for track in db.scalars(select(Track).where(Track.id.in_(scope.track_ids))).all()
        }
        return [rows[track_id] for track_id in scope.track_ids if track_id in rows]
    if scope.type == "folder":
        if not scope.path:
            stmt = select(Track)
            if not scope.recursive:
                stmt = stmt.where(~Track.path.like("%/%"))
        else:
            prefix = library_index.like_escape(f"{scope.path}/")
            escape = library_index.LIKE_ESCAPE_CHAR
            stmt = select(Track).where(Track.path.like(f"{prefix}%", escape=escape))
            if not scope.recursive:
                stmt = stmt.where(~Track.path.like(f"{prefix}%/%", escape=escape))
        return list(db.scalars(stmt.order_by(Track.id)).all())
    return list(db.scalars(select(Track).order_by(Track.id)).all())


def _parse_context(row: TrackContext) -> CurrentTrackContext | None:
    if row.analyzer_id != LOCAL_CONTEXT_ANALYZER_ID:
        return None
    if row.completeness not in {"full", "partial"}:
        return None
    if row.confidence not in {"high", "medium", "low"}:
        return None
    try:
        summary = json.loads(row.summary_json)
        timeline = json.loads(row.timeline_json)
        sections = json.loads(row.sections_json)
        technical = json.loads(row.technical_json)
        stages = json.loads(row.stages_json)
    except json.JSONDecodeError:
        return None
    if not isinstance(summary, dict) or summary.get("schema_version") != CONTEXT_ANALYZER_ID:
        return None
    if not isinstance(timeline, list) or not all(isinstance(item, dict) for item in timeline):
        return None
    if not isinstance(sections, list) or not all(isinstance(item, dict) for item in sections):
        return None
    if not isinstance(technical, dict) or not isinstance(stages, dict):
        return None
    return CurrentTrackContext(
        analyzer_id=LOCAL_CONTEXT_ANALYZER_ID,
        source_signature=row.source_signature,
        completeness=cast("Literal['full', 'partial']", row.completeness),
        confidence=cast("Literal['high', 'medium', 'low']", row.confidence),
        summary=cast("dict[str, object]", summary),
        timeline=cast("list[dict[str, float]]", timeline),
        sections=cast("list[dict[str, object]]", sections),
        technical=cast("dict[str, object]", technical),
        stages=cast("dict[str, object]", stages),
    )


def load_current_contexts(
    db: Session,
    tracks: list[Track],
) -> dict[int, CurrentTrackContext]:
    track_by_id = {track.id: track for track in tracks}
    if not track_by_id:
        return {}
    rows = db.scalars(
        select(TrackContext).where(
            TrackContext.analyzer_id == LOCAL_CONTEXT_ANALYZER_ID,
            TrackContext.track_id.in_(track_by_id),
        )
    ).all()
    contexts: dict[int, CurrentTrackContext] = {}
    for row in rows:
        track = track_by_id.get(row.track_id)
        if track is None or row.source_signature != context_source_signature(track):
            continue
        parsed = _parse_context(row)
        if parsed is not None:
            contexts[row.track_id] = parsed
    return contexts


def compact_context_projection(context: CurrentTrackContext) -> dict[str, object]:
    """Return the bounded, auditable subset allowed to leave the server."""

    trajectories = context.summary.get("trajectories")
    tempo = context.summary.get("tempo")
    structure = context.summary.get("structure")
    voice = context.summary.get("voice")
    evidence = context.summary.get("evidence")
    bounded_sections: list[dict[str, object]] = []
    for section in context.sections[:8]:
        bounded_sections.append(
            {
                key: section.get(key)
                for key in (
                    "id",
                    "start_fraction",
                    "end_fraction",
                    "intensity",
                    "rhythmic_drive",
                    "brightness",
                    "density",
                    "tempo_bpm",
                    "tempo_confidence",
                    "changes_from_previous",
                    "repeats_section_ids",
                )
            }
        )
    return {
        "analyzer_id": context.analyzer_id,
        "completeness": context.completeness,
        "confidence": context.confidence,
        "trajectories": trajectories if isinstance(trajectories, dict) else {},
        "tempo": tempo if isinstance(tempo, dict) else {},
        "structure": structure if isinstance(structure, dict) else {},
        "voice": voice if isinstance(voice, dict) else {},
        "sections": bounded_sections,
        "evidence": (
            [item for item in evidence[:4] if isinstance(item, str)]
            if isinstance(evidence, list)
            else []
        ),
    }


def _store_document(
    track: Track,
    signature: str,
    context: JobExecutionContext,
    document: AudioContextDocument,
) -> None:
    with SessionLocal() as db:
        row = db.get(TrackContext, (track.id, LOCAL_CONTEXT_ANALYZER_ID))
        if row is None:
            row = TrackContext(
                track_id=track.id,
                analyzer_id=LOCAL_CONTEXT_ANALYZER_ID,
                source_signature=signature,
                job_id=context.job_id,
                completeness=document.completeness,
                confidence=document.confidence,
                summary_json="{}",
                timeline_json="[]",
                sections_json="[]",
                technical_json="{}",
                stages_json="{}",
            )
            db.add(row)
        row.source_signature = signature
        row.job_id = context.job_id
        row.completeness = document.completeness
        row.confidence = document.confidence
        row.summary_json = _json(document.summary)
        row.timeline_json = _json(document.timeline)
        row.sections_json = _json(document.sections)
        row.technical_json = _json(document.technical)
        row.stages_json = _json(document.stages)
        row.updated_at = utcnow()
        db.execute(
            delete(TrackAnalysisFailure).where(
                TrackAnalysisFailure.track_id == track.id,
                TrackAnalysisFailure.analyzer_id == LOCAL_CONTEXT_ANALYZER_ID,
            )
        )
        db.commit()


def _json(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True)


def _store_failure(
    track: Track,
    signature: str,
    context: JobExecutionContext,
    error: str,
) -> None:
    with SessionLocal() as db:
        row = db.get(TrackAnalysisFailure, (track.id, LOCAL_CONTEXT_ANALYZER_ID))
        if row is None:
            row = TrackAnalysisFailure(
                track_id=track.id,
                analyzer_id=LOCAL_CONTEXT_ANALYZER_ID,
                source_signature=signature,
                job_id=context.job_id,
                error=error,
            )
            db.add(row)
        row.source_signature = signature
        row.job_id = context.job_id
        row.error = error[:2_000]
        row.updated_at = utcnow()
        db.commit()


def _store_analysis_result(
    track: Track,
    signature: str,
    context: JobExecutionContext,
    result: ContextAnalysisResult,
) -> bool:
    if result.fatal:
        raise RuntimeError(
            f"context analysis worker failed for track {track.id}: "
            f"{result.error or 'unknown error'}"
        )
    if result.document is not None:
        _store_document(track, signature, context, result.document)
        return True
    _store_failure(
        track,
        signature,
        context,
        result.error or "AudioSignalError: context analysis returned no result",
    )
    return False


def run_library_context_analysis(
    context: JobExecutionContext,
    parameters: dict[str, object],
) -> dict[str, object]:
    force = parameters.get("force") is True
    scope = ModelTaggingScope.model_validate(parameters.get("scope", {"type": "all"}))
    with SessionLocal() as db:
        tracks = resolve_context_scope(db, scope)
        existing = {
            row.track_id: row
            for row in db.scalars(
                select(TrackContext).where(TrackContext.analyzer_id == LOCAL_CONTEXT_ANALYZER_ID)
            ).all()
        }
        failures = {
            row.track_id: row
            for row in db.scalars(
                select(TrackAnalysisFailure).where(
                    TrackAnalysisFailure.analyzer_id == LOCAL_CONTEXT_ANALYZER_ID
                )
            ).all()
        }
    signatures = {track.id: context_source_signature(track) for track in tracks}
    work: list[Track] = []
    checkpointed = 0
    updated_track_ids: set[int] = set()
    failed_track_ids: set[int] = set()
    for track in tracks:
        signature = signatures[track.id]
        profile = existing.get(track.id)
        failure = failures.get(track.id)
        completed_by_job = (
            profile is not None
            and profile.job_id == context.job_id
            and profile.source_signature == signature
        ) or (
            failure is not None
            and failure.job_id == context.job_id
            and failure.source_signature == signature
        )
        if completed_by_job:
            checkpointed += 1
            if (
                profile is not None
                and profile.job_id == context.job_id
                and profile.source_signature == signature
            ):
                updated_track_ids.add(track.id)
            else:
                failed_track_ids.add(track.id)
            continue
        parsed = _parse_context(profile) if profile is not None else None
        current_failure = failure is not None and failure.source_signature == signature
        if (
            not force
            and profile is not None
            and profile.source_signature == signature
            and parsed is not None
            and parsed.completeness == "full"
            and not current_failure
        ):
            continue
        work.append(track)
    starting = max(context.progress_current, checkpointed)
    total = starting + len(work)
    configured_workers = get_settings().assistant_library_context_workers
    active_workers = min(configured_workers, len(work)) if work else 0
    context.update_progress(
        starting,
        total,
        phase="Building track context",
        message=(
            f"{len(work)} tracks need comprehensive analysis"
            + (f" with {active_workers} workers" if active_workers else "")
        ),
    )
    if active_workers == 1:
        for processed, track in enumerate(work, start=1):
            context.check_cancelled()
            signature = signatures[track.id]
            try:
                document = analyze_audio_context(
                    library_index.to_absolute(track.path),
                    check_cancelled=context.check_cancelled,
                )
                _store_document(track, signature, context, document)
                updated_track_ids.add(track.id)
            except (AudioSignalError, OSError) as exc:
                _store_failure(track, signature, context, f"{type(exc).__name__}: {exc}")
                failed_track_ids.add(track.id)
            current = starting + processed
            context.update_progress(
                current,
                total,
                phase="Building track context",
                message=f"Processed {current} of {total} tracks",
            )
    elif active_workers > 1:
        tracks_by_id = {track.id: track for track in work}
        tasks = [
            ContextAnalysisTask(
                track_id=track.id,
                path=str(library_index.to_absolute(track.path)),
            )
            for track in work
        ]
        results = analyze_tracks_in_processes(
            tasks,
            max_workers=active_workers,
            check_cancelled=context.check_cancelled,
        )
        for processed, result in enumerate(results, start=1):
            track = tracks_by_id[result.track_id]
            if _store_analysis_result(track, signatures[track.id], context, result):
                updated_track_ids.add(track.id)
            else:
                failed_track_ids.add(track.id)
            current = starting + processed
            context.update_progress(
                current,
                total,
                phase="Building track context",
                message=f"Processed {current} of {total} tracks with {active_workers} workers",
            )
    with SessionLocal() as db:
        current_contexts = load_current_contexts(db, tracks)
        current_failures = {
            row.track_id: row
            for row in db.scalars(
                select(TrackAnalysisFailure).where(
                    TrackAnalysisFailure.analyzer_id == LOCAL_CONTEXT_ANALYZER_ID,
                    TrackAnalysisFailure.track_id.in_([track.id for track in tracks]),
                )
            ).all()
            if row.source_signature == signatures.get(row.track_id)
        }
    updated = len(updated_track_ids)
    failed = len(failed_track_ids)
    path_by_id = {track.id: track.path for track in tracks}
    failure_samples = [
        {"track_id": row.track_id, "path": path_by_id.get(row.track_id, ""), "error": row.error}
        for row in current_failures.values()
        if row.track_id in failed_track_ids
    ][:_FAILURE_SAMPLE_LIMIT]
    return {
        "schema_version": "assistant-library-context-job-result/v1",
        "analyzer": LOCAL_CONTEXT_ANALYZER_ID,
        "scope": scope.model_dump(mode="json"),
        "tracks": len(tracks),
        "analysis_workers": active_workers,
        "updated": updated,
        "failed": failed,
        "unchanged": max(0, len(tracks) - updated - failed),
        "current_contexts": len(current_contexts),
        "current_failures": len(current_failures),
        "failure_samples": failure_samples,
    }


def context_summary(db: Session) -> dict[str, object]:
    tracks = list(db.scalars(select(Track).order_by(Track.id)).all())
    rows = {
        row.track_id: row
        for row in db.scalars(
            select(TrackContext).where(TrackContext.analyzer_id == LOCAL_CONTEXT_ANALYZER_ID)
        ).all()
    }
    failures = {
        row.track_id: row
        for row in db.scalars(
            select(TrackAnalysisFailure).where(
                TrackAnalysisFailure.analyzer_id == LOCAL_CONTEXT_ANALYZER_ID
            )
        ).all()
    }
    current: list[TrackContext] = []
    current_failures: list[TrackAnalysisFailure] = []
    stale = 0
    for track in tracks:
        signature = context_source_signature(track)
        row = rows.get(track.id)
        if row is not None and row.source_signature == signature and _parse_context(row) is not None:
            current.append(row)
        elif row is not None:
            stale += 1
        failure = failures.get(track.id)
        if failure is not None and failure.source_signature == signature:
            current_failures.append(failure)
    confidence = {"high": 0, "medium": 0, "low": 0}
    completeness = {"full": 0, "partial": 0}
    for row in current:
        if row.confidence in confidence:
            confidence[row.confidence] += 1
        if row.completeness in completeness:
            completeness[row.completeness] += 1
    update_times = [row.updated_at for row in current]
    update_times.extend(row.updated_at for row in current_failures)
    return {
        "analyzer": LOCAL_CONTEXT_ANALYZER_ID,
        "library_tracks": len(tracks),
        "analyzed_tracks": len(current),
        "full_tracks": completeness["full"],
        "partial_tracks": completeness["partial"],
        "missing_tracks": max(0, len(tracks) - len(current) - len(current_failures)),
        "failed_tracks": len(current_failures),
        "stale_tracks": stale,
        "high_confidence": confidence["high"],
        "medium_confidence": confidence["medium"],
        "low_confidence": confidence["low"],
        "last_updated_at": max(update_times, default=None),
    }


def context_detail(db: Session, track: Track) -> dict[str, object]:
    signature = context_source_signature(track)
    row = db.get(TrackContext, (track.id, LOCAL_CONTEXT_ANALYZER_ID))
    failure = db.get(TrackAnalysisFailure, (track.id, LOCAL_CONTEXT_ANALYZER_ID))
    parsed = _parse_context(row) if row is not None and row.source_signature == signature else None
    if parsed is not None and row is not None:
        return {
            "track_id": track.id,
            "title": track.display_title or track.title,
            "artist": track.artist,
            "status": parsed.completeness,
            "analyzer_id": parsed.analyzer_id,
            "confidence": parsed.confidence,
            "updated_at": row.updated_at,
            "summary": parsed.summary,
            "timeline": parsed.timeline,
            "sections": parsed.sections,
            "technical": parsed.technical,
            "stages": parsed.stages,
            "error": None,
        }
    if failure is not None and failure.source_signature == signature:
        status = "failed"
        error = failure.error
        updated_at = failure.updated_at
    elif row is not None:
        status = "stale"
        error = None
        updated_at = row.updated_at
    else:
        status = "missing"
        error = None
        updated_at = None
    return {
        "track_id": track.id,
        "title": track.display_title or track.title,
        "artist": track.artist,
        "status": status,
        "analyzer_id": LOCAL_CONTEXT_ANALYZER_ID,
        "confidence": None,
        "updated_at": updated_at,
        "summary": None,
        "timeline": [],
        "sections": [],
        "technical": None,
        "stages": None,
        "error": error,
    }


register_job_handler(
    LIBRARY_CONTEXT_JOB_KIND,
    run_library_context_analysis,
    restartable=True,
)
