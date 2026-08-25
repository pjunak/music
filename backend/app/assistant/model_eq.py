"""Strict provider contract for review-only graphic-EQ preset drafts."""

import re
from collections.abc import Callable
from dataclasses import dataclass
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

EQ_DRAFT_INPUT_CONTRACT: Literal["assistant-eq-draft-input/v2"] = (
    "assistant-eq-draft-input/v2"
)
EQ_DRAFT_OUTPUT_CONTRACT: Literal["assistant-eq-draft-output/v1"] = (
    "assistant-eq-draft-output/v1"
)
EQ_DRAFT_ENGINE_ID: Literal["model-graphic-eq/v2"] = "model-graphic-eq/v2"
EQ_FREQUENCIES: tuple[int, ...] = (
    32,
    64,
    125,
    250,
    500,
    1000,
    2000,
    4000,
    8000,
    16000,
)
_MAX_OUTPUT_TOKENS = 2_000
_SAFE_ERROR_CODE = re.compile(r"^[a-z0-9_]{1,64}$")

EqGain = Annotated[float, Field(ge=-12.0, le=12.0)]


class _StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class EqDraftInput(_StrictModel):
    schema_version: Literal["assistant-eq-draft-input/v2"]
    goal: str = Field(min_length=2, max_length=1000)
    band_frequencies_hz: list[int] = Field(min_length=10, max_length=10)
    gain_min_db: Literal[-12] = -12
    gain_max_db: Literal[12] = 12
    gain_step_db: float = Field(default=0.5, ge=0.5, le=0.5)
    local_guidance: EqLocalGuidance


class EqBandGuidance(_StrictModel):
    frequency_hz: int
    baseline_gain_db: EqGain
    minimum_gain_db: EqGain
    maximum_gain_db: EqGain


class EqLocalGuidance(_StrictModel):
    method: Literal["deterministic-eq-intent/v1"]
    matched_rules: list[str] = Field(max_length=8)
    bands: list[EqBandGuidance] = Field(min_length=10, max_length=10)


class EqDraftOutput(_StrictModel):
    model_config = ConfigDict(extra="forbid", frozen=True, strict=True)

    schema_version: Literal["assistant-eq-draft-output/v1"]
    gains_db: list[EqGain] = Field(min_length=10, max_length=10)
    rationale: str = Field(min_length=1, max_length=1000)
    cautions: list[Annotated[str, Field(min_length=1, max_length=256)]] = Field(
        max_length=5,
    )

    @field_validator("rationale", mode="before")
    @classmethod
    def bound_incidental_rationale(cls, value: object) -> object:
        if isinstance(value, str) and len(value) > 1000:
            return f"{value[:997].rstrip()}..."
        return value

    @field_validator("cautions", mode="before")
    @classmethod
    def bound_incidental_cautions(cls, value: object) -> object:
        if not isinstance(value, list):
            return value
        bounded: list[object] = []
        for item in value[:5]:
            if isinstance(item, str) and len(item) > 256:
                item = f"{item[:253].rstrip()}..."
            bounded.append(item)
        return bounded

    @field_validator("gains_db")
    @classmethod
    def gains_use_supported_step(cls, values: list[float]) -> list[float]:
        if any(abs(value * 2 - round(value * 2)) > 1e-9 for value in values):
            raise ValueError("EQ gains must use 0.5 dB steps")
        return values


class EqPresetBand(_StrictModel):
    frequency: int
    gain: EqGain


class EqPresetDraft(_StrictModel):
    name: str = Field(min_length=1, max_length=128)
    goal: str = Field(min_length=2, max_length=1000)
    bands: list[EqPresetBand] = Field(min_length=10, max_length=10)
    rationale: str = Field(min_length=1, max_length=1000)
    cautions: list[Annotated[str, Field(min_length=1, max_length=256)]] = Field(
        max_length=5
    )

    @field_validator("bands")
    @classmethod
    def bands_use_canonical_graphic_eq(
        cls,
        values: list[EqPresetBand],
    ) -> list[EqPresetBand]:
        if tuple(band.frequency for band in values) != EQ_FREQUENCIES:
            raise ValueError("EQ draft bands must use the canonical frequency order")
        if any(abs(band.gain * 2 - round(band.gain * 2)) > 1e-9 for band in values):
            raise ValueError("EQ draft gains must use 0.5 dB steps")
        return values


class EqBandExpectation(_StrictModel):
    frequency_hz: int
    minimum_gain_db: float = Field(ge=-12.0, le=12.0)
    maximum_gain_db: float = Field(ge=-12.0, le=12.0)

    @model_validator(mode="after")
    def valid_band_range(self) -> EqBandExpectation:
        if self.frequency_hz not in EQ_FREQUENCIES:
            raise ValueError("EQ expectation must use a canonical band frequency")
        if self.minimum_gain_db > self.maximum_gain_db:
            raise ValueError("EQ expectation minimum cannot exceed its maximum")
        return self


class EqQualityCase(_StrictModel):
    id: str = Field(min_length=1, max_length=128)
    description: str = Field(min_length=1, max_length=512)
    goal: str = Field(min_length=2, max_length=1000)
    expectations: list[EqBandExpectation] = Field(min_length=1, max_length=10)

    @model_validator(mode="after")
    def unique_expected_bands(self) -> EqQualityCase:
        frequencies = [item.frequency_hz for item in self.expectations]
        if len(set(frequencies)) != len(frequencies):
            raise ValueError("EQ expectation frequencies must be unique within a case")
        return self


class EqQualitySuite(_StrictModel):
    schema_version: Literal["assistant-eq-quality/v1"]
    id: str = Field(min_length=1, max_length=128)
    cases: list[EqQualityCase] = Field(min_length=1, max_length=20)

    @model_validator(mode="after")
    def unique_case_ids(self) -> EqQualitySuite:
        case_ids = [case.id for case in self.cases]
        if len(set(case_ids)) != len(case_ids):
            raise ValueError("case IDs must be unique within an EQ suite")
        return self


class EqQualityCaseResult(_StrictModel):
    id: str
    description: str
    passed: bool
    failures: list[str]


class EqQualityEvaluationResult(_StrictModel):
    schema_version: Literal["assistant-eq-quality-result/v1"] = (
        "assistant-eq-quality-result/v1"
    )
    suite_id: str
    engine_id: Literal["model-graphic-eq/v2"] = EQ_DRAFT_ENGINE_ID
    passed: bool
    passed_cases: int
    total_cases: int
    cases: list[EqQualityCaseResult]


class ModelEqError(RuntimeError):
    def __init__(self, code: str, *, diagnostic: str | None = None) -> None:
        super().__init__(code)
        self.code = code
        self.diagnostic = diagnostic


StructuredEqExecutor = Callable[[StructuredModelRequest], StructuredModelResult]


@dataclass(frozen=True)
class _EqIntentRule:
    id: str
    terms: frozenset[str]
    gains: tuple[float, ...]


_EQ_INTENT_RULES: tuple[_EqIntentRule, ...] = (
    _EqIntentRule(
        "warmth",
        frozenset({"warm", "warmer", "wooden", "body", "intimate", "tavern"}),
        (0.0, 0.5, 1.0, 1.5, 0.5, 0.0, -0.5, -0.5, -1.0, -0.5),
    ),
    _EqIntentRule(
        "reduce-harshness",
        frozenset({"harsh", "piercing", "brittle", "fatigue", "shrill", "sibilant"}),
        (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -0.5, -2.0, -1.0, -0.5),
    ),
    _EqIntentRule(
        "clarity",
        frozenset({"clarity", "clear", "dialogue", "understandable", "definition"}),
        (-0.5, -0.5, 0.0, -0.5, -0.5, 0.0, 1.0, 0.5, 0.0, 0.0),
    ),
    _EqIntentRule(
        "bass-weight",
        frozenset({"bass", "low", "low-end", "weight", "thump"}),
        (1.0, 1.5, 1.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
    ),
    _EqIntentRule(
        "reduce-mud",
        frozenset({"muddy", "mud", "boxy", "boomy", "boom"}),
        (0.0, -0.5, -1.0, -1.5, -1.0, -0.5, 0.0, 0.0, 0.0, 0.0),
    ),
)
_EQ_WORDS = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)?")


def build_local_eq_guidance(goal: str) -> EqLocalGuidance:
    """Create a conservative deterministic baseline and a narrow refinement envelope."""

    tokens = frozenset(_EQ_WORDS.findall(goal.casefold()))
    matched = [rule for rule in _EQ_INTENT_RULES if tokens & rule.terms]
    baseline = [0.0] * len(EQ_FREQUENCIES)
    for rule in matched:
        baseline = [left + right for left, right in zip(baseline, rule.gains, strict=True)]
    baseline = [round(max(-4.0, min(4.0, value)) * 2.0) / 2.0 for value in baseline]
    bands = [
        EqBandGuidance(
            frequency_hz=frequency,
            baseline_gain_db=value,
            minimum_gain_db=max(-6.0, min(0.0, value - 1.5)),
            maximum_gain_db=min(4.0, max(0.0, value + 1.5)),
        )
        for frequency, value in zip(EQ_FREQUENCIES, baseline, strict=True)
    ]
    return EqLocalGuidance(
        method="deterministic-eq-intent/v1",
        matched_rules=[rule.id for rule in matched],
        bands=bands,
    )


_EQ_TASK = StructuredTaskDefinition(
    task_id="assistant-eq-draft",
    role="A conservative graphic-EQ refinement engine.",
    objective=(
        "Refine a deterministic ten-band baseline within server-owned safety envelopes "
        "for later human listening review."
    ),
    untrusted_data=("goal",),
    rules=numbered_rules(
        "Use the supplied bands in their exact order. Start from local_guidance.baseline_gain_db and change a band only when the sound goal supports the change.",
        "Every gain must stay within that band's minimum_gain_db and maximum_gain_db and use 0.5 dB steps. Prefer the smallest effective change.",
        "Avoid broad boosts, extreme bass, excessive presence, or curves likely to reduce headroom. This is a review draft, not a promise about a recording or playback system.",
        "If no local intent rule matched, remain close to neutral and use cautions to explain genuine ambiguity.",
        "rationale briefly relates the curve to the stated sound goal. cautions contains only practical listening or headroom checks, not hidden reasoning.",
    ),
)


def _safe_execution_error(code: str | None) -> str:
    if code is not None and _SAFE_ERROR_CODE.fullmatch(code):
        return f"model_execution_{code}"
    return "model_execution_failed"


def generate_eq_draft(
    name: str,
    goal: str,
    execute: StructuredEqExecutor,
) -> EqPresetDraft:
    """Ask a model for bounded gains, then construct every preset field locally."""

    guidance = build_local_eq_guidance(goal)
    model_input = EqDraftInput(
        schema_version=EQ_DRAFT_INPUT_CONTRACT,
        goal=goal,
        band_frequencies_hz=list(EQ_FREQUENCIES),
        local_guidance=guidance,
    )
    result = execute(
        build_structured_request(
            _EQ_TASK,
            model_input,
            EqDraftOutput,
            output_example={
                "schema_version": EQ_DRAFT_OUTPUT_CONTRACT,
                "gains_db": [band.baseline_gain_db for band in guidance.bands],
                "rationale": "Conservative refinement of the local baseline.",
                "cautions": ["Review on the intended speakers at matched volume."],
            },
            max_output_tokens=_MAX_OUTPUT_TOKENS,
        )
    )
    if not result.succeeded or result.payload is None:
        raise ModelEqError(_safe_execution_error(result.error_code))
    if result.finish_reason in {"length", "max_tokens"}:
        raise ModelEqError("model_output_incomplete")
    try:
        output = EqDraftOutput.model_validate(result.payload)
    except ValidationError as exc:
        raise ModelEqError(
            "model_output_schema_invalid",
            diagnostic=safe_validation_diagnostic(exc, EqDraftOutput),
        ) from exc
    if any(
        not guidance_band.minimum_gain_db
        <= gain
        <= guidance_band.maximum_gain_db
        for gain, guidance_band in zip(output.gains_db, guidance.bands, strict=True)
    ):
        raise ModelEqError("model_output_outside_local_envelope")
    return EqPresetDraft(
        name=name,
        goal=goal,
        bands=[
            EqPresetBand(frequency=frequency, gain=gain)
            for frequency, gain in zip(EQ_FREQUENCIES, output.gains_db, strict=True)
        ],
        rationale=output.rationale,
        cautions=output.cautions,
    )


def load_eq_quality_suite(path: Path) -> EqQualitySuite:
    return EqQualitySuite.model_validate_json(path.read_text(encoding="utf-8"))


def evaluate_eq_model(
    execute: StructuredEqExecutor,
    suite: EqQualitySuite,
    *,
    on_case_complete: Callable[[int, int], None] | None = None,
) -> EqQualityEvaluationResult:
    results: list[EqQualityCaseResult] = []
    total = len(suite.cases)
    for index, case in enumerate(suite.cases, start=1):
        failures: list[str] = []
        try:
            draft = generate_eq_draft("Synthetic EQ check", case.goal, execute)
            gains = {
                band.frequency: band.gain for band in draft.bands
            }
            for expected in case.expectations:
                actual = gains.get(expected.frequency_hz)
                if actual is None or not (
                    expected.minimum_gain_db <= actual <= expected.maximum_gain_db
                ):
                    failures.append(
                        f"{expected.frequency_hz} Hz must be between "
                        f"{expected.minimum_gain_db:g} and "
                        f"{expected.maximum_gain_db:g} dB."
                    )
        except ModelEqError as exc:
            failure = f"EQ model error: {exc.code}"
            if exc.diagnostic is not None:
                failure += f" ({exc.diagnostic})"
            failures.append(failure)
        results.append(
            EqQualityCaseResult(
                id=case.id,
                description=case.description,
                passed=not failures,
                failures=failures,
            )
        )
        if on_case_complete is not None:
            on_case_complete(index, total)
    passed_cases = sum(item.passed for item in results)
    return EqQualityEvaluationResult(
        suite_id=suite.id,
        passed=passed_cases == total,
        passed_cases=passed_cases,
        total_cases=total,
        cases=results,
    )
