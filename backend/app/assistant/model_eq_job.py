"""Consent-bound durable job for creating a review-only EQ preset draft."""

from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field
from sqlalchemy.orm import Session

from app.assistant.model_eq import EQ_DRAFT_ENGINE_ID, generate_eq_draft
from app.assistant.model_evaluation import (
    EQ_QUALITY_EVALUATION_ID,
    prepare_quality_gated_role_execution,
)
from app.assistant.providers.execution import (
    StructuredModelRequest,
    StructuredModelResult,
    execute_structured_model_request,
)
from app.assistant.providers.service import ProviderServiceError
from app.assistant.providers.usage import ProviderUsageAccumulator
from app.assistant.schemas import (
    MODEL_EQ_DISCLOSURE_VERSION,
    EqDraftRequest,
    ModelEqAvailability,
    ModelEqDisclosure,
    ModelEqDraftJobResult,
)
from app.core.db import SessionLocal
from app.jobs.registry import JobExecutionContext, register_job_handler
from app.models.assistant_model_role import AssistantModelRole
from app.models.assistant_provider_connection import AssistantProviderConnection

MODEL_EQ_ROLE_ID: Literal["eq_assistant"] = "eq_assistant"
MODEL_EQ_DRAFT_JOB_KIND = "assistant.model-eq-draft"

MODEL_EQ_DISCLOSURE = ModelEqDisclosure(
    version=MODEL_EQ_DISCLOSURE_VERSION,
    shared_with_provider=[
        "The preset goal you type",
        "The fixed ten EQ band frequencies and supported gain limits",
        "A deterministic local baseline and per-band safety envelope derived from the goal",
    ],
    never_shared=[
        "Songs, audio, waveforms, or library metadata",
        "Filesystem paths, playlists, existing presets, or provider credentials",
    ],
    may_incur_cost=True,
)


class _ModelEqJobParameters(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)

    role_id: Literal["eq_assistant"]
    quality_evaluation_id: Literal["eq-quality-v1"]
    disclosure_version: Literal["assistant-eq-draft-disclosure/v2"]
    consent: Literal[True]
    role_fingerprint: str = Field(pattern=r"^[a-f0-9]{64}$")
    request: EqDraftRequest


def model_eq_availability(db: Session) -> ModelEqAvailability:
    role = db.get(AssistantModelRole, MODEL_EQ_ROLE_ID)
    connection = (
        db.get(AssistantProviderConnection, role.connection_id)
        if role is not None
        else None
    )
    reason_code: str | None = None
    try:
        prepare_quality_gated_role_execution(
            db,
            MODEL_EQ_ROLE_ID,
            EQ_QUALITY_EVALUATION_ID,
        )
    except ProviderServiceError as exc:
        reason_code = exc.code
    return ModelEqAvailability(
        available=reason_code is None,
        reason_code=reason_code,
        role_id=MODEL_EQ_ROLE_ID,
        connection_name=connection.name if connection is not None else None,
        model_id=role.model_id if role is not None else None,
        quality_evaluation_id=EQ_QUALITY_EVALUATION_ID,
        job_kind=MODEL_EQ_DRAFT_JOB_KIND,
        disclosure=MODEL_EQ_DISCLOSURE,
    )


def model_eq_job_parameters(db: Session, request: EqDraftRequest) -> dict[str, Any]:
    resolved = prepare_quality_gated_role_execution(
        db,
        MODEL_EQ_ROLE_ID,
        EQ_QUALITY_EVALUATION_ID,
    )
    return _ModelEqJobParameters(
        role_id=MODEL_EQ_ROLE_ID,
        quality_evaluation_id=EQ_QUALITY_EVALUATION_ID,
        disclosure_version=MODEL_EQ_DISCLOSURE_VERSION,
        consent=True,
        role_fingerprint=resolved.fingerprint,
        request=request,
    ).model_dump(mode="json")


def _require_unchanged_role(
    db: Session,
    parameters: _ModelEqJobParameters,
) -> None:
    resolved = prepare_quality_gated_role_execution(
        db,
        parameters.role_id,
        parameters.quality_evaluation_id,
    )
    if resolved.fingerprint != parameters.role_fingerprint:
        raise ProviderServiceError(
            "role_changed",
            "The EQ model changed while the draft was running. Run it again.",
            409,
        )


def run_model_eq_draft(
    context: JobExecutionContext,
    raw_parameters: dict[str, Any],
) -> dict[str, Any]:
    parameters = _ModelEqJobParameters.model_validate(raw_parameters)
    context.update_progress(
        0,
        2,
        phase="Preparing EQ request",
        message="Validating the fixed graphic-EQ contract",
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
                "The EQ model changed before the draft started. Run it again.",
                409,
            )
    usage = ProviderUsageAccumulator()

    def execute(request: StructuredModelRequest) -> StructuredModelResult:
        context.check_cancelled()
        context.update_progress(
            1,
            2,
            phase="Waiting for EQ model",
            message="Sending only the disclosed sound goal and fixed EQ limits",
        )
        result = usage.record(
            execute_structured_model_request(resolved.execution, request)
        )
        context.checkpoint_result(usage.checkpoint())
        return result

    draft = generate_eq_draft(
        parameters.request.name,
        parameters.request.goal,
        execute,
    )
    context.check_cancelled()
    with SessionLocal() as db:
        _require_unchanged_role(db, parameters)
    context.update_progress(
        2,
        2,
        phase="Draft ready",
        message="The EQ draft is ready for Authoring review",
    )
    return ModelEqDraftJobResult(
        schema_version="assistant-eq-draft-job-result/v1",
        disclosure_version=parameters.disclosure_version,
        role_id=parameters.role_id,
        role_fingerprint=parameters.role_fingerprint,
        engine_id=EQ_DRAFT_ENGINE_ID,
        draft=draft,
        usage=usage.summary(),
    ).model_dump(mode="json")


register_job_handler(
    MODEL_EQ_DRAFT_JOB_KIND,
    run_model_eq_draft,
    restartable=False,
)
