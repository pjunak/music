"""Versioned, provider-neutral playlist recommendation evaluation.

Evaluation suites are synthetic and read-only. They describe representative
library evidence plus observable quality expectations; engines still run
through the same public suggestion contract used by the application.
"""

from __future__ import annotations

import hashlib
import json
from collections.abc import Iterable
from pathlib import Path
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, model_validator

from app.assistant.engine import PlaylistSuggestionEngine, TrackAnalysisProfile
from app.assistant.schemas import PlaylistSuggestionRequest, PlaylistSuggestionResponse


class StrictEvaluationModel(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class EvaluationAnalysisProfile(StrictEvaluationModel):
    energy: float = Field(ge=0.0, le=1.0)
    brightness: float = Field(ge=0.0, le=1.0)
    tension: float = Field(ge=0.0, le=1.0)
    moods: list[str] = Field(default_factory=list, max_length=50)
    evidence: list[str] = Field(default_factory=list, max_length=50)
    confidence: Literal["high", "medium", "low"]

    def to_engine_profile(self) -> TrackAnalysisProfile:
        return TrackAnalysisProfile(
            energy=self.energy,
            brightness=self.brightness,
            tension=self.tension,
            moods=tuple(self.moods),
            evidence=tuple(self.evidence),
            confidence=self.confidence,
        )


class EvaluationSignalProfile(StrictEvaluationModel):
    analyzer_id: str = Field(default="evaluation-signal/v1", min_length=1, max_length=128)
    energy: float = Field(ge=0.0, le=1.0)
    brightness: float = Field(ge=0.0, le=1.0)
    tension: float = Field(ge=0.0, le=1.0)
    tempo_bpm: float | None = Field(default=None, gt=0.0, le=999.0)
    confidence: Literal["high", "medium", "low"]


class EvaluationTrack(StrictEvaluationModel):
    id: int = Field(gt=0)
    path: str = Field(min_length=1, max_length=1_000)
    title: str = Field(min_length=1, max_length=500)
    display_title: str = Field(default="", max_length=500)
    artist: str = Field(default="", max_length=500)
    album: str = Field(default="", max_length=500)
    origin: str = Field(default="", max_length=500)
    genre: str = Field(default="", max_length=500)
    length_s: float = Field(default=180.0, ge=0.0, le=86_400.0)
    bpm: int | None = Field(default=None, ge=1, le=999)
    manual_tags: list[str] = Field(default_factory=list, max_length=100)
    analysis: EvaluationAnalysisProfile | None = None
    signal: EvaluationSignalProfile | None = None


class EvaluationExpectations(StrictEvaluationModel):
    top_k: int = Field(default=5, ge=1, le=100)
    relevant_track_ids: list[int] = Field(min_length=1, max_length=1_000)
    forbidden_track_ids: list[int] = Field(default_factory=list, max_length=1_000)
    required_default_track_ids: list[int] = Field(default_factory=list, max_length=1_000)
    order_pairs: list[tuple[int, int]] = Field(default_factory=list, max_length=1_000)

    @model_validator(mode="after")
    def unique_expectations(self) -> EvaluationExpectations:
        duplicated: list[str] = []
        if len(set(self.relevant_track_ids)) != len(self.relevant_track_ids):
            duplicated.append("relevant_track_ids")
        if len(set(self.forbidden_track_ids)) != len(self.forbidden_track_ids):
            duplicated.append("forbidden_track_ids")
        if len(set(self.required_default_track_ids)) != len(
            self.required_default_track_ids
        ):
            duplicated.append("required_default_track_ids")
        if len(set(self.order_pairs)) != len(self.order_pairs):
            duplicated.append("order_pairs")
        if duplicated:
            raise ValueError(f"expectation values must be unique: {', '.join(duplicated)}")
        return self


class EvaluationThresholds(StrictEvaluationModel):
    min_precision_at_k: float = Field(default=0.0, ge=0.0, le=1.0)
    min_recall_at_k: float = Field(default=0.0, ge=0.0, le=1.0)
    min_reciprocal_rank: float = Field(default=0.0, ge=0.0, le=1.0)
    min_required_selected_recall: float = Field(default=0.0, ge=0.0, le=1.0)
    min_order_pair_accuracy: float = Field(default=0.0, ge=0.0, le=1.0)
    min_reason_coverage: float = Field(default=1.0, ge=0.0, le=1.0)
    max_forbidden_candidates: int = Field(default=0, ge=0)
    require_deterministic: bool = False


class PlaylistEvaluationCase(StrictEvaluationModel):
    id: str = Field(pattern=r"^[a-z0-9][a-z0-9-]{1,63}$")
    description: str = Field(min_length=1, max_length=1_000)
    request: PlaylistSuggestionRequest
    tracks: list[EvaluationTrack] = Field(min_length=1, max_length=1_000)
    expectations: EvaluationExpectations
    thresholds: EvaluationThresholds = Field(default_factory=EvaluationThresholds)

    @model_validator(mode="after")
    def validate_references(self) -> PlaylistEvaluationCase:
        track_ids = [track.id for track in self.tracks]
        known = set(track_ids)
        if len(known) != len(track_ids):
            raise ValueError("track IDs must be unique within a case")

        expected = self.expectations
        relevant = set(expected.relevant_track_ids)
        forbidden = set(expected.forbidden_track_ids)
        required = set(expected.required_default_track_ids)
        referenced = relevant | forbidden | required
        referenced.update(track_id for pair in expected.order_pairs for track_id in pair)
        unknown = sorted(referenced - known)
        if unknown:
            raise ValueError(f"expectations reference unknown track IDs: {unknown}")
        if relevant & forbidden:
            raise ValueError("relevant and forbidden track IDs must be disjoint")
        if not required <= relevant:
            raise ValueError("required default tracks must also be relevant")
        if expected.top_k > self.request.candidate_limit:
            raise ValueError("top_k cannot exceed request.candidate_limit")
        if any(before == after for before, after in expected.order_pairs):
            raise ValueError("order pairs must reference two different tracks")
        if (
            not required
            and self.thresholds.min_required_selected_recall > 0.0
        ):
            raise ValueError(
                "required selected recall needs required_default_track_ids"
            )
        if (
            not expected.order_pairs
            and self.thresholds.min_order_pair_accuracy > 0.0
        ):
            raise ValueError("order accuracy needs at least one order pair")
        return self


class PlaylistEvaluationSuite(StrictEvaluationModel):
    schema_version: Literal["playlist-evaluation/v1"]
    id: str = Field(pattern=r"^[a-z0-9][a-z0-9-]{1,63}$")
    description: str = Field(min_length=1, max_length=2_000)
    cases: list[PlaylistEvaluationCase] = Field(min_length=1, max_length=200)

    @model_validator(mode="after")
    def unique_case_ids(self) -> PlaylistEvaluationSuite:
        case_ids = [case.id for case in self.cases]
        if len(set(case_ids)) != len(case_ids):
            raise ValueError("case IDs must be unique within a suite")
        return self


class EvaluationMetrics(StrictEvaluationModel):
    precision_at_k: float = Field(ge=0.0, le=1.0)
    recall_at_k: float = Field(ge=0.0, le=1.0)
    reciprocal_rank: float = Field(ge=0.0, le=1.0)
    required_selected_recall: float | None = Field(default=None, ge=0.0, le=1.0)
    order_pair_accuracy: float | None = Field(default=None, ge=0.0, le=1.0)
    reason_coverage: float = Field(ge=0.0, le=1.0)
    forbidden_candidate_count: int = Field(ge=0)
    unknown_candidate_count: int = Field(ge=0)
    excluded_candidate_count: int = Field(ge=0)
    source_mismatch_count: int = Field(ge=0)
    deterministic: bool | None
    contract_valid: bool


class EvaluationCaseResult(StrictEvaluationModel):
    id: str
    description: str
    passed: bool
    metrics: EvaluationMetrics
    failures: list[str]
    top_track_ids: list[int]
    selected_track_ids: list[int]
    response_fingerprint: str


class EvaluationSummary(StrictEvaluationModel):
    cases: int = Field(ge=0)
    passed_cases: int = Field(ge=0)
    failed_cases: int = Field(ge=0)
    mean_precision_at_k: float = Field(ge=0.0, le=1.0)
    mean_recall_at_k: float = Field(ge=0.0, le=1.0)
    mean_reciprocal_rank: float = Field(ge=0.0, le=1.0)
    mean_required_selected_recall: float | None = Field(default=None, ge=0.0, le=1.0)
    mean_order_pair_accuracy: float | None = Field(default=None, ge=0.0, le=1.0)
    mean_reason_coverage: float = Field(ge=0.0, le=1.0)


class PlaylistEvaluationResult(StrictEvaluationModel):
    schema_version: Literal["playlist-evaluation-result/v1"]
    suite_id: str
    engine_id: str
    passed: bool
    summary: EvaluationSummary
    cases: list[EvaluationCaseResult]


def load_evaluation_suite(path: Path) -> PlaylistEvaluationSuite:
    return PlaylistEvaluationSuite.model_validate_json(path.read_text(encoding="utf-8"))


def _response_fingerprint(response: PlaylistSuggestionResponse) -> str:
    payload = json.dumps(
        response.model_dump(mode="json"),
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def _mean(values: Iterable[float]) -> float | None:
    collected = list(values)
    if not collected:
        return None
    return round(sum(collected) / len(collected), 4)


def _evaluate_case(
    engine: PlaylistSuggestionEngine,
    case: PlaylistEvaluationCase,
) -> EvaluationCaseResult:
    tracks = list(case.tracks)
    profiles = {
        track.id: track.analysis.to_engine_profile()
        for track in tracks
        if track.analysis is not None
    }
    manual_tags = {
        track.id: tuple(track.manual_tags)
        for track in tracks
        if track.manual_tags
    }
    signals = {
        track.id: track.signal
        for track in tracks
        if track.signal is not None
    }
    response = engine.suggest(
        tracks,
        case.request,
        profiles=profiles,
        manual_tags=manual_tags,
        signal_profiles=signals,
    )
    deterministic: bool | None = None
    if case.thresholds.require_deterministic:
        repeated = engine.suggest(
            tracks,
            case.request,
            profiles=profiles,
            manual_tags=manual_tags,
            signal_profiles=signals,
        )
        deterministic = response.model_dump(mode="json") == repeated.model_dump(mode="json")

    candidates = response.candidates
    candidate_ids = [candidate.track_id for candidate in candidates]
    top_ids = candidate_ids[: case.expectations.top_k]
    selected = [candidate for candidate in candidates if candidate.default_selected]
    selected_ids = [candidate.track_id for candidate in selected]
    relevant = set(case.expectations.relevant_track_ids)
    relevant_in_top = sum(track_id in relevant for track_id in top_ids)
    precision = relevant_in_top / len(top_ids) if top_ids else 0.0
    recall = relevant_in_top / len(relevant)
    reciprocal_rank = next(
        (1.0 / rank for rank, track_id in enumerate(candidate_ids, start=1) if track_id in relevant),
        0.0,
    )

    required = set(case.expectations.required_default_track_ids)
    required_selected_recall = (
        len(required & set(selected_ids)) / len(required) if required else None
    )
    positions = {
        candidate.track_id: candidate.sequence_position
        for candidate in selected
        if candidate.sequence_position is not None
    }
    order_pairs = case.expectations.order_pairs
    order_pair_accuracy = (
        sum(
            before in positions
            and after in positions
            and positions[before] < positions[after]
            for before, after in order_pairs
        )
        / len(order_pairs)
        if order_pairs
        else None
    )
    top_candidates = candidates[: case.expectations.top_k]
    reason_coverage = (
        sum(bool(candidate.reasons) for candidate in top_candidates) / len(top_candidates)
        if top_candidates
        else 0.0
    )

    known_ids = {track.id for track in tracks}
    track_by_id = {track.id: track for track in tracks}
    forbidden = set(case.expectations.forbidden_track_ids)
    excluded = set(case.request.exclude_track_ids)
    unknown_count = sum(track_id not in known_ids for track_id in candidate_ids)
    forbidden_count = sum(track_id in forbidden for track_id in candidate_ids)
    excluded_count = sum(track_id in excluded for track_id in candidate_ids)
    source_mismatch_count = 0
    for candidate in candidates:
        source = track_by_id.get(candidate.track_id)
        if source is None:
            continue
        if (
            candidate.path != source.path
            or candidate.title != source.title
            or candidate.display_title != source.display_title
            or candidate.artist != source.artist
            or candidate.album != source.album
            or candidate.origin != source.origin
            or candidate.genre != source.genre
            or candidate.length_s != source.length_s
            or candidate.bpm != source.bpm
            or set(candidate.manual_tags) != set(source.manual_tags)
        ):
            source_mismatch_count += 1
    selected_positions = [candidate.sequence_position for candidate in selected]
    concrete_positions = [
        position for position in selected_positions if position is not None
    ]
    expected_positions = list(range(1, len(selected) + 1))
    sequence_valid = (
        len(concrete_positions) == len(selected_positions)
        and sorted(concrete_positions) == expected_positions
        and response.plan.selected_tracks == len(selected)
    )
    contract_valid = (
        response.engine == engine.engine_id
        and response.library_tracks == len(tracks)
        and len(candidate_ids) == len(set(candidate_ids))
        and len(candidates) <= case.request.candidate_limit
        and 0 <= response.eligible_tracks <= len(tracks)
        and len(candidates) <= response.eligible_tracks
        and unknown_count == 0
        and excluded_count == 0
        and source_mismatch_count == 0
        and sequence_valid
    )

    metrics = EvaluationMetrics(
        precision_at_k=round(precision, 4),
        recall_at_k=round(recall, 4),
        reciprocal_rank=round(reciprocal_rank, 4),
        required_selected_recall=(
            round(required_selected_recall, 4)
            if required_selected_recall is not None
            else None
        ),
        order_pair_accuracy=(
            round(order_pair_accuracy, 4) if order_pair_accuracy is not None else None
        ),
        reason_coverage=round(reason_coverage, 4),
        forbidden_candidate_count=forbidden_count,
        unknown_candidate_count=unknown_count,
        excluded_candidate_count=excluded_count,
        source_mismatch_count=source_mismatch_count,
        deterministic=deterministic,
        contract_valid=contract_valid,
    )
    thresholds = case.thresholds
    failures: list[str] = []
    if metrics.precision_at_k < thresholds.min_precision_at_k:
        failures.append("precision_at_k below threshold")
    if metrics.recall_at_k < thresholds.min_recall_at_k:
        failures.append("recall_at_k below threshold")
    if metrics.reciprocal_rank < thresholds.min_reciprocal_rank:
        failures.append("reciprocal_rank below threshold")
    if (
        metrics.required_selected_recall is not None
        and metrics.required_selected_recall < thresholds.min_required_selected_recall
    ):
        failures.append("required_selected_recall below threshold")
    if (
        metrics.order_pair_accuracy is not None
        and metrics.order_pair_accuracy < thresholds.min_order_pair_accuracy
    ):
        failures.append("order_pair_accuracy below threshold")
    if metrics.reason_coverage < thresholds.min_reason_coverage:
        failures.append("reason_coverage below threshold")
    if metrics.forbidden_candidate_count > thresholds.max_forbidden_candidates:
        failures.append("forbidden candidate limit exceeded")
    if thresholds.require_deterministic and metrics.deterministic is not True:
        failures.append("engine response is not deterministic")
    if not metrics.contract_valid:
        failures.append("suggestion response violates the evaluation contract")

    return EvaluationCaseResult(
        id=case.id,
        description=case.description,
        passed=not failures,
        metrics=metrics,
        failures=failures,
        top_track_ids=top_ids,
        selected_track_ids=selected_ids,
        response_fingerprint=_response_fingerprint(response),
    )


def _engine_error_result(
    case: PlaylistEvaluationCase,
    error: Exception,
) -> EvaluationCaseResult:
    return EvaluationCaseResult(
        id=case.id,
        description=case.description,
        passed=False,
        metrics=EvaluationMetrics(
            precision_at_k=0.0,
            recall_at_k=0.0,
            reciprocal_rank=0.0,
            required_selected_recall=None,
            order_pair_accuracy=None,
            reason_coverage=0.0,
            forbidden_candidate_count=0,
            unknown_candidate_count=0,
            excluded_candidate_count=0,
            source_mismatch_count=0,
            deterministic=None,
            contract_valid=False,
        ),
        failures=[f"engine raised {type(error).__name__}"],
        top_track_ids=[],
        selected_track_ids=[],
        response_fingerprint="",
    )


def evaluate_playlist_engine(
    engine: PlaylistSuggestionEngine,
    suite: PlaylistEvaluationSuite,
) -> PlaylistEvaluationResult:
    cases: list[EvaluationCaseResult] = []
    for case in suite.cases:
        try:
            cases.append(_evaluate_case(engine, case))
        except Exception as exc:
            cases.append(_engine_error_result(case, exc))
    passed_cases = sum(case.passed for case in cases)
    required_recall = _mean(
        case.metrics.required_selected_recall
        for case in cases
        if case.metrics.required_selected_recall is not None
    )
    order_accuracy = _mean(
        case.metrics.order_pair_accuracy
        for case in cases
        if case.metrics.order_pair_accuracy is not None
    )
    return PlaylistEvaluationResult(
        schema_version="playlist-evaluation-result/v1",
        suite_id=suite.id,
        engine_id=engine.engine_id,
        passed=passed_cases == len(cases),
        summary=EvaluationSummary(
            cases=len(cases),
            passed_cases=passed_cases,
            failed_cases=len(cases) - passed_cases,
            mean_precision_at_k=_mean(
                case.metrics.precision_at_k for case in cases
            )
            or 0.0,
            mean_recall_at_k=_mean(case.metrics.recall_at_k for case in cases)
            or 0.0,
            mean_reciprocal_rank=_mean(
                case.metrics.reciprocal_rank for case in cases
            )
            or 0.0,
            mean_required_selected_recall=required_recall,
            mean_order_pair_accuracy=order_accuracy,
            mean_reason_coverage=_mean(
                case.metrics.reason_coverage for case in cases
            )
            or 0.0,
        ),
        cases=cases,
    )
