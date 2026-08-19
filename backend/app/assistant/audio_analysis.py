from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, cast

from sqlalchemy import delete, select
from sqlalchemy.orm import Session

from app.assistant.audio_signal import AudioSignalError, analyze_audio_file
from app.core.db import SessionLocal
from app.jobs.registry import JobExecutionContext, register_job_handler
from app.library import index as library_index
from app.models.base import utcnow
from app.models.track import Track
from app.models.track_analysis import TrackAnalysis
from app.models.track_analysis_failure import TrackAnalysisFailure

AUDIO_ANALYSIS_JOB_KIND = "assistant.library-audio-analysis"
LOCAL_AUDIO_ANALYZER_ID = "local-audio/v1"
_FAILURE_SAMPLE_LIMIT = 20


@dataclass(frozen=True)
class CurrentAudioProfile:
    analyzer_id: str
    confidence: Literal["high", "medium", "low"]
    evidence: tuple[str, ...]
    metrics: dict[str, str | int | float | None]


def audio_source_signature(track: Track) -> str:
    """Fingerprint the indexed file identity consumed by the signal analyzer."""

    payload = [track.path, track.size_bytes, track.mtime]
    encoded = json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode(
        "utf-8"
    )
    return hashlib.sha256(encoded).hexdigest()


def load_current_audio_profiles(
    db: Session,
    tracks: list[Track],
) -> dict[int, CurrentAudioProfile]:
    track_by_id = {track.id: track for track in tracks}
    if not track_by_id:
        return {}
    rows = db.scalars(
        select(TrackAnalysis).where(
            TrackAnalysis.analyzer_id == LOCAL_AUDIO_ANALYZER_ID,
            TrackAnalysis.track_id.in_(track_by_id),
        )
    ).all()
    profiles: dict[int, CurrentAudioProfile] = {}
    for row in rows:
        track = track_by_id.get(row.track_id)
        if track is None or row.source_signature != audio_source_signature(track):
            continue
        try:
            evidence = json.loads(row.evidence_json)
            metrics = json.loads(row.metrics_json)
        except json.JSONDecodeError:
            continue
        if not isinstance(evidence, list) or not all(
            isinstance(item, str) for item in evidence
        ):
            continue
        if not isinstance(metrics, dict) or metrics.get("schema") != LOCAL_AUDIO_ANALYZER_ID:
            continue
        if not all(
            value is None
            or isinstance(value, str)
            or (isinstance(value, (int, float)) and not isinstance(value, bool))
            for value in metrics.values()
        ):
            continue
        if row.confidence not in {"high", "medium", "low"}:
            continue
        profiles[row.track_id] = CurrentAudioProfile(
            analyzer_id=LOCAL_AUDIO_ANALYZER_ID,
            confidence=cast(
                "Literal['high', 'medium', 'low']",
                row.confidence,
            ),
            evidence=tuple(evidence),
            metrics=cast("dict[str, str | int | float | None]", metrics),
        )
    return profiles


def _load_tracks_and_state() -> tuple[
    list[Track],
    dict[int, TrackAnalysis],
    dict[int, TrackAnalysisFailure],
]:
    with SessionLocal() as db:
        tracks = list(db.scalars(select(Track).order_by(Track.id)).all())
        profiles = {
            row.track_id: row
            for row in db.scalars(
                select(TrackAnalysis).where(
                    TrackAnalysis.analyzer_id == LOCAL_AUDIO_ANALYZER_ID
                )
            ).all()
        }
        failures = {
            row.track_id: row
            for row in db.scalars(
                select(TrackAnalysisFailure).where(
                    TrackAnalysisFailure.analyzer_id == LOCAL_AUDIO_ANALYZER_ID
                )
            ).all()
        }
    return tracks, profiles, failures


def _store_profile(
    track: Track,
    signature: str,
    context: JobExecutionContext,
    absolute_path: Path,
) -> None:
    profile = analyze_audio_file(
        absolute_path,
        check_cancelled=context.check_cancelled,
    )
    with SessionLocal() as db:
        row = db.get(TrackAnalysis, (track.id, LOCAL_AUDIO_ANALYZER_ID))
        if row is None:
            row = TrackAnalysis(
                track_id=track.id,
                analyzer_id=LOCAL_AUDIO_ANALYZER_ID,
                source_signature=signature,
                job_id=context.job_id,
                energy=profile.energy,
                brightness=profile.brightness,
                tension=profile.tension,
                confidence=profile.confidence,
            )
            db.add(row)
        row.source_signature = signature
        row.job_id = context.job_id
        row.energy = profile.energy
        row.brightness = profile.brightness
        row.tension = profile.tension
        row.moods_json = "[]"
        row.evidence_json = json.dumps(list(profile.evidence), ensure_ascii=False)
        row.metrics_json = json.dumps(
            profile.metrics,
            ensure_ascii=False,
            separators=(",", ":"),
            sort_keys=True,
        )
        row.confidence = profile.confidence
        row.updated_at = utcnow()
        db.execute(
            delete(TrackAnalysisFailure).where(
                TrackAnalysisFailure.track_id == track.id,
                TrackAnalysisFailure.analyzer_id == LOCAL_AUDIO_ANALYZER_ID,
            )
        )
        db.commit()


def _store_failure(
    track: Track,
    signature: str,
    context: JobExecutionContext,
    error: str,
) -> None:
    with SessionLocal() as db:
        row = db.get(TrackAnalysisFailure, (track.id, LOCAL_AUDIO_ANALYZER_ID))
        if row is None:
            row = TrackAnalysisFailure(
                track_id=track.id,
                analyzer_id=LOCAL_AUDIO_ANALYZER_ID,
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


def run_library_audio_analysis(
    context: JobExecutionContext,
    parameters: dict[str, object],
) -> dict[str, object]:
    """Measure library audio one track at a time with durable checkpoints."""

    force = parameters.get("force") is True
    tracks, existing_profiles, existing_failures = _load_tracks_and_state()
    signatures = {track.id: audio_source_signature(track) for track in tracks}

    work: list[Track] = []
    checkpointed = 0
    for track in tracks:
        signature = signatures[track.id]
        profile = existing_profiles.get(track.id)
        failure = existing_failures.get(track.id)
        completed_by_this_job = (
            profile is not None
            and profile.job_id == context.job_id
            and profile.source_signature == signature
        ) or (
            failure is not None
            and failure.job_id == context.job_id
            and failure.source_signature == signature
        )
        if completed_by_this_job:
            checkpointed += 1
            continue
        current_failure = failure is not None and failure.source_signature == signature
        if (
            not force
            and profile is not None
            and profile.source_signature == signature
            and not current_failure
        ):
            continue
        work.append(track)

    starting_progress = max(context.progress_current, checkpointed)
    total = starting_progress + len(work)
    context.update_progress(
        starting_progress,
        total,
        phase="Measuring audio signals",
        message=f"{len(work)} tracks need signal analysis",
    )

    for processed, track in enumerate(work, start=1):
        context.check_cancelled()
        signature = signatures[track.id]
        try:
            absolute_path = library_index.to_absolute(track.path)
            _store_profile(track, signature, context, absolute_path)
        except (AudioSignalError, OSError) as exc:
            _store_failure(track, signature, context, f"{type(exc).__name__}: {exc}")
        current = starting_progress + processed
        context.update_progress(
            current,
            total,
            phase="Measuring audio signals",
            message=f"Processed {current} of {total} tracks",
        )

    tracks, profiles, failures = _load_tracks_and_state()
    current_profiles = [
        profile_row
        for track in tracks
        if (profile_row := profiles.get(track.id)) is not None
        and profile_row.source_signature == audio_source_signature(track)
    ]
    current_failures = [
        failure_row
        for track in tracks
        if (failure_row := failures.get(track.id)) is not None
        and failure_row.source_signature == audio_source_signature(track)
    ]
    updated = sum(row.job_id == context.job_id for row in current_profiles)
    failed = sum(row.job_id == context.job_id for row in current_failures)
    path_by_id = {track.id: track.path for track in tracks}
    failure_samples = [
        {
            "track_id": row.track_id,
            "path": path_by_id.get(row.track_id, ""),
            "error": row.error,
        }
        for row in current_failures
        if row.job_id == context.job_id
    ][:_FAILURE_SAMPLE_LIMIT]
    return {
        "tracks": len(tracks),
        "updated": updated,
        "failed": failed,
        "unchanged": max(0, len(tracks) - updated - failed),
        "current_profiles": len(current_profiles),
        "current_failures": len(current_failures),
        "failure_samples": failure_samples,
        "analyzer": LOCAL_AUDIO_ANALYZER_ID,
    }


def audio_analysis_summary(db: Session) -> dict[str, object]:
    tracks = list(db.scalars(select(Track).order_by(Track.id)).all())
    profiles = {
        row.track_id: row
        for row in db.scalars(
            select(TrackAnalysis).where(
                TrackAnalysis.analyzer_id == LOCAL_AUDIO_ANALYZER_ID
            )
        ).all()
    }
    failures = {
        row.track_id: row
        for row in db.scalars(
            select(TrackAnalysisFailure).where(
                TrackAnalysisFailure.analyzer_id == LOCAL_AUDIO_ANALYZER_ID
            )
        ).all()
    }
    current: list[TrackAnalysis] = []
    current_failures: list[TrackAnalysisFailure] = []
    stale_tracks = 0
    for track in tracks:
        signature = audio_source_signature(track)
        profile = profiles.get(track.id)
        failure = failures.get(track.id)
        if profile is not None and profile.source_signature == signature:
            current.append(profile)
        elif profile is not None:
            stale_tracks += 1
        if failure is not None and failure.source_signature == signature:
            current_failures.append(failure)

    confidence_counts = {"high": 0, "medium": 0, "low": 0}
    for row in current:
        if row.confidence in confidence_counts:
            confidence_counts[row.confidence] += 1
    update_times = [row.updated_at for row in current]
    update_times.extend(row.updated_at for row in current_failures)
    last_updated_at = max(update_times, default=None)
    return {
        "analyzer": LOCAL_AUDIO_ANALYZER_ID,
        "library_tracks": len(tracks),
        "analyzed_tracks": len(current),
        "failed_tracks": len(current_failures),
        "stale_tracks": stale_tracks,
        "high_confidence": confidence_counts["high"],
        "medium_confidence": confidence_counts["medium"],
        "low_confidence": confidence_counts["low"],
        "last_updated_at": last_updated_at,
    }


register_job_handler(
    AUDIO_ANALYSIS_JOB_KIND,
    run_library_audio_analysis,
    restartable=True,
)
