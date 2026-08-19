"""Durable, synthetic quality gates for optional Assistant model roles."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any, Literal, cast

from pydantic import BaseModel, ConfigDict, Field
from sqlalchemy.orm import Session

from app.assistant.evaluation import (
    PlaylistEvaluationResult,
    evaluate_playlist_engine,
    load_evaluation_suite,
)
from app.assistant.model_playlist import ModelPlaylistPlanner
from app.assistant.providers.definitions import MODEL_ROLE_BY_ID
from app.assistant.providers.execution import (
    StructuredModelRequest,
    StructuredModelResult,
    execute_structured_model_request,
)
from app.assistant.providers.schemas import ModelQualityEvaluationOut
from app.assistant.providers.service import (
    ProviderServiceError,
    current_role_runtime_fingerprint,
    prepare_role_execution_details,
)
from app.core.db import SessionLocal
from app.jobs.registry import JobExecutionContext, register_job_handler
from app.models.assistant_model_evaluation import AssistantModelEvaluation
from app.models.base import utcnow

PLAYLIST_QUALITY_EVALUATION_ID = "playlist-quality-v1"
PLAYLIST_QUALITY_JOB_KIND = "assistant.model-evaluation.playlist-quality-v1"
_PLAYLIST_SUITE_PATH = (
    Path(__file__).resolve().parents[2] / "evaluation" / "playlist-local-v1.json"
)


@dataclass(frozen=True)
class ModelEvaluationDefinition:
    id: str
    role_id: str
    label: str
    description: str
    suite_id: str
    suite_path: Path
    job_kind: str


PLAYLIST_QUALITY_EVALUATION = ModelEvaluationDefinition(
    id=PLAYLIST_QUALITY_EVALUATION_ID,
    role_id="playlist_planner",
    label="Playlist planning quality",
    description=(
        "Runs fixed synthetic D&D playlist scenarios through this model. "
        "No songs or live library data are sent."
    ),
    suite_id="local-dnd-playlist-baseline",
    suite_path=_PLAYLIST_SUITE_PATH,
    job_kind=PLAYLIST_QUALITY_JOB_KIND,
)

_EVALUATIONS_BY_ROLE: dict[str, tuple[ModelEvaluationDefinition, ...]] = {
    PLAYLIST_QUALITY_EVALUATION.role_id: (PLAYLIST_QUALITY_EVALUATION,),
}


class _EvaluationJobParameters(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)

    role_id: Literal["playlist_planner"]
    evaluation_id: Literal["playlist-quality-v1"]
    role_fingerprint: str = Field(pattern=r"^[a-f0-9]{64}$")


def evaluation_definitions_for_role(
    role_id: str,
) -> tuple[ModelEvaluationDefinition, ...]:
    if role_id not in MODEL_ROLE_BY_ID:
        raise ProviderServiceError("role_not_found", "Model role not found.", 404)
    return _EVALUATIONS_BY_ROLE.get(role_id, ())


def require_evaluation_definition(
    role_id: str,
    evaluation_id: str,
) -> ModelEvaluationDefinition:
    definition = next(
        (
            item
            for item in evaluation_definitions_for_role(role_id)
            if item.id == evaluation_id
        ),
        None,
    )
    if definition is None:
        raise ProviderServiceError(
            "evaluation_not_found",
            "Model quality evaluation not found for this role.",
            404,
        )
    return definition


def _current_role_fingerprint(db: Session, role_id: str) -> str | None:
    return current_role_runtime_fingerprint(db, role_id)


def evaluation_out(
    db: Session,
    definition: ModelEvaluationDefinition,
) -> ModelQualityEvaluationOut:
    row = db.get(
        AssistantModelEvaluation,
        (definition.role_id, definition.id),
    )
    status: Literal["never", "passed", "failed", "stale"] = "never"
    if row is not None:
        current_fingerprint = _current_role_fingerprint(db, definition.role_id)
        if current_fingerprint != row.role_fingerprint:
            status = "stale"
        elif row.status in {"passed", "failed"}:
            status = cast(
                "Literal['never', 'passed', 'failed', 'stale']",
                row.status,
            )
    return ModelQualityEvaluationOut(
        evaluation_id=definition.id,
        role_id=definition.role_id,
        label=definition.label,
        description=definition.description,
        status=status,
        suite_id=row.suite_id if row is not None else definition.suite_id,
        passed_cases=row.passed_cases if row is not None else 0,
        total_cases=row.total_cases if row is not None else 0,
        last_job_id=row.job_id if row is not None else None,
        last_evaluated_at=row.evaluated_at if row is not None else None,
    )


def list_role_evaluations(
    db: Session,
    role_id: str,
) -> list[ModelQualityEvaluationOut]:
    return [
        evaluation_out(db, definition)
        for definition in evaluation_definitions_for_role(role_id)
    ]


def evaluation_job_parameters(
    db: Session,
    role_id: str,
    evaluation_id: str,
) -> tuple[ModelEvaluationDefinition, dict[str, Any]]:
    definition = require_evaluation_definition(role_id, evaluation_id)
    resolved = prepare_role_execution_details(db, role_id)
    parameters = _EvaluationJobParameters(
        role_id="playlist_planner",
        evaluation_id="playlist-quality-v1",
        role_fingerprint=resolved.fingerprint,
    )
    return definition, parameters.model_dump(mode="json")


def _record_result(
    context: JobExecutionContext,
    parameters: _EvaluationJobParameters,
    result: PlaylistEvaluationResult,
) -> None:
    with SessionLocal() as db:
        resolved = prepare_role_execution_details(db, parameters.role_id)
        if resolved.fingerprint != parameters.role_fingerprint:
            raise ProviderServiceError(
                "role_changed",
                "The model role changed during evaluation. Run it again.",
                409,
            )
        row = db.get(
            AssistantModelEvaluation,
            (parameters.role_id, parameters.evaluation_id),
        )
        if row is None:
            row = AssistantModelEvaluation(
                role_id=parameters.role_id,
                evaluation_id=parameters.evaluation_id,
                role_fingerprint=parameters.role_fingerprint,
                status="never",
                suite_id=result.suite_id,
                engine_id=result.engine_id,
                passed_cases=0,
                total_cases=0,
                job_id=context.job_id,
            )
            db.add(row)
        row.role_fingerprint = parameters.role_fingerprint
        row.status = "passed" if result.passed else "failed"
        row.suite_id = result.suite_id
        row.engine_id = result.engine_id
        row.passed_cases = result.summary.passed_cases
        row.total_cases = result.summary.cases
        row.job_id = context.job_id
        row.evaluated_at = utcnow()
        db.commit()


def run_playlist_quality_evaluation(
    context: JobExecutionContext,
    raw_parameters: dict[str, Any],
) -> dict[str, Any]:
    parameters = _EvaluationJobParameters.model_validate(raw_parameters)
    definition = require_evaluation_definition(
        parameters.role_id,
        parameters.evaluation_id,
    )
    suite = load_evaluation_suite(definition.suite_path)
    if suite.id != definition.suite_id:
        raise RuntimeError("Configured model evaluation suite ID does not match.")
    context.update_progress(
        0,
        len(suite.cases),
        phase="Preparing evaluation",
        message="Loading fixed synthetic playlist scenarios",
    )

    with SessionLocal() as db:
        resolved = prepare_role_execution_details(db, parameters.role_id)
    if resolved.fingerprint != parameters.role_fingerprint:
        raise ProviderServiceError(
            "role_changed",
            "The model role changed before evaluation started. Run it again.",
            409,
        )

    def execute(request: StructuredModelRequest) -> StructuredModelResult:
        context.check_cancelled()
        return execute_structured_model_request(resolved.execution, request)

    def case_complete(current: int, total: int) -> None:
        context.update_progress(
            current,
            total,
            phase="Evaluating playlist model",
            message=f"Completed {current} of {total} synthetic scenarios",
        )

    result = evaluate_playlist_engine(
        ModelPlaylistPlanner(execute),
        suite,
        on_case_complete=case_complete,
    )
    context.check_cancelled()
    _record_result(context, parameters, result)
    return {
        "schema_version": "assistant-model-quality-result/v1",
        "role_id": parameters.role_id,
        "evaluation_id": parameters.evaluation_id,
        "role_fingerprint": parameters.role_fingerprint,
        "evaluation": result.model_dump(mode="json"),
    }


register_job_handler(
    PLAYLIST_QUALITY_JOB_KIND,
    run_playlist_quality_evaluation,
    restartable=False,
)
