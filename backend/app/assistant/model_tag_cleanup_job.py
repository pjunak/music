"""Consent-bound durable job for review-only model tag cleanup."""

from __future__ import annotations

import hashlib
import math
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field
from sqlalchemy.orm import Session

from app.assistant.model_evaluation import (
    TAG_CLEANUP_QUALITY_EVALUATION_ID,
    prepare_quality_gated_role_execution,
)
from app.assistant.model_tag_cleanup import (
    MAX_MODEL_CLEANUP_TAGS,
    MODEL_TAG_CLEANUP_BATCH_SIZE,
    MODEL_TAG_CLEANUP_ENGINE_ID,
    ModelTagCleanupSuggestion,
    suggest_model_tag_cleanup,
    unresolved_model_cleanup_usage,
)
from app.assistant.providers.execution import (
    StructuredModelRequest,
    StructuredModelResult,
    execute_structured_model_request,
)
from app.assistant.providers.service import ProviderServiceError
from app.assistant.providers.usage import ProviderUsageAccumulator
from app.assistant.tag_cleanup import build_tag_cleanup_preview, tag_catalog_snapshot
from app.assistant.tag_schemas import (
    MODEL_TAG_CLEANUP_DISCLOSURE_VERSION,
    ModelTagCleanupAvailability,
    ModelTagCleanupDisclosure,
    ModelTagCleanupJobResult,
    ModelTagCleanupSuggestionOut,
)
from app.assistant.tag_vocabulary import load_tag_vocabulary
from app.core.db import SessionLocal
from app.jobs.registry import JobExecutionContext, register_job_handler
from app.models.assistant_model_role import AssistantModelRole
from app.models.assistant_provider_connection import AssistantProviderConnection

MODEL_TAG_CLEANUP_ROLE_ID: Literal["tag_cleanup"] = "tag_cleanup"
MODEL_TAG_CLEANUP_JOB_KIND = "assistant.model-tag-cleanup"

MODEL_TAG_CLEANUP_DISCLOSURE = ModelTagCleanupDisclosure(
    version=MODEL_TAG_CLEANUP_DISCLOSURE_VERSION,
    shared_with_provider=[
        "Manual source tags not already resolved by deterministic cleanup rules",
        "The number of tracks using each shared source tag",
        (
            "The operator-managed canonical tag IDs, names, groups, and definitions; "
            "the model may return only those IDs or no match"
        ),
    ],
    never_shared=[
        "Audio or media files",
        "Track titles, artists, albums, metadata, or filesystem paths",
        "Playlists, generated tags, review history, or provider credentials",
    ],
    maximum_tags=MAX_MODEL_CLEANUP_TAGS,
    may_incur_cost=True,
)


class _ModelTagCleanupJobParameters(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)

    role_id: Literal["tag_cleanup"]
    quality_evaluation_id: Literal["tag-cleanup-quality-v1"]
    disclosure_version: Literal["assistant-model-tag-cleanup-disclosure/v3"]
    consent: Literal[True]
    role_fingerprint: str = Field(pattern=r"^[a-f0-9]{64}$")
    catalog_signature: str = Field(pattern=r"^[a-f0-9]{64}$")
    vocabulary_fingerprint: str = Field(pattern=r"^[a-f0-9]{64}$")


def _suggestion_id(
    role_fingerprint: str,
    catalog_signature: str,
    vocabulary_fingerprint: str,
    suggestion: ModelTagCleanupSuggestion,
) -> str:
    value = (
        f"{MODEL_TAG_CLEANUP_ENGINE_ID}\0{role_fingerprint}\0"
        f"{catalog_signature}\0{vocabulary_fingerprint}\0"
        f"{suggestion.source}\0{suggestion.target}"
    )
    return hashlib.sha256(value.encode()).hexdigest()


def model_tag_cleanup_availability(db: Session) -> ModelTagCleanupAvailability:
    role = db.get(AssistantModelRole, MODEL_TAG_CLEANUP_ROLE_ID)
    connection = (
        db.get(AssistantProviderConnection, role.connection_id)
        if role is not None
        else None
    )
    snapshot = tag_catalog_snapshot(db)
    vocabulary = load_tag_vocabulary(db)
    unresolved = unresolved_model_cleanup_usage(snapshot.usage, vocabulary)
    reason_code: str | None = None
    try:
        prepare_quality_gated_role_execution(
            db,
            MODEL_TAG_CLEANUP_ROLE_ID,
            TAG_CLEANUP_QUALITY_EVALUATION_ID,
        )
    except ProviderServiceError as exc:
        reason_code = exc.code
    if reason_code is None and not snapshot.usage:
        reason_code = "tag_catalog_empty"
    if reason_code is None and len(snapshot.usage) > MAX_MODEL_CLEANUP_TAGS:
        reason_code = "tag_catalog_too_large"
    return ModelTagCleanupAvailability(
        available=reason_code is None,
        reason_code=reason_code,
        role_id=MODEL_TAG_CLEANUP_ROLE_ID,
        connection_name=connection.name if connection is not None else None,
        model_id=role.model_id if role is not None else None,
        quality_evaluation_id=TAG_CLEANUP_QUALITY_EVALUATION_ID,
        job_kind=MODEL_TAG_CLEANUP_JOB_KIND,
        catalog_signature=snapshot.signature,
        vocabulary_fingerprint=vocabulary.fingerprint,
        manual_tags=len(snapshot.usage),
        estimated_provider_requests=math.ceil(
            len(unresolved) / MODEL_TAG_CLEANUP_BATCH_SIZE
        ),
        disclosure=MODEL_TAG_CLEANUP_DISCLOSURE,
    )


def model_tag_cleanup_job_parameters(db: Session) -> dict[str, Any]:
    resolved = prepare_quality_gated_role_execution(
        db,
        MODEL_TAG_CLEANUP_ROLE_ID,
        TAG_CLEANUP_QUALITY_EVALUATION_ID,
    )
    snapshot = tag_catalog_snapshot(db)
    vocabulary = load_tag_vocabulary(db)
    if not snapshot.usage:
        raise ProviderServiceError(
            "tag_catalog_empty",
            "Add at least one mood-library tag before requesting model cleanup.",
            409,
        )
    if len(snapshot.usage) > MAX_MODEL_CLEANUP_TAGS:
        raise ProviderServiceError(
            "tag_catalog_too_large",
            f"Model cleanup currently supports at most {MAX_MODEL_CLEANUP_TAGS} tags.",
            409,
        )
    return _ModelTagCleanupJobParameters(
        role_id=MODEL_TAG_CLEANUP_ROLE_ID,
        quality_evaluation_id=TAG_CLEANUP_QUALITY_EVALUATION_ID,
        disclosure_version=MODEL_TAG_CLEANUP_DISCLOSURE_VERSION,
        consent=True,
        role_fingerprint=resolved.fingerprint,
        catalog_signature=snapshot.signature,
        vocabulary_fingerprint=vocabulary.fingerprint,
    ).model_dump(mode="json")


def _require_unchanged_role(
    db: Session,
    parameters: _ModelTagCleanupJobParameters,
) -> None:
    resolved = prepare_quality_gated_role_execution(
        db,
        parameters.role_id,
        parameters.quality_evaluation_id,
    )
    if resolved.fingerprint != parameters.role_fingerprint:
        raise ProviderServiceError(
            "role_changed",
            "The tag cleanup model changed while the job was running. Run it again.",
            409,
        )
    if load_tag_vocabulary(db).fingerprint != parameters.vocabulary_fingerprint:
        raise ProviderServiceError(
            "tag_vocabulary_changed",
            "The tag vocabulary changed while cleanup was running. Run it again.",
            409,
        )


def run_model_tag_cleanup(
    context: JobExecutionContext,
    raw_parameters: dict[str, Any],
) -> dict[str, Any]:
    parameters = _ModelTagCleanupJobParameters.model_validate(raw_parameters)
    with SessionLocal() as db:
        resolved = prepare_quality_gated_role_execution(
            db,
            parameters.role_id,
            parameters.quality_evaluation_id,
        )
        if resolved.fingerprint != parameters.role_fingerprint:
            raise ProviderServiceError(
                "role_changed",
                "The tag cleanup model changed before the job started. Run it again.",
                409,
            )
        snapshot = tag_catalog_snapshot(db)
        vocabulary = load_tag_vocabulary(db)
    if snapshot.signature != parameters.catalog_signature:
        raise ProviderServiceError(
            "tag_catalog_changed",
            "Mood-library tags changed before model cleanup started. Run it again.",
            409,
        )
    if vocabulary.fingerprint != parameters.vocabulary_fingerprint:
        raise ProviderServiceError(
            "tag_vocabulary_changed",
            "The tag vocabulary changed before model cleanup started. Run it again.",
            409,
        )

    context.update_progress(
        0,
        1,
        phase="Preparing tag catalog",
        message=f"Preparing {len(snapshot.usage)} mood-library tags for review",
    )
    usage = ProviderUsageAccumulator()

    def execute(request: StructuredModelRequest) -> StructuredModelResult:
        context.check_cancelled()
        result = usage.record(
            execute_structured_model_request(resolved.execution, request)
        )
        context.checkpoint_result(usage.checkpoint())
        return result

    unresolved = unresolved_model_cleanup_usage(snapshot.usage, vocabulary)
    context.update_progress(
        0,
        1,
        phase=(
            "Waiting for tag cleanup model"
            if unresolved
            else "Applying deterministic cleanup rules"
        ),
        message=(
            f"The provider is reviewing {len(unresolved)} unresolved tag names"
            if unresolved
            else "All cleanup candidates were resolved locally"
        ),
    )
    suggestions = suggest_model_tag_cleanup(snapshot.usage, execute, vocabulary)
    context.check_cancelled()
    with SessionLocal() as db:
        _require_unchanged_role(db, parameters)
        if tag_catalog_snapshot(db).signature != parameters.catalog_signature:
            raise ProviderServiceError(
                "tag_catalog_changed",
                "Mood-library tags changed while model cleanup was running. Run it again.",
                409,
            )

    counts = {item.tag: item.track_count for item in snapshot.usage}
    local_pairs = {
        (item.source, item.target)
        for item in build_tag_cleanup_preview(
            snapshot.usage,
            vocabulary,
        ).suggestions
    }
    output = [
        ModelTagCleanupSuggestionOut(
            id=_suggestion_id(
                parameters.role_fingerprint,
                parameters.catalog_signature,
                parameters.vocabulary_fingerprint,
                suggestion,
            ),
            source=suggestion.source,
            target=suggestion.target,
            origin=(
                "local-rule"
                if (suggestion.source, suggestion.target) in local_pairs
                else "model"
            ),
            confidence=suggestion.confidence,
            reason=suggestion.reason,
            source_track_count=counts[suggestion.source],
            target_track_count=counts.get(suggestion.target, 0),
            merged=suggestion.target in counts,
        )
        for suggestion in suggestions
    ]
    context.update_progress(
        1,
        1,
        phase="Saving cleanup proposal",
        message=f"Saved {len(output)} review-only suggestions",
    )
    return ModelTagCleanupJobResult(
        schema_version="assistant-model-tag-cleanup-job-result/v3",
        disclosure_version=parameters.disclosure_version,
        role_id=parameters.role_id,
        role_fingerprint=parameters.role_fingerprint,
        engine_id=MODEL_TAG_CLEANUP_ENGINE_ID,
        catalog_signature=parameters.catalog_signature,
        vocabulary_fingerprint=parameters.vocabulary_fingerprint,
        catalog_tags=len(snapshot.usage),
        suggestions=output,
        usage=usage.summary(),
    ).model_dump(mode="json")
register_job_handler(
    MODEL_TAG_CLEANUP_JOB_KIND,
    run_model_tag_cleanup,
    restartable=False,
)
