"""Strict provider contract and synthetic evaluation for manual-tag cleanup."""

import re
from collections.abc import Callable, Sequence
from copy import deepcopy
from functools import partial
from pathlib import Path
from typing import Annotated, Literal

from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    ValidationError,
    field_validator,
    model_validator,
)

from app.assistant.providers.execution import StructuredModelRequest, StructuredModelResult
from app.assistant.schema_diagnostics import safe_validation_diagnostic
from app.assistant.structured_harness import (
    StructuredTaskDefinition,
    build_structured_request,
    numbered_rules,
)
from app.assistant.tag_cleanup import build_tag_cleanup_preview
from app.assistant.tag_vocabulary import (
    TagVocabularySnapshot,
    default_tag_vocabulary_snapshot,
)
from app.assistant.tags import TagUsage

MODEL_TAG_CLEANUP_INPUT_CONTRACT: Literal[
    "assistant-model-tag-cleanup-input/v3"
] = "assistant-model-tag-cleanup-input/v3"
MODEL_TAG_CLEANUP_OUTPUT_CONTRACT: Literal[
    "assistant-model-tag-cleanup-output/v2"
] = "assistant-model-tag-cleanup-output/v2"
MODEL_TAG_CLEANUP_EVALUATION_CONTRACT: Literal[
    "assistant-model-tag-cleanup-evaluation/v2"
] = "assistant-model-tag-cleanup-evaluation/v2"
MODEL_TAG_CLEANUP_ENGINE_ID: Literal["model-tag-cleanup/v3"] = (
    "model-tag-cleanup/v3"
)
MAX_MODEL_CLEANUP_TAGS = 500
MAX_MODEL_CLEANUP_SUGGESTIONS = 100
MODEL_TAG_CLEANUP_BATCH_SIZE = 20
_MAX_MODEL_OUTPUT_TOKENS = 8_000
_SAFE_ERROR_CODE = re.compile(r"^[a-z0-9_]{1,64}$")
_DEFAULT_TAG_SET = default_tag_vocabulary_snapshot().names

BoundedReason = Annotated[str, Field(min_length=1, max_length=512)]
CleanupConfidence = Literal["high", "medium", "low"]


class _StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class ModelTagUsageInput(_StrictModel):
    tag: str = Field(min_length=1, max_length=64)
    track_count: int = Field(ge=0)


class ModelCanonicalTagInput(_StrictModel):
    tag_id: str = Field(min_length=2, max_length=64)
    name: str = Field(min_length=1, max_length=64)
    group: str = Field(min_length=1, max_length=64)
    description: str = Field(min_length=2, max_length=300)


class ModelTagCleanupSourceInput(_StrictModel):
    source_id: str = Field(pattern=r"^source-[0-9]{3}$")
    tag: str = Field(min_length=1, max_length=64)
    track_count: int = Field(ge=0)


class ModelTagCleanupInput(_StrictModel):
    schema_version: Literal["assistant-model-tag-cleanup-input/v3"]
    canonical_tags: list[ModelCanonicalTagInput] = Field(
        min_length=1,
        max_length=MAX_MODEL_CLEANUP_TAGS,
    )
    candidate_sources: list[ModelTagCleanupSourceInput] = Field(
        min_length=1,
        max_length=MAX_MODEL_CLEANUP_TAGS,
    )
    remaining_suggestion_slots: int = Field(
        ge=0,
        le=MAX_MODEL_CLEANUP_SUGGESTIONS,
    )


class ModelTagCleanupDecision(_StrictModel):
    model_config = ConfigDict(extra="forbid", frozen=True, strict=True)

    source_id: str = Field(pattern=r"^source-[0-9]{3}$")
    target_tag_id: str | None = Field(default=None, min_length=2, max_length=64)
    confidence: CleanupConfidence
    reason: BoundedReason

    @field_validator("reason", mode="before")
    @classmethod
    def bound_incidental_reason(cls, value: object) -> object:
        if isinstance(value, str) and len(value) > 512:
            return f"{value[:509].rstrip()}..."
        return value


class ModelTagCleanupOutput(_StrictModel):
    model_config = ConfigDict(extra="forbid", frozen=True, strict=True)

    schema_version: Literal["assistant-model-tag-cleanup-output/v2"]
    decisions: list[ModelTagCleanupDecision] = Field(
        min_length=1,
        max_length=MAX_MODEL_CLEANUP_TAGS,
    )


class ModelTagCleanupSuggestion(_StrictModel):
    source: str = Field(min_length=1, max_length=64)
    target: str = Field(min_length=1, max_length=64)
    confidence: CleanupConfidence
    reason: BoundedReason


class TagCleanupPair(_StrictModel):
    source: str = Field(min_length=1, max_length=64)
    target: str = Field(min_length=1, max_length=64)


class GeneratedCleanupTags(_StrictModel):
    count: int = Field(ge=1, le=MODEL_TAG_CLEANUP_BATCH_SIZE)
    prefix: str = Field(min_length=1, max_length=48)
    track_count: int = Field(default=1, ge=0)

    def materialize(self) -> list[dict[str, object]]:
        return [
            {
                "tag": f"{self.prefix} {index + 1:02d}",
                "track_count": self.track_count,
            }
            for index in range(self.count)
        ]


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

    @model_validator(mode="before")
    @classmethod
    def materialize_generated_tags(cls, value: object) -> object:
        if not isinstance(value, dict) or "generated_used_tags" not in value:
            return value
        payload = dict(value)
        generated = GeneratedCleanupTags.model_validate(
            payload.pop("generated_used_tags")
        )
        used_tags = list(payload.get("used_tags", []))
        used_tags.extend(generated.materialize())
        payload["used_tags"] = used_tags
        return payload

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
        allowed_targets = known_sources | _DEFAULT_TAG_SET
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
    schema_version: Literal["assistant-model-tag-cleanup-evaluation/v2"]
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
        "Classify every unresolved source as exactly one canonical tag ID or no safe "
        "match after deterministic aliases, spelling, and plurals are removed."
    ),
    untrusted_data=("candidate source tags", "canonical tag names and descriptions"),
    rules=numbered_rules(
        "Return every candidate source_id exactly once and no other source IDs. Preserve the input order.",
        "target_tag_id must be an exact ID from canonical_tags or null. Never return a tag name, source text, or invented ID as the target.",
        "The server already handled declared aliases and unambiguous spelling and plurals. Choose a target only for a clear semantic synonym that preserves useful distinctions.",
        "The target definition must cover the complete source meaning, not merely one word. A multiword source may map when one definition covers all meaningful parts; otherwise use null instead of discarding a mood, period, setting, scene, or other modifier.",
        "Do not merge related but meaningfully distinct settings, scenes, or moods. Use null whenever track context would be needed to decide safely.",
        "Track counts indicate adoption only; popularity does not make two meanings equivalent. Definitions are authoritative when labels could overlap.",
        "Do not stop after the first match. Evaluate each source independently even when earlier sources map to the same target.",
        "At most remaining_suggestion_slots decisions may use a non-null target_tag_id. All other sources must still receive an explicit null decision.",
        "reason must be a short catalog-level explanation and confidence must reflect how unambiguous the decision is.",
    ),
)


def _safe_execution_error(code: str | None) -> str:
    if code is not None and _SAFE_ERROR_CODE.fullmatch(code):
        return f"model_execution_{code}"
    return "model_execution_failed"


def unresolved_model_cleanup_usage(
    usage: Sequence[TagUsage],
    vocabulary: TagVocabularySnapshot | None = None,
) -> tuple[TagUsage, ...]:
    """Return only non-canonical sources not handled by deterministic cleanup."""

    vocabulary = vocabulary or default_tag_vocabulary_snapshot()
    local_sources = {
        item.source
        for item in build_tag_cleanup_preview(usage, vocabulary).suggestions
    }
    return tuple(
        item
        for item in usage
        if item.tag not in vocabulary.names and item.tag not in local_sources
    )


def _closed_cleanup_schema(
    schema: dict[str, object],
    *,
    source_ids: list[str],
    tag_ids: list[str],
) -> dict[str, object]:
    closed = deepcopy(schema)
    definitions = closed.get("$defs")
    if not isinstance(definitions, dict):
        raise RuntimeError("cleanup output schema is missing definitions")
    decision = definitions.get("ModelTagCleanupDecision")
    if not isinstance(decision, dict):
        raise RuntimeError("cleanup decision schema is missing")
    properties = decision.get("properties")
    if not isinstance(properties, dict):
        raise RuntimeError("cleanup decision properties are missing")
    source_schema = properties.get("source_id")
    if not isinstance(source_schema, dict):
        raise RuntimeError("cleanup source schema is missing")
    source_schema["enum"] = source_ids
    target_schema = properties.get("target_tag_id")
    if not isinstance(target_schema, dict):
        raise RuntimeError("cleanup target schema is missing")
    choices = target_schema.get("anyOf")
    if not isinstance(choices, list):
        raise RuntimeError("cleanup target choices are missing")
    string_choice = next(
        (choice for choice in choices if isinstance(choice, dict) and choice.get("type") == "string"),
        None,
    )
    if string_choice is None:
        raise RuntimeError("cleanup target string choice is missing")
    string_choice["enum"] = tag_ids
    output_properties = closed.get("properties")
    if not isinstance(output_properties, dict):
        raise RuntimeError("cleanup output properties are missing")
    decisions_schema = output_properties.get("decisions")
    if not isinstance(decisions_schema, dict):
        raise RuntimeError("cleanup decisions schema is missing")
    decisions_schema["minItems"] = len(source_ids)
    decisions_schema["maxItems"] = len(source_ids)
    return closed


def suggest_model_tag_cleanup(
    usage: Sequence[TagUsage],
    execute: StructuredCleanupExecutor,
    vocabulary: TagVocabularySnapshot | None = None,
) -> tuple[ModelTagCleanupSuggestion, ...]:
    """Return validated canonical mappings without changing manual tags."""

    if not usage:
        return ()
    if len(usage) > MAX_MODEL_CLEANUP_TAGS:
        raise ModelTagCleanupError("catalog_too_large")
    vocabulary = vocabulary or default_tag_vocabulary_snapshot()
    local_preview = build_tag_cleanup_preview(usage, vocabulary)
    all_local_suggestions = tuple(
        ModelTagCleanupSuggestion(
            source=item.source,
            target=item.target,
            confidence="high",
            reason=item.reason,
        )
        for item in local_preview.suggestions
    )
    local_suggestions = all_local_suggestions[:MAX_MODEL_CLEANUP_SUGGESTIONS]
    remaining_suggestion_slots = (
        MAX_MODEL_CLEANUP_SUGGESTIONS - len(local_suggestions)
    )
    candidate_sources = list(unresolved_model_cleanup_usage(usage, vocabulary))
    if not candidate_sources or remaining_suggestion_slots == 0:
        return local_suggestions

    groups = vocabulary.group_by_tag_id
    canonical_inputs = [
        ModelCanonicalTagInput(
            tag_id=tag.id,
            name=tag.name,
            group=groups[tag.id].label,
            description=tag.description,
        )
        for tag in vocabulary.entries
    ]
    by_id = vocabulary.by_id
    model_suggestions: list[ModelTagCleanupSuggestion] = []
    indexed_sources = [
        (f"source-{index:03d}", item)
        for index, item in enumerate(candidate_sources, start=1)
    ]
    for offset in range(0, len(indexed_sources), MODEL_TAG_CLEANUP_BATCH_SIZE):
        if len(model_suggestions) >= remaining_suggestion_slots:
            break
        batch = indexed_sources[offset : offset + MODEL_TAG_CLEANUP_BATCH_SIZE]
        source_ids = [source_id for source_id, _item in batch]
        source_by_id = dict(batch)
        vocabulary_tag_ids = [tag.id for tag in vocabulary.entries]
        model_input = ModelTagCleanupInput(
            schema_version=MODEL_TAG_CLEANUP_INPUT_CONTRACT,
            canonical_tags=canonical_inputs,
            candidate_sources=[
                ModelTagCleanupSourceInput(
                    source_id=source_id,
                    tag=item.tag,
                    track_count=item.track_count,
                )
                for source_id, item in batch
            ],
            remaining_suggestion_slots=(
                remaining_suggestion_slots - len(model_suggestions)
            ),
        )
        result = execute(
            build_structured_request(
                _TAG_CLEANUP_TASK,
                model_input,
                ModelTagCleanupOutput,
                output_example={
                    "schema_version": MODEL_TAG_CLEANUP_OUTPUT_CONTRACT,
                    "decisions": [
                        {
                            "source_id": source_id,
                            "target_tag_id": None,
                            "confidence": "low",
                            "reason": "No safe canonical match is established.",
                        }
                        for source_id in source_ids
                    ],
                },
                max_output_tokens=_MAX_MODEL_OUTPUT_TOKENS,
                schema_transform=partial(
                    _closed_cleanup_schema,
                    source_ids=source_ids,
                    tag_ids=vocabulary_tag_ids,
                ),
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
        returned_source_ids = [item.source_id for item in output.decisions]
        if returned_source_ids != source_ids:
            raise ModelTagCleanupError("model_output_source_set_mismatch")
        for decision in output.decisions:
            if decision.target_tag_id is None:
                continue
            target = by_id.get(decision.target_tag_id)
            if target is None:
                raise ModelTagCleanupError("model_output_unknown_target")
            source = source_by_id[decision.source_id]
            model_suggestions.append(
                ModelTagCleanupSuggestion(
                    source=source.tag,
                    target=target.name,
                    confidence=decision.confidence,
                    reason=decision.reason,
                )
            )
            if len(model_suggestions) > remaining_suggestion_slots:
                raise ModelTagCleanupError("model_output_too_many_suggestions")
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
