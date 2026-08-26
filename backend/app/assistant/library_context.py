"""Durable, versioned factual context for indexed library recordings."""

import hashlib
import json
import time
from dataclasses import dataclass
from typing import Literal, cast

from sqlalchemy import delete, select
from sqlalchemy.orm import Session

from app.assistant.audio_context import (
    CONTEXT_ANALYZER_ID,
    CONTEXT_IMPLEMENTATION_ID,
    AudioContextDocument,
    AudioContextPerformance,
    analyze_audio_context,
)
from app.assistant.audio_signal import AudioSignalError
from app.assistant.context_workers import (
    ContextAnalysisResult,
    ContextAnalysisTask,
    VoiceAnalysisResult,
    VoiceAnalysisTask,
    analyze_tracks_in_processes,
    analyze_voice_tracks_in_processes,
)
from app.assistant.tag_schemas import ModelTaggingScope
from app.assistant.voice_analysis import (
    VoiceAnalysis,
    voice_analyzer_signature,
    voice_analyzer_status,
)
from app.core.config import get_settings
from app.core.db import SessionLocal
from app.jobs.registry import JobExecutionContext, register_job_handler
from app.library import index as library_index
from app.models.background_job import BackgroundJob
from app.models.base import utcnow
from app.models.track import Track
from app.models.track_analysis_failure import TrackAnalysisFailure
from app.models.track_context import TrackContext

LIBRARY_CONTEXT_JOB_KIND = "assistant.library-context-analysis"
LOCAL_CONTEXT_ANALYZER_ID: Literal["local-context/v2"] = "local-context/v2"
_FAILURE_SAMPLE_LIMIT = 20
_VOICE_TRACKS_PER_WORKER = 4


@dataclass(frozen=True)
class CurrentTrackContext:
    analyzer_id: Literal["local-context/v2"]
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
    source_facts: list[object] = [
        CONTEXT_IMPLEMENTATION_ID,
        track.path,
        track.size_bytes,
        track.mtime,
    ]
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


def _voice_stage_status(track_context: CurrentTrackContext) -> str | None:
    voice_stage = track_context.stages.get("voice")
    if not isinstance(voice_stage, dict):
        return None
    status = voice_stage.get("status")
    return status if isinstance(status, str) else None


def _store_voice_result(
    track: Track,
    signature: str,
    context: JobExecutionContext,
    result: VoiceAnalysisResult,
) -> bool:
    if result.fatal or result.analysis is None:
        raise RuntimeError(
            f"voice analysis worker failed for track {track.id}: "
            f"{result.error or 'unknown error'}"
        )
    analysis: VoiceAnalysis = result.analysis
    with SessionLocal() as db:
        row = db.get(TrackContext, (track.id, LOCAL_CONTEXT_ANALYZER_ID))
        parsed = (
            _parse_context(row)
            if row is not None and row.source_signature == signature
            else None
        )
        if row is None or parsed is None:
            raise RuntimeError(
                f"voice analysis has no current audio-context checkpoint for track {track.id}"
            )
        summary = dict(parsed.summary)
        summary["voice"] = analysis.summary
        reliability_value = summary.get("measurement_reliability")
        reliability = dict(reliability_value) if isinstance(reliability_value, dict) else {}
        reliability["voice"] = (
            "high" if analysis.summary.get("status") == "classified" else "unavailable"
        )
        summary["measurement_reliability"] = reliability
        stages = dict(parsed.stages)
        stages["voice"] = analysis.stage

        row.job_id = context.job_id
        row.completeness = "full"
        row.summary_json = _json(summary)
        row.stages_json = _json(stages)
        row.updated_at = utcnow()
        db.execute(
            delete(TrackAnalysisFailure).where(
                TrackAnalysisFailure.track_id == track.id,
                TrackAnalysisFailure.analyzer_id == LOCAL_CONTEXT_ANALYZER_ID,
            )
        )
        db.commit()
    return analysis.summary.get("status") != "unavailable"


def _performance_summary(
    samples: list[AudioContextPerformance],
    *,
    wall_seconds: float,
) -> dict[str, object]:
    stage_seconds: dict[str, float] = {}
    for sample in samples:
        for stage, seconds in sample.stage_seconds.items():
            stage_seconds[stage] = stage_seconds.get(stage, 0.0) + seconds
    measured_stage_seconds = sum(stage_seconds.values())
    audio_seconds = sum(sample.audio_seconds for sample in samples)
    worker_seconds = sum(sample.elapsed_seconds for sample in samples)
    dominant_stage = (
        max(stage_seconds, key=lambda stage: stage_seconds[stage]) if stage_seconds else None
    )

    return {
        "schema_version": "library-context-performance/v1",
        "tracks_profiled": len(samples),
        "wall_seconds": round(wall_seconds, 3),
        "worker_seconds": round(worker_seconds, 3),
        "audio_seconds": round(audio_seconds, 3),
        "audio_realtime_factor": (
            round(audio_seconds / wall_seconds, 3) if wall_seconds > 0.0 else None
        ),
        "dominant_stage": dominant_stage,
        "stage_seconds": {
            stage: round(seconds, 3) for stage, seconds in sorted(stage_seconds.items())
        },
        "stage_share_percent": {
            stage: round(seconds * 100.0 / measured_stage_seconds, 1)
            for stage, seconds in sorted(stage_seconds.items())
        }
        if measured_stage_seconds > 0.0
        else {},
    }


def _voice_performance_summary(
    samples: list[float],
    *,
    wall_seconds: float,
) -> dict[str, object]:
    return {
        "schema_version": "library-context-voice-performance/v1",
        "tracks_profiled": len(samples),
        "wall_seconds": round(wall_seconds, 3),
        "worker_seconds": round(sum(samples), 3),
    }


def _pass_progress(
    *,
    status: str,
    completed: int,
    failed: int,
    skipped: int,
    total: int,
) -> dict[str, object]:
    return {
        "status": status,
        "completed_tracks": completed,
        "failed_tracks": failed,
        "skipped_tracks": skipped,
        "total_tracks": total,
    }


def _checkpoint_passes(
    context: JobExecutionContext,
    *,
    audio_context: dict[str, object],
    voice_detection: dict[str, object],
) -> None:
    context.checkpoint_result(
        {
            "schema_version": "assistant-library-context-job-progress/v1",
            "passes": {
                "audio_context": audio_context,
                "voice_detection": voice_detection,
            },
        }
    )


def run_library_context_analysis(
    context: JobExecutionContext,
    parameters: dict[str, object],
) -> dict[str, object]:
    job_started = time.perf_counter()
    force = parameters.get("force") is True
    scope = ModelTaggingScope.model_validate(parameters.get("scope", {"type": "all"}))
    analyzer_status = voice_analyzer_status()
    voice_enabled = analyzer_status.get("status") == "ready"
    with SessionLocal() as db:
        job_row = db.get(BackgroundJob, context.job_id)
        checkpoint_job_ids = {context.job_id}
        if job_row is not None and job_row.retry_of_id is not None:
            checkpoint_job_ids.add(job_row.retry_of_id)
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
    current_contexts: dict[int, CurrentTrackContext] = {}
    signal_work: list[Track] = []
    audio_completed_ids: set[int] = set()
    audio_failed_ids: set[int] = set()
    updated_track_ids: set[int] = set()
    failed_track_ids: set[int] = set()
    performance_samples: list[AudioContextPerformance] = []

    for track in tracks:
        signature = signatures[track.id]
        profile = existing.get(track.id)
        parsed = (
            _parse_context(profile)
            if profile is not None and profile.source_signature == signature
            else None
        )
        failure = failures.get(track.id)
        current_failure = failure is not None and failure.source_signature == signature
        if parsed is not None:
            current_contexts[track.id] = parsed
        completed_by_this_attempt = (
            profile is not None
            and parsed is not None
            and profile.job_id in checkpoint_job_ids
        )
        failed_by_this_job = (
            current_failure and failure is not None and failure.job_id == context.job_id
        )

        # A partial row is the durable hand-off between passes. Preserve it even
        # for a retried forced rebuild so voice work can resume without decoding
        # the track again.
        signal_is_current = parsed is not None and (
            parsed.completeness == "partial" or not force or completed_by_this_attempt
        )
        if signal_is_current:
            audio_completed_ids.add(track.id)
            if completed_by_this_attempt:
                updated_track_ids.add(track.id)
            continue
        if failed_by_this_job:
            audio_failed_ids.add(track.id)
            failed_track_ids.add(track.id)
            continue
        signal_work.append(track)

    voice_completed_ids = {
        track_id
        for track_id in audio_completed_ids
        if (parsed := current_contexts.get(track_id)) is not None
        and _voice_stage_status(parsed) not in {None, "pending", "unavailable"}
    }
    voice_failed_ids = {
        track_id
        for track_id in audio_completed_ids
        if (parsed := current_contexts.get(track_id)) is not None
        and _voice_stage_status(parsed) == "unavailable"
    }
    audio_progress = _pass_progress(
        status="running" if signal_work else "complete",
        completed=len(audio_completed_ids),
        failed=len(audio_failed_ids),
        skipped=0,
        total=len(tracks),
    )
    voice_progress = _pass_progress(
        status="waiting" if voice_enabled else "not_available",
        completed=len(voice_completed_ids) if voice_enabled else 0,
        failed=len(voice_failed_ids) if voice_enabled else 0,
        skipped=0 if voice_enabled else len(tracks),
        total=len(tracks),
    )
    _checkpoint_passes(
        context,
        audio_context=audio_progress,
        voice_detection=voice_progress,
    )

    settings = get_settings()
    configured_workers = settings.assistant_library_context_workers
    active_workers = min(configured_workers, len(signal_work)) if signal_work else 0
    progress_current = context.progress_current
    progress_total = progress_current + len(signal_work) * (2 if voice_enabled else 1)
    context.update_progress(
        progress_current,
        progress_total,
        phase="Analyzing audio context",
        message=(
            f"Audio context: {len(audio_completed_ids) + len(audio_failed_ids)} of "
            f"{len(tracks)} tracks processed"
        ),
    )

    signal_pass_started = time.perf_counter()
    if len(signal_work) == 1:
        for track in signal_work:
            context.check_cancelled()
            signature = signatures[track.id]
            try:
                document = analyze_audio_context(
                    library_index.to_absolute(track.path),
                    check_cancelled=context.check_cancelled,
                    include_voice=not voice_enabled,
                )
                if document.performance is not None:
                    performance_samples.append(document.performance)
                _store_document(track, signature, context, document)
                audio_completed_ids.add(track.id)
                updated_track_ids.add(track.id)
            except (AudioSignalError, OSError) as exc:
                _store_failure(track, signature, context, f"{type(exc).__name__}: {exc}")
                audio_failed_ids.add(track.id)
                failed_track_ids.add(track.id)
            progress_current += 1
            audio_progress = _pass_progress(
                status="running",
                completed=len(audio_completed_ids),
                failed=len(audio_failed_ids),
                skipped=0,
                total=len(tracks),
            )
            _checkpoint_passes(
                context,
                audio_context=audio_progress,
                voice_detection=voice_progress,
            )
            context.update_progress(
                progress_current,
                progress_total,
                phase="Analyzing audio context",
                message=(
                    f"Audio context: {len(audio_completed_ids) + len(audio_failed_ids)} "
                    f"of {len(tracks)} tracks processed"
                ),
            )
    elif active_workers > 0:
        tracks_by_id = {track.id: track for track in signal_work}
        tasks = [
            ContextAnalysisTask(
                track_id=track.id,
                path=str(library_index.to_absolute(track.path)),
                include_voice=not voice_enabled,
            )
            for track in signal_work
        ]
        signal_results = analyze_tracks_in_processes(
            tasks,
            max_workers=active_workers,
            max_tasks_per_worker=None,
            check_cancelled=context.check_cancelled,
        )
        for signal_result in signal_results:
            track = tracks_by_id[signal_result.track_id]
            if (
                signal_result.document is not None
                and signal_result.document.performance is not None
            ):
                performance_samples.append(signal_result.document.performance)
            if _store_analysis_result(track, signatures[track.id], context, signal_result):
                audio_completed_ids.add(track.id)
                updated_track_ids.add(track.id)
            else:
                audio_failed_ids.add(track.id)
                failed_track_ids.add(track.id)
            progress_current += 1
            audio_progress = _pass_progress(
                status="running",
                completed=len(audio_completed_ids),
                failed=len(audio_failed_ids),
                skipped=0,
                total=len(tracks),
            )
            _checkpoint_passes(
                context,
                audio_context=audio_progress,
                voice_detection=voice_progress,
            )
            context.update_progress(
                progress_current,
                progress_total,
                phase="Analyzing audio context",
                message=(
                    f"Audio context: {len(audio_completed_ids) + len(audio_failed_ids)} "
                    f"of {len(tracks)} tracks processed with {active_workers} workers"
                ),
            )
    signal_wall_seconds = time.perf_counter() - signal_pass_started
    audio_progress = _pass_progress(
        status="complete_with_failures" if audio_failed_ids else "complete",
        completed=len(audio_completed_ids),
        failed=len(audio_failed_ids),
        skipped=0,
        total=len(tracks),
    )

    voice_performance_samples: list[float] = []
    voice_wall_seconds = 0.0
    active_voice_workers = 0
    if voice_enabled:
        with SessionLocal() as db:
            contexts_after_signal = load_current_contexts(db, tracks)
        voice_work = [
            track
            for track in tracks
            if (parsed := contexts_after_signal.get(track.id)) is not None
            and _voice_stage_status(parsed) == "pending"
        ]
        voice_completed_ids = {
            track.id
            for track in tracks
            if (parsed := contexts_after_signal.get(track.id)) is not None
            and _voice_stage_status(parsed) not in {None, "pending", "unavailable"}
        }
        voice_failed_ids = {
            track.id
            for track in tracks
            if (parsed := contexts_after_signal.get(track.id)) is not None
            and _voice_stage_status(parsed) == "unavailable"
        }
        voice_skipped_ids = {track.id for track in tracks} - set(contexts_after_signal)
        active_voice_workers = min(configured_workers, len(voice_work)) if voice_work else 0
        progress_total = progress_current + len(voice_work)
        voice_progress = _pass_progress(
            status="running" if voice_work else "complete",
            completed=len(voice_completed_ids),
            failed=len(voice_failed_ids),
            skipped=len(voice_skipped_ids),
            total=len(tracks),
        )
        _checkpoint_passes(
            context,
            audio_context=audio_progress,
            voice_detection=voice_progress,
        )
        context.update_progress(
            progress_current,
            progress_total,
            phase="Detecting voice",
            message=(
                f"Voice detection: {len(voice_completed_ids) + len(voice_failed_ids)} "
                f"of {len(tracks) - len(voice_skipped_ids)} eligible tracks processed"
            ),
        )
        voice_pass_started = time.perf_counter()
        if active_voice_workers > 0:
            tracks_by_id = {track.id: track for track in voice_work}
            voice_results = analyze_voice_tracks_in_processes(
                [
                    VoiceAnalysisTask(
                        track_id=track.id,
                        path=str(library_index.to_absolute(track.path)),
                    )
                    for track in voice_work
                ],
                max_workers=active_voice_workers,
                max_tasks_per_worker=_VOICE_TRACKS_PER_WORKER,
                check_cancelled=context.check_cancelled,
            )
            for voice_result in voice_results:
                track = tracks_by_id[voice_result.track_id]
                voice_performance_samples.append(voice_result.elapsed_seconds)
                if _store_voice_result(track, signatures[track.id], context, voice_result):
                    voice_completed_ids.add(track.id)
                else:
                    voice_failed_ids.add(track.id)
                updated_track_ids.add(track.id)
                progress_current += 1
                voice_progress = _pass_progress(
                    status="running",
                    completed=len(voice_completed_ids),
                    failed=len(voice_failed_ids),
                    skipped=len(voice_skipped_ids),
                    total=len(tracks),
                )
                _checkpoint_passes(
                    context,
                    audio_context=audio_progress,
                    voice_detection=voice_progress,
                )
                context.update_progress(
                    progress_current,
                    progress_total,
                    phase="Detecting voice",
                    message=(
                        f"Voice detection: {len(voice_completed_ids) + len(voice_failed_ids)} "
                        f"of {len(tracks) - len(voice_skipped_ids)} eligible tracks processed "
                        f"with {active_voice_workers} workers"
                    ),
                )
        voice_wall_seconds = time.perf_counter() - voice_pass_started
        voice_progress = _pass_progress(
            status=(
                "complete_with_failures"
                if voice_failed_ids or voice_skipped_ids
                else "complete"
            ),
            completed=len(voice_completed_ids),
            failed=len(voice_failed_ids),
            skipped=len(voice_skipped_ids),
            total=len(tracks),
        )
    else:
        voice_progress = _pass_progress(
            status="not_available",
            completed=0,
            failed=0,
            skipped=len(tracks),
            total=len(tracks),
        )

    _checkpoint_passes(
        context,
        audio_context=audio_progress,
        voice_detection=voice_progress,
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
        "schema_version": "assistant-library-context-job-result/v3",
        "analyzer": LOCAL_CONTEXT_ANALYZER_ID,
        "scope": scope.model_dump(mode="json"),
        "tracks": len(tracks),
        "analysis_workers": active_workers,
        "voice_workers": active_voice_workers,
        "passes": {
            "audio_context": audio_progress,
            "voice_detection": voice_progress,
        },
        "updated": updated,
        "failed": failed,
        "unchanged": max(0, len(tracks) - updated - failed),
        "current_contexts": len(current_contexts),
        "current_failures": len(current_failures),
        "failure_samples": failure_samples,
        "performance": _performance_summary(
            performance_samples,
            wall_seconds=signal_wall_seconds,
        ),
        "voice_performance": _voice_performance_summary(
            voice_performance_samples,
            wall_seconds=voice_wall_seconds,
        ),
        "wall_seconds": round(time.perf_counter() - job_started, 3),
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
    current: list[tuple[TrackContext, CurrentTrackContext]] = []
    current_failures: list[TrackAnalysisFailure] = []
    stale = 0
    for track in tracks:
        signature = context_source_signature(track)
        row = rows.get(track.id)
        parsed = _parse_context(row) if row is not None and row.source_signature == signature else None
        if row is not None and parsed is not None:
            current.append((row, parsed))
        elif row is not None:
            stale += 1
        failure = failures.get(track.id)
        if parsed is None and failure is not None and failure.source_signature == signature:
            current_failures.append(failure)
    confidence = {"high": 0, "medium": 0, "low": 0}
    completeness = {"full": 0, "partial": 0}
    voice_complete = 0
    voice_failed = 0
    for row, parsed in current:
        if row.confidence in confidence:
            confidence[row.confidence] += 1
        if row.completeness in completeness:
            completeness[row.completeness] += 1
        voice_status = _voice_stage_status(parsed)
        if voice_status == "unavailable":
            voice_failed += 1
        elif voice_status not in {None, "pending", "not_configured"}:
            voice_complete += 1
    update_times = [row.updated_at for row, _parsed in current]
    update_times.extend(row.updated_at for row in current_failures)
    analyzer_status = voice_analyzer_status()
    voice_enabled = analyzer_status.get("status") == "ready"
    return {
        "analyzer": LOCAL_CONTEXT_ANALYZER_ID,
        "voice_analyzer": analyzer_status,
        "passes": {
            "audio_context": {
                "completed_tracks": len(current),
                "failed_tracks": len(current_failures),
                "skipped_tracks": 0,
                "total_tracks": len(tracks),
                "enabled": True,
            },
            "voice_detection": {
                "completed_tracks": voice_complete if voice_enabled else 0,
                "failed_tracks": voice_failed if voice_enabled else 0,
                "skipped_tracks": 0 if voice_enabled else len(tracks),
                "total_tracks": len(tracks),
                "enabled": voice_enabled,
            },
        },
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
