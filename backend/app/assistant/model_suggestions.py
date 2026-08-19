"""Consent-bound, preview-only live-library model playlist suggestions."""

from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field
from sqlalchemy import select
from sqlalchemy.orm import Session

from app.assistant.analysis import load_current_metadata_profiles
from app.assistant.audio_analysis import load_current_audio_profiles
from app.assistant.model_evaluation import (
    PLAYLIST_QUALITY_EVALUATION_ID,
    prepare_quality_gated_role_execution,
)
from app.assistant.model_playlist import ModelPlaylistPlanner
from app.assistant.providers.execution import (
    StructuredModelRequest,
    StructuredModelResult,
    execute_structured_model_request,
)
from app.assistant.providers.service import ProviderServiceError
from app.assistant.providers.usage import ProviderUsageAccumulator
from app.assistant.schemas import (
    MODEL_PLAYLIST_DISCLOSURE_VERSION,
    ModelPlaylistAvailability,
    ModelPlaylistDisclosure,
    ModelPlaylistSuggestionJobResult,
    PlaylistSuggestionRequest,
)
from app.assistant.tags import load_manual_tags
from app.core.db import SessionLocal
from app.jobs.registry import JobExecutionContext, register_job_handler
from app.models.assistant_model_role import AssistantModelRole
from app.models.assistant_provider_connection import AssistantProviderConnection
from app.models.track import Track

MODEL_PLAYLIST_SUGGESTION_JOB_KIND = "assistant.model-playlist-suggestion"
MODEL_PLAYLIST_ROLE_ID: Literal["playlist_planner"] = "playlist_planner"

MODEL_PLAYLIST_DISCLOSURE = ModelPlaylistDisclosure(
    version=MODEL_PLAYLIST_DISCLOSURE_VERSION,
    shared_with_provider=[
        "Your mood prompt, duration, tempo filters, and requested energy flow",
        "Up to 100 locally prefiltered candidate IDs and descriptive metadata",
        "Candidate titles, artists, albums, origins, genres, durations, and BPM values",
        "Your manual tags, generated analysis tags, and numeric audio-signal summaries",
    ],
    never_shared=[
        "Audio files or cover artwork",
        "Filesystem or library-relative paths",
        "Songs removed by local eligibility, exclusion, and candidate-limit checks",
        "Provider credentials",
    ],
    maximum_candidates=100,
    may_incur_cost=True,
)


class _ModelSuggestionJobParameters(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)

    role_id: Literal["playlist_planner"]
    quality_evaluation_id: Literal["playlist-quality-v1"]
    disclosure_version: Literal["assistant-playlist-model-disclosure/v1"]
    consent: Literal[True]
    role_fingerprint: str = Field(pattern=r"^[a-f0-9]{64}$")
    request: PlaylistSuggestionRequest


def model_playlist_availability(db: Session) -> ModelPlaylistAvailability:
    role = db.get(AssistantModelRole, MODEL_PLAYLIST_ROLE_ID)
    connection = (
        db.get(AssistantProviderConnection, role.connection_id)
        if role is not None
        else None
    )
    reason_code: str | None = None
    try:
        prepare_quality_gated_role_execution(
            db,
            MODEL_PLAYLIST_ROLE_ID,
            PLAYLIST_QUALITY_EVALUATION_ID,
        )
    except ProviderServiceError as exc:
        reason_code = exc.code
    return ModelPlaylistAvailability(
        available=reason_code is None,
        reason_code=reason_code,
        role_id=MODEL_PLAYLIST_ROLE_ID,
        connection_name=connection.name if connection is not None else None,
        model_id=role.model_id if role is not None else None,
        quality_evaluation_id=PLAYLIST_QUALITY_EVALUATION_ID,
        job_kind=MODEL_PLAYLIST_SUGGESTION_JOB_KIND,
        disclosure=MODEL_PLAYLIST_DISCLOSURE,
    )


def model_suggestion_job_parameters(
    db: Session,
    request: PlaylistSuggestionRequest,
) -> dict[str, Any]:
    resolved = prepare_quality_gated_role_execution(
        db,
        MODEL_PLAYLIST_ROLE_ID,
        PLAYLIST_QUALITY_EVALUATION_ID,
    )
    return _ModelSuggestionJobParameters(
        role_id=MODEL_PLAYLIST_ROLE_ID,
        quality_evaluation_id=PLAYLIST_QUALITY_EVALUATION_ID,
        disclosure_version=MODEL_PLAYLIST_DISCLOSURE_VERSION,
        consent=True,
        role_fingerprint=resolved.fingerprint,
        request=request,
    ).model_dump(mode="json")


def _require_unchanged_quality_gate(
    db: Session,
    parameters: _ModelSuggestionJobParameters,
) -> None:
    resolved = prepare_quality_gated_role_execution(
        db,
        parameters.role_id,
        parameters.quality_evaluation_id,
    )
    if resolved.fingerprint != parameters.role_fingerprint:
        raise ProviderServiceError(
            "role_changed",
            "The playlist model changed while the suggestion was running. Run it again.",
            409,
        )


def run_model_playlist_suggestion(
    context: JobExecutionContext,
    raw_parameters: dict[str, Any],
) -> dict[str, Any]:
    parameters = _ModelSuggestionJobParameters.model_validate(raw_parameters)
    context.update_progress(
        0,
        3,
        phase="Loading library evidence",
        message="Reading the current local library snapshot",
    )
    with SessionLocal() as db:
        resolved = prepare_quality_gated_role_execution(
            db,
            parameters.role_id,
            parameters.quality_evaluation_id,
        )
        if resolved.fingerprint != parameters.role_fingerprint:
            raise ProviderServiceError(
                "role_changed",
                "The playlist model changed before the suggestion started. Run it again.",
                409,
            )
        tracks = list(db.scalars(select(Track).order_by(Track.id)).all())
        profiles = load_current_metadata_profiles(db, tracks)
        signal_profiles = load_current_audio_profiles(db, tracks)
        manual_tags = load_manual_tags(db, [track.id for track in tracks])

    context.update_progress(
        1,
        3,
        phase="Filtering locally",
        message=f"Preparing a bounded candidate pool from {len(tracks)} library tracks",
    )
    usage = ProviderUsageAccumulator()

    def execute(request: StructuredModelRequest) -> StructuredModelResult:
        context.check_cancelled()
        context.update_progress(
            2,
            3,
            phase="Waiting for playlist model",
            message="Sending the disclosed, path-free candidate pool",
        )
        return usage.record(
            execute_structured_model_request(resolved.execution, request)
        )

    suggestion = ModelPlaylistPlanner(execute).suggest(
        tracks,
        parameters.request,
        profiles=profiles,
        manual_tags=manual_tags,
        signal_profiles=signal_profiles,
    )
    context.check_cancelled()
    with SessionLocal() as db:
        _require_unchanged_quality_gate(db, parameters)
    context.update_progress(
        3,
        3,
        phase="Draft ready",
        message="The model-ranked draft is ready for your review",
    )
    return ModelPlaylistSuggestionJobResult(
        schema_version="assistant-playlist-suggestion-job-result/v1",
        disclosure_version=parameters.disclosure_version,
        role_id=parameters.role_id,
        role_fingerprint=parameters.role_fingerprint,
        suggestion=suggestion,
        usage=usage.summary(),
    ).model_dump(mode="json")


register_job_handler(
    MODEL_PLAYLIST_SUGGESTION_JOB_KIND,
    run_model_playlist_suggestion,
    restartable=False,
)
