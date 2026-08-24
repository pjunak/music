"""Durable, synthetic quality gates for optional Assistant model roles."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any, Literal, cast

from pydantic import BaseModel, ConfigDict, Field, ValidationError, model_validator
from sqlalchemy.orm import Session

from app.assistant.evaluation import (
    evaluate_playlist_engine,
    load_evaluation_suite,
)
from app.assistant.model_eq import (
    EQ_DRAFT_ENGINE_ID,
    EqQualityEvaluationResult,
    evaluate_eq_model,
    load_eq_quality_suite,
)
from app.assistant.model_playlist import ModelPlaylistPlanner
from app.assistant.model_tag_cleanup import (
    MODEL_TAG_CLEANUP_ENGINE_ID,
    TagCleanupQualityEvaluationResult,
    evaluate_model_tag_cleanup,
    load_tag_cleanup_quality_suite,
)
from app.assistant.model_tagger import (
    MODEL_TAG_ANALYZER_ID,
    TagQualityEvaluationResult,
    TagQualitySuite,
    evaluate_music_tagger,
    load_tag_quality_suite,
    summarize_music_tagger_quality,
    tag_quality_attempts,
)
from app.assistant.providers.definitions import MODEL_ROLE_BY_ID
from app.assistant.providers.execution import (
    StructuredModelRequest,
    StructuredModelResult,
    execute_structured_model_request,
)
from app.assistant.providers.schemas import (
    ModelQualityEvaluationOut,
    ModelQualityJobResult,
)
from app.assistant.providers.service import (
    ProviderServiceError,
    ResolvedRoleExecution,
    current_role_runtime_fingerprint,
    prepare_role_execution_details,
)
from app.assistant.providers.usage import ProviderUsageAccumulator
from app.core.db import SessionLocal
from app.jobs.registry import JobExecutionContext, register_job_handler
from app.models.assistant_model_evaluation import AssistantModelEvaluation
from app.models.background_job import BackgroundJob
from app.models.base import utcnow

PLAYLIST_QUALITY_EVALUATION_ID: Literal["playlist-quality-v1"] = (
    "playlist-quality-v1"
)
PLAYLIST_QUALITY_JOB_KIND = "assistant.model-evaluation.playlist-quality-v1"
TAGGING_QUALITY_EVALUATION_ID: Literal["music-tagging-quality-v1"] = (
    "music-tagging-quality-v1"
)
TAGGING_QUALITY_JOB_KIND = "assistant.model-evaluation.music-tagging-quality-v1"
TAG_CLEANUP_QUALITY_EVALUATION_ID: Literal["tag-cleanup-quality-v1"] = (
    "tag-cleanup-quality-v1"
)
TAG_CLEANUP_QUALITY_JOB_KIND = "assistant.model-evaluation.tag-cleanup-quality-v1"
EQ_QUALITY_EVALUATION_ID: Literal["eq-quality-v1"] = "eq-quality-v1"
EQ_QUALITY_JOB_KIND = "assistant.model-evaluation.eq-quality-v1"
_EVALUATION_SUITE_DIR = Path(__file__).resolve().with_name("evaluation_suites")
_PLAYLIST_SUITE_PATH = _EVALUATION_SUITE_DIR / "playlist-local-v1.json"
_TAGGING_SUITE_PATH = _EVALUATION_SUITE_DIR / "music-tagging-v1.json"
_TAG_CLEANUP_SUITE_PATH = _EVALUATION_SUITE_DIR / "tag-cleanup-v1.json"
_EQ_SUITE_PATH = _EVALUATION_SUITE_DIR / "eq-assistant-v1.json"


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
    suite_id="local-dnd-playlist-baseline-v4",
    suite_path=_PLAYLIST_SUITE_PATH,
    job_kind=PLAYLIST_QUALITY_JOB_KIND,
)

TAGGING_QUALITY_EVALUATION = ModelEvaluationDefinition(
    id=TAGGING_QUALITY_EVALUATION_ID,
    role_id="music_tagger",
    label="Mood tagging quality",
    description=(
        "Runs fixed synthetic metadata and signal-evidence cases against the "
        "server-owned tag vocabulary. No songs or live library data are sent."
    ),
    suite_id="controlled-vocabulary-tagging-baseline-v13",
    suite_path=_TAGGING_SUITE_PATH,
    job_kind=TAGGING_QUALITY_JOB_KIND,
)

TAG_CLEANUP_QUALITY_EVALUATION = ModelEvaluationDefinition(
    id=TAG_CLEANUP_QUALITY_EVALUATION_ID,
    role_id="tag_cleanup",
    label="Mood-tag cleanup quality",
    description=(
        "Runs fixed synthetic canonical-ID and no-match tag-cleanup cases through "
        "this model. No songs or live library data are sent."
    ),
    suite_id="controlled-vocabulary-cleanup-baseline-v3",
    suite_path=_TAG_CLEANUP_SUITE_PATH,
    job_kind=TAG_CLEANUP_QUALITY_JOB_KIND,
)
EQ_QUALITY_EVALUATION = ModelEvaluationDefinition(
    id=EQ_QUALITY_EVALUATION_ID,
    role_id="eq_assistant",
    label="EQ draft quality",
    description=(
        "Runs fixed synthetic sound goals through this model and checks bounded, "
        "conservative graphic-EQ behavior. No songs or live presets are sent."
    ),
    suite_id="graphic-eq-safety-baseline-v2",
    suite_path=_EQ_SUITE_PATH,
    job_kind=EQ_QUALITY_JOB_KIND,
)

_EVALUATIONS_BY_ROLE: dict[str, tuple[ModelEvaluationDefinition, ...]] = {
    PLAYLIST_QUALITY_EVALUATION.role_id: (PLAYLIST_QUALITY_EVALUATION,),
    TAGGING_QUALITY_EVALUATION.role_id: (TAGGING_QUALITY_EVALUATION,),
    TAG_CLEANUP_QUALITY_EVALUATION.role_id: (TAG_CLEANUP_QUALITY_EVALUATION,),
    EQ_QUALITY_EVALUATION.role_id: (EQ_QUALITY_EVALUATION,),
}


def bundled_evaluation_suite_paths() -> tuple[Path, ...]:
    """Return every read-only suite that must ship with the backend package."""

    return tuple(
        definition.suite_path
        for definitions in _EVALUATIONS_BY_ROLE.values()
        for definition in definitions
    )


class _EvaluationJobParameters(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)

    role_id: str = Field(min_length=1, max_length=64)
    evaluation_id: str = Field(min_length=1, max_length=128)
    role_fingerprint: str = Field(pattern=r"^[a-f0-9]{64}$")
    case_ids: tuple[str, ...] = Field(default=(), max_length=100)
    baseline_job_id: str | None = Field(default=None, min_length=32, max_length=32)

    @model_validator(mode="after")
    def valid_retest(self) -> _EvaluationJobParameters:
        if bool(self.case_ids) != (self.baseline_job_id is not None):
            raise ValueError("case IDs and a baseline job must be supplied together")
        if len(set(self.case_ids)) != len(self.case_ids):
            raise ValueError("retest case IDs must be unique")
        return self


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
        if (
            current_fingerprint != row.role_fingerprint
            or row.suite_id != definition.suite_id
        ):
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
        suite_id=definition.suite_id,
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
        role_id=definition.role_id,
        evaluation_id=definition.id,
        role_fingerprint=resolved.fingerprint,
    )
    return definition, parameters.model_dump(mode="json")


def failed_scenario_job_parameters(
    db: Session,
    role_id: str,
    evaluation_id: str,
) -> tuple[ModelEvaluationDefinition, dict[str, Any]]:
    """Build a bounded retest from the current complete mood-quality result."""

    definition = require_evaluation_definition(role_id, evaluation_id)
    if definition.id != TAGGING_QUALITY_EVALUATION_ID:
        raise ProviderServiceError(
            "evaluation_partial_retest_unavailable",
            "Failed-scenario retesting is currently available for mood tagging.",
            409,
        )
    resolved = prepare_role_execution_details(db, role_id)
    row = db.get(AssistantModelEvaluation, (role_id, evaluation_id))
    if (
        row is None
        or row.suite_id != definition.suite_id
        or row.role_fingerprint != resolved.fingerprint
        or row.job_id is None
    ):
        raise ProviderServiceError(
            "evaluation_retest_baseline_unavailable",
            "Run the complete current quality suite before rechecking failures.",
            409,
        )
    baseline = db.get(BackgroundJob, row.job_id)
    if baseline is None or baseline.status != "succeeded":
        raise ProviderServiceError(
            "evaluation_retest_baseline_unavailable",
            "The saved complete quality result is not available.",
            409,
        )
    try:
        result = ModelQualityJobResult.model_validate_json(
            baseline.result_json or "null"
        )
        evaluation = TagQualityEvaluationResult.model_validate(result.evaluation)
        case_ids = tuple(
            item.id for item in evaluation.cases if item.passed is False
        )
    except (ValidationError, ValueError) as exc:
        raise ProviderServiceError(
            "evaluation_retest_baseline_unavailable",
            "The saved result cannot be used as a failed-scenario baseline.",
            409,
        ) from exc
    if (
        result.role_id != role_id
        or result.evaluation_id != evaluation_id
        or result.role_fingerprint != resolved.fingerprint
        or evaluation.suite_id != definition.suite_id
    ):
        raise ProviderServiceError(
            "evaluation_retest_baseline_stale",
            "The saved quality result belongs to different model settings.",
            409,
        )
    if not case_ids:
        raise ProviderServiceError(
            "evaluation_retest_not_needed",
            "The current quality result has no failed scenarios to recheck.",
            409,
        )
    parameters = _EvaluationJobParameters(
        role_id=definition.role_id,
        evaluation_id=definition.id,
        role_fingerprint=resolved.fingerprint,
        case_ids=case_ids,
        baseline_job_id=row.job_id,
    )
    return definition, parameters.model_dump(mode="json")


def _tagging_retest_suite(
    suite: TagQualitySuite,
    parameters: _EvaluationJobParameters,
) -> TagQualitySuite:
    if not parameters.case_ids:
        return suite
    requested = set(parameters.case_ids)
    selected = [case for case in suite.cases if case.id in requested]
    if {case.id for case in selected} != requested:
        raise ProviderServiceError(
            "evaluation_retest_baseline_stale",
            "The saved failed scenarios do not match the current quality suite.",
            409,
        )
    return suite.model_copy(update={"cases": selected})


def _merge_tagging_retest(
    suite: TagQualitySuite,
    parameters: _EvaluationJobParameters,
    retest: TagQualityEvaluationResult,
) -> TagQualityEvaluationResult:
    if parameters.baseline_job_id is None:
        return retest
    with SessionLocal() as db:
        baseline = db.get(BackgroundJob, parameters.baseline_job_id)
        if baseline is None or baseline.status != "succeeded":
            raise ProviderServiceError(
                "evaluation_retest_baseline_unavailable",
                "The saved quality result is no longer available.",
                409,
            )
        try:
            result = ModelQualityJobResult.model_validate_json(
                baseline.result_json or "null"
            )
            previous = TagQualityEvaluationResult.model_validate(result.evaluation)
        except (ValidationError, ValueError) as exc:
            raise ProviderServiceError(
                "evaluation_retest_baseline_unavailable",
                "The saved quality result cannot be merged safely.",
                409,
            ) from exc
    if (
        result.role_id != parameters.role_id
        or result.evaluation_id != parameters.evaluation_id
        or result.role_fingerprint != parameters.role_fingerprint
        or previous.suite_id != suite.id
    ):
        raise ProviderServiceError(
            "evaluation_retest_baseline_stale",
            "The saved quality result belongs to different model settings.",
            409,
        )
    replacements = {item.id: item for item in retest.cases}
    merged = [replacements.get(item.id, item) for item in previous.cases]
    if len(merged) != len(suite.cases):
        raise ProviderServiceError(
            "evaluation_retest_baseline_stale",
            "The saved quality result does not cover the current suite.",
            409,
        )
    return summarize_music_tagger_quality(suite, merged)


def prepare_quality_gated_role_execution(
    db: Session,
    role_id: str,
    evaluation_id: str,
) -> ResolvedRoleExecution:
    """Resolve a role only when its current runtime passed the required suite."""

    definition = require_evaluation_definition(role_id, evaluation_id)
    resolved = prepare_role_execution_details(db, role_id)
    row = db.get(AssistantModelEvaluation, (role_id, evaluation_id))
    if (
        row is None
        or row.status != "passed"
        or row.suite_id != definition.suite_id
        or row.role_fingerprint != resolved.fingerprint
    ):
        raise ProviderServiceError(
            "model_quality_not_passed",
            "Run and pass the current model quality check before using live library data.",
            409,
        )
    return resolved


def _record_result(
    context: JobExecutionContext,
    parameters: _EvaluationJobParameters,
    *,
    passed: bool,
    suite_id: str,
    engine_id: str,
    passed_cases: int,
    total_cases: int,
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
                suite_id=suite_id,
                engine_id=engine_id,
                passed_cases=0,
                total_cases=0,
                job_id=context.job_id,
            )
            db.add(row)
        row.role_fingerprint = parameters.role_fingerprint
        row.status = "passed" if passed else "failed"
        row.suite_id = suite_id
        row.engine_id = engine_id
        row.passed_cases = passed_cases
        row.total_cases = total_cases
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
    usage = ProviderUsageAccumulator()

    def execute(request: StructuredModelRequest) -> StructuredModelResult:
        context.check_cancelled()
        result = usage.record(
            execute_structured_model_request(resolved.execution, request)
        )
        context.checkpoint_result(usage.checkpoint())
        return result

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
    _record_result(
        context,
        parameters,
        passed=result.passed,
        suite_id=result.suite_id,
        engine_id=result.engine_id,
        passed_cases=result.summary.passed_cases,
        total_cases=result.summary.cases,
    )
    return ModelQualityJobResult(
        schema_version="assistant-model-quality-result/v1",
        role_id=parameters.role_id,
        evaluation_id=parameters.evaluation_id,
        role_fingerprint=parameters.role_fingerprint,
        evaluation=result.model_dump(mode="json"),
        usage=usage.summary(),
    ).model_dump(mode="json")


def run_tagging_quality_evaluation(
    context: JobExecutionContext,
    raw_parameters: dict[str, Any],
) -> dict[str, Any]:
    parameters = _EvaluationJobParameters.model_validate(raw_parameters)
    definition = require_evaluation_definition(
        parameters.role_id,
        parameters.evaluation_id,
    )
    if definition.id != TAGGING_QUALITY_EVALUATION_ID:
        raise ValueError("Tagging quality handler received the wrong evaluation.")
    suite = load_tag_quality_suite(definition.suite_path)
    if suite.id != definition.suite_id:
        raise RuntimeError("Configured model tagging suite ID does not match.")
    execution_suite = _tagging_retest_suite(suite, parameters)
    scenario_count = len(execution_suite.cases)
    safety_rerun_count = sum(
        case.gate == "safety" for case in execution_suite.cases
    )
    scored_attempt_count = tag_quality_attempts(execution_suite)
    context.update_progress(
        0,
        scored_attempt_count,
        phase="Preparing evaluation",
        message=(
            f"Loading {scenario_count} "
            f"{'failed' if parameters.case_ids else 'fixed'} tagging "
            f"{'scenario' if scenario_count == 1 else 'scenarios'}"
        )
        + (
            f"; {safety_rerun_count} safety stability "
            f"{'rerun' if safety_rerun_count == 1 else 'reruns'} make "
            f"{scored_attempt_count} scored attempts"
            if safety_rerun_count
            else ""
        ),
    )

    with SessionLocal() as db:
        resolved = prepare_role_execution_details(db, parameters.role_id)
    if resolved.fingerprint != parameters.role_fingerprint:
        raise ProviderServiceError(
            "role_changed",
            "The model role changed before evaluation started. Run it again.",
            409,
        )
    usage = ProviderUsageAccumulator()

    def execute(request: StructuredModelRequest) -> StructuredModelResult:
        context.check_cancelled()
        result = usage.record(
            execute_structured_model_request(resolved.execution, request)
        )
        context.checkpoint_result(usage.checkpoint())
        return result

    def case_complete(current: int, total: int) -> None:
        context.update_progress(
            current,
            total,
            phase="Evaluating tagging model",
            message=(
                f"Completed {current} of {total} scored attempts across "
                f"{scenario_count} scored "
                f"{'scenario' if scenario_count == 1 else 'scenarios'}"
            ),
        )

    result: TagQualityEvaluationResult = evaluate_music_tagger(
        execute,
        execution_suite,
        on_case_complete=case_complete,
    )
    result = _merge_tagging_retest(suite, parameters, result)
    context.check_cancelled()
    _record_result(
        context,
        parameters,
        passed=result.passed,
        suite_id=result.suite_id,
        engine_id=MODEL_TAG_ANALYZER_ID,
        passed_cases=result.passed_cases,
        total_cases=result.total_cases,
    )
    return ModelQualityJobResult(
        schema_version="assistant-model-quality-result/v1",
        role_id=parameters.role_id,
        evaluation_id=parameters.evaluation_id,
        role_fingerprint=parameters.role_fingerprint,
        evaluation=result.model_dump(mode="json"),
        usage=usage.summary(),
    ).model_dump(mode="json")


def run_tag_cleanup_quality_evaluation(
    context: JobExecutionContext,
    raw_parameters: dict[str, Any],
) -> dict[str, Any]:
    parameters = _EvaluationJobParameters.model_validate(raw_parameters)
    definition = require_evaluation_definition(
        parameters.role_id,
        parameters.evaluation_id,
    )
    if definition.id != TAG_CLEANUP_QUALITY_EVALUATION_ID:
        raise ValueError("Tag cleanup quality handler received the wrong evaluation.")
    suite = load_tag_cleanup_quality_suite(definition.suite_path)
    if suite.id != definition.suite_id:
        raise RuntimeError("Configured model tag-cleanup suite ID does not match.")
    context.update_progress(
        0,
        len(suite.cases),
        phase="Preparing evaluation",
        message="Loading fixed synthetic tag-cleanup cases",
    )

    with SessionLocal() as db:
        resolved = prepare_role_execution_details(db, parameters.role_id)
    if resolved.fingerprint != parameters.role_fingerprint:
        raise ProviderServiceError(
            "role_changed",
            "The model role changed before evaluation started. Run it again.",
            409,
        )
    usage = ProviderUsageAccumulator()

    def execute(request: StructuredModelRequest) -> StructuredModelResult:
        context.check_cancelled()
        result = usage.record(
            execute_structured_model_request(resolved.execution, request)
        )
        context.checkpoint_result(usage.checkpoint())
        return result

    def case_complete(current: int, total: int) -> None:
        context.update_progress(
            current,
            total,
            phase="Evaluating tag cleanup model",
            message=f"Completed {current} of {total} synthetic cases",
        )

    result: TagCleanupQualityEvaluationResult = evaluate_model_tag_cleanup(
        execute,
        suite,
        on_case_complete=case_complete,
    )
    context.check_cancelled()
    _record_result(
        context,
        parameters,
        passed=result.passed,
        suite_id=result.suite_id,
        engine_id=MODEL_TAG_CLEANUP_ENGINE_ID,
        passed_cases=result.passed_cases,
        total_cases=result.total_cases,
    )
    return ModelQualityJobResult(
        schema_version="assistant-model-quality-result/v1",
        role_id=parameters.role_id,
        evaluation_id=parameters.evaluation_id,
        role_fingerprint=parameters.role_fingerprint,
        evaluation=result.model_dump(mode="json"),
        usage=usage.summary(),
    ).model_dump(mode="json")


def run_eq_quality_evaluation(
    context: JobExecutionContext,
    raw_parameters: dict[str, Any],
) -> dict[str, Any]:
    parameters = _EvaluationJobParameters.model_validate(raw_parameters)
    definition = require_evaluation_definition(
        parameters.role_id,
        parameters.evaluation_id,
    )
    if definition.id != EQ_QUALITY_EVALUATION_ID:
        raise ValueError("EQ quality handler received the wrong evaluation.")
    suite = load_eq_quality_suite(definition.suite_path)
    if suite.id != definition.suite_id:
        raise RuntimeError("Configured EQ quality suite ID does not match.")
    context.update_progress(
        0,
        len(suite.cases),
        phase="Preparing evaluation",
        message="Loading fixed synthetic EQ goals",
    )
    with SessionLocal() as db:
        resolved = prepare_role_execution_details(db, parameters.role_id)
    if resolved.fingerprint != parameters.role_fingerprint:
        raise ProviderServiceError(
            "role_changed",
            "The model role changed before evaluation started. Run it again.",
            409,
        )
    usage = ProviderUsageAccumulator()

    def execute(request: StructuredModelRequest) -> StructuredModelResult:
        context.check_cancelled()
        result = usage.record(
            execute_structured_model_request(resolved.execution, request)
        )
        context.checkpoint_result(usage.checkpoint())
        return result

    def case_complete(current: int, total: int) -> None:
        context.update_progress(
            current,
            total,
            phase="Evaluating EQ model",
            message=f"Completed {current} of {total} synthetic goals",
        )

    result: EqQualityEvaluationResult = evaluate_eq_model(
        execute,
        suite,
        on_case_complete=case_complete,
    )
    context.check_cancelled()
    _record_result(
        context,
        parameters,
        passed=result.passed,
        suite_id=result.suite_id,
        engine_id=EQ_DRAFT_ENGINE_ID,
        passed_cases=result.passed_cases,
        total_cases=result.total_cases,
    )
    return ModelQualityJobResult(
        schema_version="assistant-model-quality-result/v1",
        role_id=parameters.role_id,
        evaluation_id=parameters.evaluation_id,
        role_fingerprint=parameters.role_fingerprint,
        evaluation=result.model_dump(mode="json"),
        usage=usage.summary(),
    ).model_dump(mode="json")


register_job_handler(
    PLAYLIST_QUALITY_JOB_KIND,
    run_playlist_quality_evaluation,
    restartable=False,
)
register_job_handler(
    TAGGING_QUALITY_JOB_KIND,
    run_tagging_quality_evaluation,
    restartable=False,
)
register_job_handler(
    TAG_CLEANUP_QUALITY_JOB_KIND,
    run_tag_cleanup_quality_evaluation,
    restartable=False,
)
register_job_handler(
    EQ_QUALITY_JOB_KIND,
    run_eq_quality_evaluation,
    restartable=False,
)
