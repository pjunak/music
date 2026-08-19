from __future__ import annotations

import hashlib
import json
from collections.abc import Sequence
from typing import Literal, cast

from sqlalchemy import select
from sqlalchemy.orm import Session

from app.assistant.engine import TrackAnalysisProfile
from app.assistant.local import analyze_track_metadata
from app.core.db import SessionLocal
from app.jobs.registry import JobExecutionContext, register_job_handler
from app.models.base import utcnow
from app.models.track import Track
from app.models.track_analysis import TrackAnalysis
from app.models.track_analysis_tag_review import TrackAnalysisTagReview

ANALYSIS_JOB_KIND = "assistant.library-analysis"
LOCAL_METADATA_ANALYZER_ID = "local-metadata/v1"
_CHUNK_SIZE = 50


def track_source_signature(track: Track) -> str:
    """Fingerprint only the fields consumed by the metadata analyzer."""

    payload = [
        track.path,
        track.title,
        track.display_title,
        track.artist,
        track.album,
        track.origin,
        track.genre,
        track.bpm,
    ]
    encoded = json.dumps(
        payload, ensure_ascii=False, separators=(",", ":")
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _string_tuple(value: str) -> tuple[str, ...] | None:
    try:
        parsed = json.loads(value)
    except json.JSONDecodeError:
        return None
    if not isinstance(parsed, list) or not all(isinstance(item, str) for item in parsed):
        return None
    return tuple(parsed)


def load_current_metadata_profiles(
    db: Session,
    tracks: Sequence[Track],
) -> dict[int, TrackAnalysisProfile]:
    """Load only valid profiles whose source metadata has not changed."""

    track_by_id = {track.id: track for track in tracks}
    if not track_by_id:
        return {}
    rows = db.scalars(
        select(TrackAnalysis).where(
            TrackAnalysis.analyzer_id == LOCAL_METADATA_ANALYZER_ID,
            TrackAnalysis.track_id.in_(track_by_id),
        )
    ).all()
    rejected_rows = db.execute(
        select(
            TrackAnalysisTagReview.track_id,
            TrackAnalysisTagReview.source_signature,
            TrackAnalysisTagReview.tag,
        ).where(
            TrackAnalysisTagReview.analyzer_id == LOCAL_METADATA_ANALYZER_ID,
            TrackAnalysisTagReview.track_id.in_(track_by_id),
            TrackAnalysisTagReview.decision == "rejected",
        )
    ).all()
    rejected_tags = {
        (track_id, source_signature, tag)
        for track_id, source_signature, tag in rejected_rows
    }
    profiles: dict[int, TrackAnalysisProfile] = {}
    for row in rows:
        track = track_by_id.get(row.track_id)
        if track is None or row.source_signature != track_source_signature(track):
            continue
        moods = _string_tuple(row.moods_json)
        evidence = _string_tuple(row.evidence_json)
        if moods is None or evidence is None:
            continue
        if row.confidence not in {"high", "medium", "low"}:
            continue
        confidence = cast("Literal['high', 'medium', 'low']", row.confidence)
        visible_moods = tuple(
            tag
            for tag in moods
            if (row.track_id, row.source_signature, tag) not in rejected_tags
        )
        profiles[row.track_id] = TrackAnalysisProfile(
            energy=row.energy,
            brightness=row.brightness,
            tension=row.tension,
            moods=visible_moods,
            evidence=evidence,
            confidence=confidence,
        )
    return profiles


def metadata_analysis_summary(db: Session) -> dict[str, object]:
    """Summarize only metadata profiles that still match indexed fields."""

    tracks = list(db.scalars(select(Track).order_by(Track.id)).all())
    rows = {
        row.track_id: row
        for row in db.scalars(
            select(TrackAnalysis).where(
                TrackAnalysis.analyzer_id == LOCAL_METADATA_ANALYZER_ID
            )
        ).all()
    }
    current: list[TrackAnalysis] = []
    stale_tracks = 0
    for track in tracks:
        row = rows.get(track.id)
        if row is None:
            continue
        if row.source_signature == track_source_signature(track):
            current.append(row)
        else:
            stale_tracks += 1

    confidence_counts = {"high": 0, "medium": 0, "low": 0}
    for row in current:
        if row.confidence in confidence_counts:
            confidence_counts[row.confidence] += 1
    last_updated_at = max((row.updated_at for row in current), default=None)
    return {
        "analyzer": LOCAL_METADATA_ANALYZER_ID,
        "library_tracks": len(tracks),
        "analyzed_tracks": len(current),
        "failed_tracks": 0,
        "stale_tracks": stale_tracks,
        "high_confidence": confidence_counts["high"],
        "medium_confidence": confidence_counts["medium"],
        "low_confidence": confidence_counts["low"],
        "last_updated_at": last_updated_at,
    }


def _chunks(values: Sequence[Track], size: int) -> list[Sequence[Track]]:
    return [values[index : index + size] for index in range(0, len(values), size)]


def run_library_analysis(
    context: JobExecutionContext,
    parameters: dict[str, object],
) -> dict[str, object]:
    """Persist metadata-derived profiles in restart-safe committed chunks."""

    force = parameters.get("force") is True
    with SessionLocal() as db:
        tracks = list(db.scalars(select(Track).order_by(Track.id)).all())
        existing = {
            row.track_id: row
            for row in db.scalars(
                select(TrackAnalysis).where(
                    TrackAnalysis.analyzer_id == LOCAL_METADATA_ANALYZER_ID
                )
            ).all()
        }

    work: list[Track] = []
    signatures: dict[int, str] = {}
    for track in tracks:
        signature = track_source_signature(track)
        signatures[track.id] = signature
        existing_profile = existing.get(track.id)
        if force:
            current = (
                existing_profile is not None
                and existing_profile.job_id == context.job_id
            )
        else:
            current = (
                existing_profile is not None
                and existing_profile.analyzer_id == LOCAL_METADATA_ANALYZER_ID
                and existing_profile.source_signature == signature
            )
        if not current:
            work.append(track)

    starting_progress = context.progress_current
    total = max(context.progress_total or 0, starting_progress + len(work))
    context.update_progress(
        starting_progress,
        total,
        phase="Profiling library",
        message=f"{len(work)} track profiles need updating",
    )

    processed = 0
    for chunk in _chunks(work, _CHUNK_SIZE):
        context.check_cancelled()
        with SessionLocal() as db:
            stored = {
                row.track_id: row
                for row in db.scalars(
                    select(TrackAnalysis).where(
                        TrackAnalysis.analyzer_id == LOCAL_METADATA_ANALYZER_ID,
                        TrackAnalysis.track_id.in_([track.id for track in chunk]),
                    )
                ).all()
            }
            for track in chunk:
                metadata_profile = analyze_track_metadata(track)
                row = stored.get(track.id)
                if row is None:
                    row = TrackAnalysis(
                        track_id=track.id,
                        analyzer_id=LOCAL_METADATA_ANALYZER_ID,
                    )
                    db.add(row)
                row.source_signature = signatures[track.id]
                row.job_id = context.job_id
                row.energy = metadata_profile.energy
                row.brightness = metadata_profile.brightness
                row.tension = metadata_profile.tension
                row.moods_json = json.dumps(
                    list(metadata_profile.moods), ensure_ascii=False
                )
                row.evidence_json = json.dumps(
                    list(metadata_profile.evidence), ensure_ascii=False
                )
                row.confidence = metadata_profile.confidence
                row.updated_at = utcnow()
            db.commit()
        processed += len(chunk)
        context.update_progress(
            min(total, starting_progress + processed),
            total,
            phase="Profiling library",
            message=f"Processed {starting_progress + processed} of {total} tracks",
        )

    if context.progress_current < total:
        context.update_progress(
            total,
            total,
            phase="Profiling library",
            message=f"Processed {total} of {total} tracks",
        )

    with SessionLocal() as db:
        profiles = list(
            db.scalars(
                select(TrackAnalysis).where(
                    TrackAnalysis.analyzer_id == LOCAL_METADATA_ANALYZER_ID
                )
            ).all()
        )
    updated = sum(1 for profile in profiles if profile.job_id == context.job_id)
    current_profiles = len(profiles)
    return {
        "tracks": len(tracks),
        "updated": updated,
        "unchanged": max(0, len(tracks) - updated),
        "current_profiles": current_profiles,
        "analyzer": LOCAL_METADATA_ANALYZER_ID,
    }


register_job_handler(
    ANALYSIS_JOB_KIND,
    run_library_analysis,
    restartable=True,
)
