"""Consent-bound durable model metadata tagging for review-only suggestions."""

from __future__ import annotations

import hashlib
import json
import math
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field
from sqlalchemy import func, select
from sqlalchemy.orm import Session

from app.assistant.analysis import track_source_signature
from app.assistant.library_context import (
    CurrentTrackContext,
    compact_context_projection,
    load_current_contexts,
)
from app.assistant.model_evaluation import (
    TAGGING_QUALITY_EVALUATION_ID,
    prepare_quality_gated_role_execution,
)
from app.assistant.model_tagger import (
    MODEL_TAG_ANALYZER_ID,
    MODEL_TAG_BATCH_SIZE,
    MODEL_TAGGER_INPUT_CONTRACT,
    MODEL_TAGGER_INVALID_RESPONSE_RETRY_LIMIT,
    ModelTagContextEvidence,
    ModelTaggerRetryBudget,
    ModelTagTrackInput,
    tag_tracks,
)
from app.assistant.providers.execution import (
    StructuredModelRequest,
    StructuredModelResult,
    execute_structured_model_request,
)
from app.assistant.providers.service import (
    ProviderServiceError,
    current_role_runtime_fingerprint,
)
from app.assistant.providers.usage import ProviderUsageAccumulator
from app.assistant.tag_schemas import (
    MODEL_TAGGING_DISCLOSURE_VERSION,
    ModelTaggingAvailability,
    ModelTaggingContextPolicy,
    ModelTaggingDisclosure,
    ModelTaggingJobResult,
    ModelTaggingScope,
)
from app.assistant.tag_vocabulary import (
    TagVocabularySnapshot,
    load_tag_vocabulary,
)
from app.core.db import SessionLocal
from app.jobs.registry import JobExecutionContext, register_job_handler
from app.library import index as library_index
from app.models.assistant_model_role import AssistantModelRole
from app.models.assistant_provider_connection import AssistantProviderConnection
from app.models.base import utcnow
from app.models.track import Track
from app.models.track_analysis import TrackAnalysis

MODEL_TAGGING_JOB_KIND = "assistant.model-music-tagging"
MODEL_TAGGING_ROLE_ID: Literal["music_tagger"] = "music_tagger"


def resolve_model_tagging_scope(
    db: Session,
    scope: ModelTaggingScope,
) -> list[Track]:
    """Resolve a validated library-relative scope without touching media files."""

    if scope.type == "tracks":
        rows = {
            track.id: track
            for track in db.scalars(
                select(Track).where(Track.id.in_(scope.track_ids))
            ).all()
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
            stmt = select(Track).where(
                Track.path.like(f"{prefix}%", escape=escape)
            )
            if not scope.recursive:
                stmt = stmt.where(
                    ~Track.path.like(f"{prefix}%/%", escape=escape)
                )
        return list(db.scalars(stmt.order_by(Track.id)).all())
    return list(db.scalars(select(Track).order_by(Track.id)).all())


def _model_tagging_disclosure(
    vocabulary: TagVocabularySnapshot,
) -> ModelTaggingDisclosure:
    return ModelTaggingDisclosure(
        version=MODEL_TAGGING_DISCLOSURE_VERSION,
        shared_with_provider=[
            "Indexed titles, display titles, artists, albums, origins, and genres",
            (
                "Canonical library-relative paths, including folder and file names, "
                "treated as untrusted descriptive context"
            ),
            "Track durations and BPM values when available",
            (
                "Current bounded local track context when available: intensity, loudness, "
                "rhythmic drive, brightness, density and spectral-change trajectories; "
                "tempo development; major acoustic sections and transitions; structural "
                "repetition; analyzer confidence; and optional local voice/instrumental "
                "classifier score and coverage (or explicit unknown/unavailable status)"
            ),
            "A server-assigned numeric track ID used only to match the response",
            (
                "The full operator-managed canonical tag ID, name, group, definition, "
                "exact-alias, and bounded semantic context cue index; the model may return "
                "only IDs from this index"
            ),
        ],
        never_shared=[
            "Audio files, waveforms, full-resolution timelines, spectrograms, or cover artwork",
            "The absolute media root or filesystem paths outside the indexed library",
            (
                "Your database mood tags, generated-tag review decisions, or accepted/rejected "
                "state"
            ),
            "Playlists, review decisions, and provider credentials",
        ],
        allowed_tags=[tag.name for tag in vocabulary.entries],
        tracks_per_request=MODEL_TAG_BATCH_SIZE,
        invalid_response_retry_limit=MODEL_TAGGER_INVALID_RESPONSE_RETRY_LIMIT,
        may_incur_cost=True,
    )


class _ModelTaggingJobParameters(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)

    role_id: Literal["music_tagger"]
    quality_evaluation_id: Literal["music-tagging-quality-v1"]
    disclosure_version: Literal["assistant-model-music-tagging-disclosure/v10"]
    consent: Literal[True]
    role_fingerprint: str = Field(pattern=r"^[a-f0-9]{64}$")
    vocabulary_fingerprint: str = Field(pattern=r"^[a-f0-9]{64}$")
    scope: ModelTaggingScope
    context_policy: ModelTaggingContextPolicy
    force: bool


def model_tag_source_signature(
    track: Track,
    role_fingerprint: str,
    vocabulary_fingerprint: str,
    track_context: CurrentTrackContext | None,
) -> str:
    """Bind a generated profile to exact metadata, optional audio evidence, and runtime."""

    context_signature = (
        track_context.source_signature if track_context is not None else "no-track-context"
    )
    payload = (
        f"{MODEL_TAG_ANALYZER_ID}\0{role_fingerprint}\0"
        f"{vocabulary_fingerprint}\0{track_source_signature(track)}\0{context_signature}"
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def _current_profile_count(
    db: Session,
    tracks: list[Track],
    fingerprint: str | None,
    vocabulary_fingerprint: str,
    contexts: dict[int, CurrentTrackContext],
) -> int:
    if not tracks or fingerprint is None:
        return 0
    rows = {
        row.track_id: row
        for row in db.scalars(
            select(TrackAnalysis).where(
                TrackAnalysis.analyzer_id == MODEL_TAG_ANALYZER_ID
            )
        ).all()
    }
    return sum(
        rows.get(track.id) is not None
        and rows[track.id].source_signature
        == model_tag_source_signature(
            track,
            fingerprint,
            vocabulary_fingerprint,
            contexts.get(track.id),
        )
        for track in tracks
    )


def model_tagging_availability(
    db: Session,
    scope: ModelTaggingScope | None = None,
    context_policy: ModelTaggingContextPolicy = "include",
) -> ModelTaggingAvailability:
    scope = scope or ModelTaggingScope()
    role = db.get(AssistantModelRole, MODEL_TAGGING_ROLE_ID)
    connection = (
        db.get(AssistantProviderConnection, role.connection_id)
        if role is not None
        else None
    )
    tracks = resolve_model_tagging_scope(db, scope)
    library_tracks = int(db.scalar(select(func.count()).select_from(Track)) or 0)
    vocabulary = load_tag_vocabulary(db)
    contexts = load_current_contexts(db, tracks)
    full_contexts = {
        track_id: context
        for track_id, context in contexts.items()
        if context.completeness == "full"
    }
    partial_contexts = {
        track_id: context
        for track_id, context in contexts.items()
        if context.completeness == "partial"
    }
    planned_tracks = (
        [track for track in tracks if track.id in full_contexts]
        if context_policy == "skip"
        else tracks
    )
    reason_code: str | None = None
    fingerprint: str | None = None
    try:
        resolved = prepare_quality_gated_role_execution(
            db,
            MODEL_TAGGING_ROLE_ID,
            TAGGING_QUALITY_EVALUATION_ID,
        )
        fingerprint = resolved.fingerprint
    except ProviderServiceError as exc:
        reason_code = exc.code
        fingerprint = current_role_runtime_fingerprint(db, MODEL_TAGGING_ROLE_ID)
    current = _current_profile_count(
        db,
        planned_tracks,
        fingerprint,
        vocabulary.fingerprint,
        contexts,
    )
    needed = max(0, len(planned_tracks) - current)
    return ModelTaggingAvailability(
        available=reason_code is None,
        reason_code=reason_code,
        role_id=MODEL_TAGGING_ROLE_ID,
        connection_name=connection.name if connection is not None else None,
        model_id=role.model_id if role is not None else None,
        quality_evaluation_id=TAGGING_QUALITY_EVALUATION_ID,
        job_kind=MODEL_TAGGING_JOB_KIND,
        library_tracks=library_tracks,
        scope_tracks=len(tracks),
        planned_tracks=len(planned_tracks),
        tracks_with_full_context=len(full_contexts),
        tracks_with_partial_context=len(partial_contexts),
        tracks_missing_context=max(0, len(tracks) - len(contexts)),
        current_profiles=current,
        tracks_needing_tags=needed,
        estimated_provider_requests=math.ceil(needed / MODEL_TAG_BATCH_SIZE),
        disclosure=_model_tagging_disclosure(vocabulary),
    )


def model_tagging_job_parameters(
    db: Session,
    *,
    force: bool,
    scope: ModelTaggingScope | None = None,
    context_policy: ModelTaggingContextPolicy = "include",
) -> dict[str, Any]:
    resolved = prepare_quality_gated_role_execution(
        db,
        MODEL_TAGGING_ROLE_ID,
        TAGGING_QUALITY_EVALUATION_ID,
    )
    vocabulary = load_tag_vocabulary(db)
    return _ModelTaggingJobParameters(
        role_id=MODEL_TAGGING_ROLE_ID,
        quality_evaluation_id=TAGGING_QUALITY_EVALUATION_ID,
        disclosure_version=MODEL_TAGGING_DISCLOSURE_VERSION,
        consent=True,
        role_fingerprint=resolved.fingerprint,
        vocabulary_fingerprint=vocabulary.fingerprint,
        scope=scope or ModelTaggingScope(),
        context_policy=context_policy,
        force=force,
    ).model_dump(mode="json")


def _require_unchanged_quality_gate(
    db: Session,
    parameters: _ModelTaggingJobParameters,
) -> None:
    resolved = prepare_quality_gated_role_execution(
        db,
        parameters.role_id,
        parameters.quality_evaluation_id,
    )
    if resolved.fingerprint != parameters.role_fingerprint:
        raise ProviderServiceError(
            "role_changed",
            "The mood-tagging model changed while the job was running. Run it again.",
            409,
        )
    if load_tag_vocabulary(db).fingerprint != parameters.vocabulary_fingerprint:
        raise ProviderServiceError(
            "tag_vocabulary_changed",
            "The tag vocabulary changed while the job was running. Run it again.",
            409,
        )


def _track_input(
    track: Track,
    track_context: CurrentTrackContext | None,
) -> ModelTagTrackInput:
    context_evidence = (
        ModelTagContextEvidence.model_validate(compact_context_projection(track_context))
        if track_context is not None
        else None
    )
    return ModelTagTrackInput(
        track_id=track.id,
        title=track.title,
        display_title=track.display_title,
        artist=track.artist,
        album=track.album,
        origin=track.origin,
        genre=track.genre,
        library_path=track.path,
        length_s=track.length_s,
        bpm=track.bpm,
        context_evidence=context_evidence,
    )


def _local_context_axes(
    track_context: CurrentTrackContext | None,
) -> tuple[float, float, float]:
    if track_context is None:
        return 0.5, 0.5, 0.5
    trajectories = track_context.summary.get("trajectories")
    if not isinstance(trajectories, dict):
        return 0.5, 0.5, 0.5

    def value(name: str, field: str, default: float) -> float:
        trajectory = trajectories.get(name)
        if not isinstance(trajectory, dict):
            return default
        raw = trajectory.get(field)
        if (
            not isinstance(raw, (int, float))
            or isinstance(raw, bool)
            or not math.isfinite(raw)
        ):
            return default
        return max(0.0, min(1.0, float(raw)))

    energy = value("intensity", "typical", 0.5)
    brightness = value("brightness", "typical", 0.5)
    tension_proxy = max(
        value("intensity", "variability", 0.5),
        value("rhythmic_drive", "typical", 0.5),
    )
    return round(energy, 6), round(brightness, 6), round(tension_proxy, 6)


def run_model_music_tagging(
    context: JobExecutionContext,
    raw_parameters: dict[str, Any],
) -> dict[str, Any]:
    parameters = _ModelTaggingJobParameters.model_validate(raw_parameters)
    with SessionLocal() as db:
        resolved = prepare_quality_gated_role_execution(
            db,
            parameters.role_id,
            parameters.quality_evaluation_id,
        )
        if resolved.fingerprint != parameters.role_fingerprint:
            raise ProviderServiceError(
                "role_changed",
                "The mood-tagging model changed before the job started. Run it again.",
                409,
            )
        vocabulary = load_tag_vocabulary(db)
        if vocabulary.fingerprint != parameters.vocabulary_fingerprint:
            raise ProviderServiceError(
                "tag_vocabulary_changed",
                "The tag vocabulary changed before the job started. Run it again.",
                409,
            )
        tracks = resolve_model_tagging_scope(db, parameters.scope)
        library_tracks = int(db.scalar(select(func.count()).select_from(Track)) or 0)
        existing = {
            row.track_id: row
            for row in db.scalars(
                select(TrackAnalysis).where(
                    TrackAnalysis.analyzer_id == MODEL_TAG_ANALYZER_ID
                )
            ).all()
        }
        contexts = load_current_contexts(db, tracks)

    scoped_tracks = tracks
    if parameters.context_policy == "skip":
        tracks = [
            track
            for track in tracks
            if (track_context := contexts.get(track.id)) is not None
            and track_context.completeness == "full"
        ]
    skipped_context_tracks = len(scoped_tracks) - len(tracks)

    signatures = {
        track.id: model_tag_source_signature(
            track,
            parameters.role_fingerprint,
            parameters.vocabulary_fingerprint,
            contexts.get(track.id),
        )
        for track in tracks
    }
    work = [
        track
        for track in tracks
        if parameters.force
        or existing.get(track.id) is None
        or existing[track.id].source_signature != signatures[track.id]
    ]
    total = len(work)
    context.update_progress(
        0,
        total,
        phase="Preparing metadata batches",
        message=(
            f"{total} of {len(tracks)} tracks need model tag suggestions"
            if total
            else "All model tag suggestions are current"
        ),
    )

    updated = 0
    skipped_changed = 0
    usage = ProviderUsageAccumulator()
    retry_budget = ModelTaggerRetryBudget()
    for start in range(0, total, MODEL_TAG_BATCH_SIZE):
        context.check_cancelled()
        batch = work[start : start + MODEL_TAG_BATCH_SIZE]
        with SessionLocal() as db:
            _require_unchanged_quality_gate(db, parameters)
        context.update_progress(
            start,
            total,
            phase="Waiting for mood-tagging model",
            message=(
                f"Classifying tracks {start + 1}-{start + len(batch)} of {total}"
            ),
        )

        def execute(request: StructuredModelRequest) -> StructuredModelResult:
            context.check_cancelled()
            result = usage.record(
                execute_structured_model_request(resolved.execution, request)
            )
            context.checkpoint_result(usage.checkpoint())
            return result

        profiles = tag_tracks(
            [
                _track_input(track, contexts.get(track.id))
                for track in batch
            ],
            execute,
            vocabulary,
            retry_budget=retry_budget,
        )
        context.check_cancelled()
        with SessionLocal() as db:
            _require_unchanged_quality_gate(db, parameters)
            stored = {
                row.track_id: row
                for row in db.scalars(
                    select(TrackAnalysis).where(
                        TrackAnalysis.analyzer_id == MODEL_TAG_ANALYZER_ID,
                        TrackAnalysis.track_id.in_([track.id for track in batch]),
                    )
                ).all()
            }
            current_tracks = {
                snapshot.id: current_track
                for snapshot in batch
                if (current_track := db.get(Track, snapshot.id)) is not None
            }
            current_contexts = load_current_contexts(
                db,
                list(current_tracks.values()),
            )
            for snapshot in batch:
                current_track = current_tracks.get(snapshot.id)
                if (
                    current_track is None
                    or model_tag_source_signature(
                        current_track,
                        parameters.role_fingerprint,
                        parameters.vocabulary_fingerprint,
                        current_contexts.get(snapshot.id),
                    )
                    != signatures[snapshot.id]
                ):
                    skipped_changed += 1
                    continue
                profile = profiles[snapshot.id]
                row = stored.get(snapshot.id)
                if row is None:
                    row = TrackAnalysis(
                        track_id=snapshot.id,
                        analyzer_id=MODEL_TAG_ANALYZER_ID,
                    )
                    db.add(row)
                row.source_signature = signatures[snapshot.id]
                row.job_id = context.job_id
                row.energy, row.brightness, row.tension = _local_context_axes(
                    current_contexts.get(snapshot.id)
                )
                row.moods_json = json.dumps(profile.tags, ensure_ascii=False)
                row.evidence_json = json.dumps(profile.evidence, ensure_ascii=False)
                row.metrics_json = json.dumps(
                    {
                        "contract": "assistant-music-tagger-output/v3",
                        "input_contract": MODEL_TAGGER_INPUT_CONTRACT,
                        "context_status": (
                            current_contexts[snapshot.id].completeness
                            if snapshot.id in current_contexts
                            else "missing"
                        ),
                        "role_fingerprint": parameters.role_fingerprint,
                        "vocabulary_fingerprint": (
                            parameters.vocabulary_fingerprint
                        ),
                    },
                    separators=(",", ":"),
                )
                row.confidence = profile.confidence
                row.updated_at = utcnow()
                updated += 1
            db.commit()
        context.update_progress(
            start + len(batch),
            total,
            phase="Saving reviewable suggestions",
            message=f"Processed {start + len(batch)} of {total} tracks",
        )

    return ModelTaggingJobResult(
        schema_version="assistant-model-music-tagging-job-result/v6",
        disclosure_version=parameters.disclosure_version,
        role_id=parameters.role_id,
        role_fingerprint=parameters.role_fingerprint,
        analyzer_id=MODEL_TAG_ANALYZER_ID,
        vocabulary_fingerprint=parameters.vocabulary_fingerprint,
        library_tracks=library_tracks,
        scope=parameters.scope,
        scope_tracks=len(scoped_tracks),
        context_policy=parameters.context_policy,
        skipped_context_tracks=skipped_context_tracks,
        updated_profiles=updated,
        unchanged_profiles=max(0, len(tracks) - len(work)),
        skipped_changed_tracks=skipped_changed,
        usage=usage.summary(),
    ).model_dump(mode="json")


register_job_handler(
    MODEL_TAGGING_JOB_KIND,
    run_model_music_tagging,
    restartable=False,
)
