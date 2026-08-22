"""Strict provider contract and synthetic evaluation for manual-tag cleanup."""

from __future__ import annotations

import re
from collections.abc import Callable, Sequence
from pathlib import Path
from typing import Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field, ValidationError, model_validator

from app.assistant.providers.execution import StructuredModelRequest, StructuredModelResult
from app.assistant.schema_diagnostics import safe_validation_diagnostic
from app.assistant.structured_harness import (
    StructuredTaskDefinition,
    build_structured_request,
    numbered_rules,
)
from app.assistant.tag_cleanup import build_tag_cleanup_preview
from app.assistant.tags import DND_STARTER_TAG_GROUPS, TagUsage

MODEL_TAG_CLEANUP_INPUT_CONTRACT: Literal[
    "assistant-model-tag-cleanup-input/v2"
] = "assistant-model-tag-cleanup-input/v2"
MODEL_TAG_CLEANUP_OUTPUT_CONTRACT: Literal[
    "assistant-model-tag-cleanup-output/v1"
] = "assistant-model-tag-cleanup-output/v1"
MODEL_TAG_CLEANUP_EVALUATION_CONTRACT: Literal[
    "assistant-model-tag-cleanup-evaluation/v1"
] = "assistant-model-tag-cleanup-evaluation/v1"
MODEL_TAG_CLEANUP_ENGINE_ID: Literal["model-tag-cleanup/v2"] = (
    "model-tag-cleanup/v2"
)
MAX_MODEL_CLEANUP_TAGS = 500
MAX_MODEL_CLEANUP_SUGGESTIONS = 100
_MAX_MODEL_OUTPUT_TOKENS = 8_000
_SAFE_ERROR_CODE = re.compile(r"^[a-z0-9_]{1,64}$")

MODEL_TAG_CLEANUP_STARTER_TAGS: tuple[str, ...] = tuple(
    tag for group in DND_STARTER_TAG_GROUPS for tag in group.tags
)
_STARTER_TAG_SET = frozenset(MODEL_TAG_CLEANUP_STARTER_TAGS)

BoundedReason = Annotated[str, Field(min_length=1, max_length=512)]
CleanupConfidence = Literal["high", "medium", "low"]


class _StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class ModelTagUsageInput(_StrictModel):
    tag: str = Field(min_length=1, max_length=64)
    track_count: int = Field(ge=0)


class ModelTagCleanupInput(_StrictModel):
    schema_version: Literal["assistant-model-tag-cleanup-input/v2"]
    starter_tags: list[str] = Field(min_length=1, max_length=64)
    candidate_sources: list[ModelTagUsageInput] = Field(
        min_length=1,
        max_length=MAX_MODEL_CLEANUP_TAGS,
    )
    allowed_targets: list[ModelTagUsageInput] = Field(
        min_length=1,
        max_length=MAX_MODEL_CLEANUP_TAGS + 64,
    )
    remaining_suggestion_slots: int = Field(
        ge=0,
        le=MAX_MODEL_CLEANUP_SUGGESTIONS,
    )


class ModelTagCleanupSuggestion(_StrictModel):
    model_config = ConfigDict(extra="forbid", frozen=True, strict=True)

    source: str = Field(min_length=1, max_length=64)
    target: str = Field(min_length=1, max_length=64)
    confidence: CleanupConfidence
    reason: BoundedReason


class ModelTagCleanupOutput(_StrictModel):
    model_config = ConfigDict(extra="forbid", frozen=True, strict=True)

    schema_version: Literal["assistant-model-tag-cleanup-output/v1"]
    suggestions: list[ModelTagCleanupSuggestion] = Field(
        max_length=MAX_MODEL_CLEANUP_SUGGESTIONS
    )


class TagCleanupPair(_StrictModel):
    source: str = Field(min_length=1, max_length=64)
    target: str = Field(min_length=1, max_length=64)


class TagCleanupQualityCase(_StrictModel):
    id: str = Field(min_length=1, max_length=128)
    description: str = Field(min_length=1, max_length=512)
    used_tags: list[ModelTagUsageInput] = Field(
        min_length=1,
        max_length=MAX_MODEL_CLEANUP_TAGS,
    )
    required_pairs: list[TagCleanupPair] = Field(
        default_factory=list,
        max_length=MAX_MODEL_CLEANUP_SUGGESTIONS,
    )
    forbidden_pairs: list[TagCleanupPair] = Field(
        default_factory=list,
        max_length=MAX_MODEL_CLEANUP_SUGGESTIONS,
    )
    maximum_suggestions: int = Field(
        default=MAX_MODEL_CLEANUP_SUGGESTIONS,
        ge=0,
        le=MAX_MODEL_CLEANUP_SUGGESTIONS,
    )

    @model_validator(mode="after")
    def valid_expectations(self) -> TagCleanupQualityCase:
        used_tags = [item.tag for item in self.used_tags]
        if len(set(used_tags)) != len(used_tags):
            raise ValueError("used cleanup tags must be unique")
        required = {(item.source, item.target) for item in self.required_pairs}
        forbidden = {(item.source, item.target) for item in self.forbidden_pairs}
        if required & forbidden:
            raise ValueError("required and forbidden cleanup pairs must be disjoint")
        if len(required) != len(self.required_pairs):
            raise ValueError("required cleanup pairs must be unique")
        if len(forbidden) != len(self.forbidden_pairs):
            raise ValueError("forbidden cleanup pairs must be unique")
        known_sources = set(used_tags)
        if unknown_sources := sorted(
            {source for source, _target in required | forbidden} - known_sources
        ):
            raise ValueError(
                f"cleanup expectations reference unknown sources: {unknown_sources}"
            )
        allowed_targets = known_sources | _STARTER_TAG_SET
        if unknown_targets := sorted(
            {target for _source, target in required | forbidden} - allowed_targets
        ):
            raise ValueError(
                f"cleanup expectations reference unknown targets: {unknown_targets}"
            )
        if len(required) > self.maximum_suggestions:
            raise ValueError(
                "maximum suggestions cannot be smaller than the required pairs"
            )
        return self


class TagCleanupQualitySuite(_StrictModel):
    schema_version: Literal["assistant-model-tag-cleanup-evaluation/v1"]
    id: str = Field(min_length=1, max_length=128)
    cases: list[TagCleanupQualityCase] = Field(min_length=1, max_length=100)

    @model_validator(mode="after")
    def unique_case_ids(self) -> TagCleanupQualitySuite:
        case_ids = [case.id for case in self.cases]
        if len(set(case_ids)) != len(case_ids):
            raise ValueError("case IDs must be unique within a cleanup suite")
        return self


class TagCleanupQualityCaseResult(_StrictModel):
    id: str
    description: str
    passed: bool
    suggestions: list[TagCleanupPair]
    failures: list[str]


class TagCleanupQualityEvaluationResult(_StrictModel):
    schema_version: Literal["assistant-model-tag-cleanup-quality-result/v1"] = (
        "assistant-model-tag-cleanup-quality-result/v1"
    )
    suite_id: str
    engine_id: str = MODEL_TAG_CLEANUP_ENGINE_ID
    passed: bool
    passed_cases: int
    total_cases: int
    cases: list[TagCleanupQualityCaseResult]


class ModelTagCleanupError(RuntimeError):
    def __init__(self, code: str, *, diagnostic: str | None = None) -> None:
        super().__init__(code)
        self.code = code
        self.diagnostic = diagnostic


StructuredCleanupExecutor = Callable[
    [StructuredModelRequest], StructuredModelResult
]


_TAG_CLEANUP_TASK = StructuredTaskDefinition(
    task_id="assistant-tag-cleanup",
    role="A conservative catalog normalizer for operator-owned music tags.",
    objective=(
        "Find only clear unresolved duplicate, synonym, or normalization pairs after "
        "the server has already removed deterministic spelling and plural cases."
    ),
    untrusted_data=("candidate source tags", "allowed target tags"),
    rules=numbered_rules(
        "source must exactly match candidate_sources. target must exactly match allowed_targets. Never invent or rewrite a tag.",
        "The server has already handled unambiguous spelling and plural rules. Focus on clear semantic synonyms that preserve the catalog's useful distinctions.",
        "Prefer an established starter tag or an already-used target. Track counts indicate adoption only; popularity does not make two meanings equivalent.",
        "Never rename a starter tag, return the same source and target, use one source twice, or create source/target chains.",
        "Return no more suggestions than remaining_suggestion_slots. The server reserves the other slots for higher-confidence deterministic results.",
        "Do not merge related but meaningfully distinct settings, scenes, or moods. Return no suggestion when context would be needed to decide safely.",
        "reason must be a short catalog-level explanation and confidence must reflect how unambiguous the normalization is.",
    ),
)


def _safe_execution_error(code: str | None) -> str:
    if code is not None and _SAFE_ERROR_CODE.fullmatch(code):
        return f"model_execution_{code}"
    return "model_execution_failed"


def unresolved_model_cleanup_usage(
    usage: Sequence[TagUsage],
) -> tuple[TagUsage, ...]:
    """Return only non-starter sources not handled by deterministic cleanup."""

    local_sources = {
        item.source for item in build_tag_cleanup_preview(usage).suggestions
    }
    return tuple(
        item
        for item in usage
        if item.tag not in _STARTER_TAG_SET and item.tag not in local_sources
    )


def suggest_model_tag_cleanup(
    usage: Sequence[TagUsage],
    execute: StructuredCleanupExecutor,
) -> tuple[ModelTagCleanupSuggestion, ...]:
    """Return validated, non-chained suggestions without changing manual tags."""

    if not usage:
        return ()
    if len(usage) > MAX_MODEL_CLEANUP_TAGS:
        raise ModelTagCleanupError("catalog_too_large")
    local_preview = build_tag_cleanup_preview(usage)
    all_local_suggestions = tuple(
        ModelTagCleanupSuggestion(
            source=item.source,
            target=item.target,
            confidence="high",
            reason=item.reason,
        )
        for item in local_preview.suggestions
    )
    locally_resolved_sources = {item.source for item in all_local_suggestions}
    local_suggestions = all_local_suggestions[:MAX_MODEL_CLEANUP_SUGGESTIONS]
    remaining_suggestion_slots = (
        MAX_MODEL_CLEANUP_SUGGESTIONS - len(local_suggestions)
    )
    candidate_sources = list(unresolved_model_cleanup_usage(usage))
    if not candidate_sources or remaining_suggestion_slots == 0:
        return local_suggestions

    counts = {item.tag: item.track_count for item in usage}
    allowed_target_tags = sorted(
        (set(counts) - locally_resolved_sources) | _STARTER_TAG_SET
    )
    model_input = ModelTagCleanupInput(
        schema_version=MODEL_TAG_CLEANUP_INPUT_CONTRACT,
        starter_tags=list(MODEL_TAG_CLEANUP_STARTER_TAGS),
        candidate_sources=[
            ModelTagUsageInput(tag=item.tag, track_count=item.track_count)
            for item in candidate_sources
        ],
        allowed_targets=[
            ModelTagUsageInput(tag=tag, track_count=counts.get(tag, 0))
            for tag in allowed_target_tags
        ],
        remaining_suggestion_slots=remaining_suggestion_slots,
    )
    result = execute(
        build_structured_request(
            _TAG_CLEANUP_TASK,
            model_input,
            ModelTagCleanupOutput,
            output_example={
                "schema_version": MODEL_TAG_CLEANUP_OUTPUT_CONTRACT,
                "suggestions": [
                    {
                        "source": "alehouse",
                        "target": "tavern",
                        "confidence": "high",
                        "reason": "Both labels describe the same catalog setting.",
                    }
                ],
            },
            max_output_tokens=_MAX_MODEL_OUTPUT_TOKENS,
        )
    )
    if not result.succeeded or result.payload is None:
        raise ModelTagCleanupError(_safe_execution_error(result.error_code))
    if result.finish_reason in {"length", "max_tokens"}:
        raise ModelTagCleanupError("model_output_incomplete")
    try:
        output = ModelTagCleanupOutput.model_validate(result.payload)
    except ValidationError as exc:
        raise ModelTagCleanupError(
            "model_output_schema_invalid",
            diagnostic=safe_validation_diagnostic(exc, ModelTagCleanupOutput),
        ) from exc
    if len(output.suggestions) > remaining_suggestion_slots:
        raise ModelTagCleanupError("model_output_too_many_suggestions")

    source_tags = {item.tag for item in candidate_sources}
    allowed_targets = set(allowed_target_tags)
    sources = [item.source for item in output.suggestions]
    targets = {item.target for item in output.suggestions}
    if len(sources) != len(set(sources)):
        raise ModelTagCleanupError("model_output_duplicate_source")
    if set(sources) & targets:
        raise ModelTagCleanupError("model_output_chained_rename")
    for item in output.suggestions:
        if item.source not in source_tags:
            raise ModelTagCleanupError("model_output_unknown_source")
        if item.source in _STARTER_TAG_SET:
            raise ModelTagCleanupError("model_output_starter_source")
        if item.target not in allowed_targets:
            raise ModelTagCleanupError("model_output_unknown_target")
        if item.source == item.target:
            raise ModelTagCleanupError("model_output_same_tag")
    model_suggestions = tuple(output.suggestions)
    combined_sources = [
        item.source for item in (*local_suggestions, *model_suggestions)
    ]
    combined_targets = {
        item.target for item in (*local_suggestions, *model_suggestions)
    }
    if len(combined_sources) != len(set(combined_sources)):
        raise ModelTagCleanupError("model_output_duplicate_source")
    if set(combined_sources) & combined_targets:
        raise ModelTagCleanupError("model_output_chained_rename")
    return (*local_suggestions, *model_suggestions)


def load_tag_cleanup_quality_suite(path: Path) -> TagCleanupQualitySuite:
    return TagCleanupQualitySuite.model_validate_json(path.read_text(encoding="utf-8"))


def evaluate_model_tag_cleanup(
    execute: StructuredCleanupExecutor,
    suite: TagCleanupQualitySuite,
    *,
    on_case_complete: Callable[[int, int], None] | None = None,
) -> TagCleanupQualityEvaluationResult:
    results: list[TagCleanupQualityCaseResult] = []
    total = len(suite.cases)
    for index, case in enumerate(suite.cases, start=1):
        failures: list[str] = []
        actual_pairs: set[tuple[str, str]] = set()
        try:
            suggestions = suggest_model_tag_cleanup(
                [
                    TagUsage(tag=item.tag, track_count=item.track_count)
                    for item in case.used_tags
                ],
                execute,
            )
            actual_pairs = {(item.source, item.target) for item in suggestions}
            required = {(item.source, item.target) for item in case.required_pairs}
            forbidden = {(item.source, item.target) for item in case.forbidden_pairs}
            if missing := sorted(required - actual_pairs):
                failures.append(
                    "Missing required cleanup pairs: "
                    + ", ".join(f"{source} -> {target}" for source, target in missing)
                )
            if returned_forbidden := sorted(forbidden & actual_pairs):
                failures.append(
                    "Returned forbidden cleanup pairs: "
                    + ", ".join(
                        f"{source} -> {target}"
                        for source, target in returned_forbidden
                    )
                )
            if len(actual_pairs) > case.maximum_suggestions:
                failures.append(
                    "Returned too many cleanup suggestions: "
                    f"expected at most {case.maximum_suggestions}"
                )
        except ModelTagCleanupError as exc:
            failure = f"Cleanup model error: {exc.code}"
            if exc.diagnostic is not None:
                failure += f" ({exc.diagnostic})"
            failures.append(failure)
        results.append(
            TagCleanupQualityCaseResult(
                id=case.id,
                description=case.description,
                passed=not failures,
                suggestions=[
                    TagCleanupPair(source=source, target=target)
                    for source, target in sorted(actual_pairs)
                ],
                failures=failures,
            )
        )
        if on_case_complete is not None:
            on_case_complete(index, total)
    passed_cases = sum(item.passed for item in results)
    return TagCleanupQualityEvaluationResult(
        suite_id=suite.id,
        passed=passed_cases == total,
        passed_cases=passed_cases,
        total_cases=total,
        cases=results,
    )
