"""Hybrid optional-model playlist planner.

The local planner remains responsible for eligibility, filtering, evidence,
and the bounded candidate pool. The model may return only track IDs; every
field in the public suggestion response is reconstructed from trusted local
data. This engine is initially used only by the synthetic evaluation CLI.
"""

import re
from collections.abc import Callable, Mapping, Sequence
from copy import deepcopy
from typing import Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field, ValidationError, model_validator

from app.assistant.engine import (
    SuggestionEngineError,
    TrackAnalysisProfile,
    TrackLike,
    TrackSignalProfile,
)
from app.assistant.local import expanded_retrieval_prompt, local_playlist_planner
from app.assistant.providers.execution import (
    StructuredModelRequest,
    StructuredModelResult,
)
from app.assistant.schema_diagnostics import safe_validation_diagnostic
from app.assistant.schemas import (
    PlaylistCandidate,
    PlaylistIntent,
    PlaylistPlan,
    PlaylistSuggestionRequest,
    PlaylistSuggestionResponse,
)
from app.assistant.structured_harness import (
    StructuredTaskDefinition,
    build_structured_request,
    numbered_rules,
)

MODEL_PLAYLIST_INPUT_CONTRACT: Literal["assistant-playlist-planner-input/v2"] = (
    "assistant-playlist-planner-input/v2"
)
MODEL_PLAYLIST_OUTPUT_CONTRACT: Literal["assistant-playlist-planner-output/v1"] = (
    "assistant-playlist-planner-output/v1"
)
_MAX_MODEL_CANDIDATES = 100
_MAX_MODEL_OUTPUT_TOKENS = 8_000
_SAFE_ERROR_CODE = re.compile(r"^[a-z0-9_]{1,64}$")

BoundedTag = Annotated[str, Field(min_length=1, max_length=64)]


class _StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class ModelPlaylistAudioSignal(_StrictModel):
    analyzer_id: str = Field(min_length=1, max_length=128)
    energy: float = Field(ge=0.0, le=1.0)
    brightness: float = Field(ge=0.0, le=1.0)
    tension: float = Field(ge=0.0, le=1.0)
    tempo_bpm: float | None = Field(default=None, gt=0.0, le=999.0)
    confidence: Literal["high", "medium", "low"]


class ModelPlaylistCandidateInput(_StrictModel):
    track_id: int = Field(gt=0)
    title: str = Field(max_length=512)
    display_title: str = Field(max_length=512)
    artist: str = Field(max_length=512)
    album: str = Field(max_length=512)
    origin: str = Field(max_length=512)
    genre: str = Field(max_length=128)
    length_s: float = Field(ge=0.0)
    bpm: int | None
    manual_tags: list[BoundedTag] = Field(max_length=32)
    analysis_tags: list[BoundedTag] = Field(max_length=50)
    local_match_score: float = Field(ge=0.0, le=1.0)
    planning_energy: float = Field(ge=0.0, le=1.0)
    evidence_confidence: Literal["high", "medium", "low"]
    audio_signal: ModelPlaylistAudioSignal | None
    local_rank: int = Field(ge=1, le=100)
    local_default_selected: bool
    local_sequence_position: int | None = Field(default=None, ge=1, le=100)
    effective_bpm: float | None = Field(default=None, gt=0.0, le=999.0)
    effective_bpm_source: Literal["metadata", "local-audio", "unknown"]


class ModelPlaylistLocalPlan(_StrictModel):
    selected_track_ids: list[int] = Field(max_length=100)
    selected_duration_s: float = Field(ge=0.0)
    target_duration_s: float = Field(gt=0.0)
    energy_curve: Literal["steady", "rising", "falling", "arc"]


class ModelPlaylistInput(_StrictModel):
    schema_version: Literal["assistant-playlist-planner-input/v2"]
    request: PlaylistSuggestionRequest
    intent_hint: PlaylistIntent
    local_plan: ModelPlaylistLocalPlan
    candidates: list[ModelPlaylistCandidateInput] = Field(max_length=100)


class ModelPlaylistOutput(_StrictModel):
    model_config = ConfigDict(extra="forbid", frozen=True, strict=True)

    schema_version: Literal["assistant-playlist-planner-output/v1"]
    ranked_track_ids: list[int] = Field(max_length=100)
    selected_track_ids: list[int] = Field(max_length=100)

    @model_validator(mode="after")
    def unique_known_selections(self) -> ModelPlaylistOutput:
        if len(set(self.ranked_track_ids)) != len(self.ranked_track_ids):
            raise ValueError("ranked_track_ids must be unique")
        if len(set(self.selected_track_ids)) != len(self.selected_track_ids):
            raise ValueError("selected_track_ids must be unique")
        if not set(self.selected_track_ids) <= set(self.ranked_track_ids):
            raise ValueError("selected_track_ids must be ranked")
        return self


type StructuredPlaylistExecutor = Callable[
    [StructuredModelRequest],
    StructuredModelResult,
]


def _model_candidate(
    candidate: PlaylistCandidate,
    local_rank: int,
) -> ModelPlaylistCandidateInput:
    signal = candidate.audio_signal
    effective_bpm = (
        float(candidate.bpm)
        if candidate.bpm is not None
        else signal.tempo_bpm
        if signal is not None
        else None
    )
    return ModelPlaylistCandidateInput(
        track_id=candidate.track_id,
        title=candidate.title,
        display_title=candidate.display_title,
        artist=candidate.artist,
        album=candidate.album,
        origin=candidate.origin,
        genre=candidate.genre,
        length_s=candidate.length_s,
        bpm=candidate.bpm,
        manual_tags=candidate.manual_tags,
        analysis_tags=candidate.analysis_tags,
        local_match_score=candidate.match_score,
        planning_energy=candidate.planning_energy,
        evidence_confidence=candidate.confidence,
        audio_signal=(
            ModelPlaylistAudioSignal(
                analyzer_id=signal.analyzer_id,
                energy=signal.energy,
                brightness=signal.brightness,
                tension=signal.tension,
                tempo_bpm=signal.tempo_bpm,
                confidence=signal.confidence,
            )
            if signal is not None
            else None
        ),
        local_rank=local_rank,
        local_default_selected=candidate.default_selected,
        local_sequence_position=candidate.sequence_position,
        effective_bpm=effective_bpm,
        effective_bpm_source=(
            "metadata"
            if candidate.bpm is not None
            else "local-audio"
            if signal is not None and signal.tempo_bpm is not None
            else "unknown"
        ),
    )


_PLAYLIST_TASK = StructuredTaskDefinition(
    task_id="assistant-playlist-planner",
    role="A cautious playlist refinement engine operating on a local plan.",
    objective=(
        "Refine the server's deterministic candidate ranking and playback sequence "
        "without inventing tracks, metadata, scores, or evidence."
    ),
    untrusted_data=(
        "request.prompt",
        "candidate titles",
        "artists",
        "albums",
        "origins",
        "genres",
        "manual_tags",
        "analysis_tags",
    ),
    rules=numbered_rules(
        "Every candidate already passed local exclusions and BPM eligibility. Use only candidate track_id values and never infer missing candidates.",
        "Treat manual_tags as operator-owned evidence, then explicit descriptive metadata, then generated analysis_tags and numeric local evidence. A weak source must not overrule a strong source without clear support.",
        "Use local_match_score, local_rank, local_default_selected, and local_plan as the deterministic baseline. Change that baseline only when the supplied evidence better satisfies the request.",
        "Respect request.candidate_limit, target duration, energy_curve, effective_bpm, and the intended playback order. Unknown BPM is not zero BPM.",
        "ranked_track_ids contains the best review candidates in relevance order. selected_track_ids is a unique subset of those IDs in intended playback order.",
        "Do not explain the ranking or copy candidate text into the response; the server reconstructs all public metadata and reasons locally.",
    ),
)


def _safe_execution_error(code: str | None) -> str:
    if code is not None and _SAFE_ERROR_CODE.fullmatch(code):
        return f"model_execution_{code}"
    return "model_execution_failed"


def _closed_playlist_schema(
    schema: dict[str, object],
    *,
    candidate_ids: list[int],
    candidate_limit: int,
) -> dict[str, object]:
    """Constrain both output arrays to the exact server-selected candidate IDs."""

    closed = deepcopy(schema)
    properties = closed.get("properties")
    if not isinstance(properties, dict):
        raise RuntimeError("playlist output schema is missing properties")
    for field in ("ranked_track_ids", "selected_track_ids"):
        array_schema = properties.get(field)
        if not isinstance(array_schema, dict):
            raise RuntimeError(f"playlist output schema is missing {field}")
        item_schema = array_schema.get("items")
        if not isinstance(item_schema, dict):
            raise RuntimeError(f"playlist output schema is missing {field} items")
        item_schema["enum"] = candidate_ids
        array_schema["maxItems"] = min(candidate_limit, len(candidate_ids))
    return closed


class ModelPlaylistPlanner:
    """Model ranking over a locally filtered, privacy-reduced candidate set."""

    engine_id = "model-playlist-planner/v2"

    def __init__(self, executor: StructuredPlaylistExecutor) -> None:
        self._executor = executor

    def suggest(
        self,
        tracks: Sequence[TrackLike],
        request: PlaylistSuggestionRequest,
        profiles: Mapping[int, TrackAnalysisProfile] | None = None,
        manual_tags: Mapping[int, Sequence[str]] | None = None,
        signal_profiles: Mapping[int, TrackSignalProfile] | None = None,
    ) -> PlaylistSuggestionResponse:
        prefilter_limit = min(
            _MAX_MODEL_CANDIDATES,
            max(request.candidate_limit, request.candidate_limit * 3),
        )
        prefilter_request = request.model_copy(
            update={"candidate_limit": prefilter_limit}
        )
        baseline = local_playlist_planner.suggest(
            tracks,
            prefilter_request,
            profiles=profiles,
            manual_tags=manual_tags,
            signal_profiles=signal_profiles,
        )
        expanded_prompt = expanded_retrieval_prompt(request.prompt)
        if expanded_prompt != request.prompt and len(baseline.candidates) < _MAX_MODEL_CANDIDATES:
            expanded_request = prefilter_request.model_copy(
                update={"prompt": expanded_prompt}
            )
            expanded = local_playlist_planner.suggest(
                tracks,
                expanded_request,
                profiles=profiles,
                manual_tags=manual_tags,
                signal_profiles=signal_profiles,
            )
            known_ids = {candidate.track_id for candidate in baseline.candidates}
            recall_candidates = [
                candidate
                for candidate in expanded.candidates
                if candidate.track_id not in known_ids
            ]
            baseline = baseline.model_copy(
                update={
                    "candidates": [
                        *baseline.candidates,
                        *recall_candidates,
                    ][:_MAX_MODEL_CANDIDATES]
                }
            )
        if not baseline.candidates:
            return baseline.model_copy(update={"engine": self.engine_id})

        baseline_selected = sorted(
            (
                item
                for item in baseline.candidates
                if item.default_selected and item.sequence_position is not None
            ),
            key=lambda item: item.sequence_position or 0,
        )
        model_input = ModelPlaylistInput(
            schema_version=MODEL_PLAYLIST_INPUT_CONTRACT,
            request=request,
            intent_hint=baseline.intent,
            local_plan=ModelPlaylistLocalPlan(
                selected_track_ids=[item.track_id for item in baseline_selected],
                selected_duration_s=baseline.plan.selected_duration_s,
                target_duration_s=request.target_minutes * 60.0,
                energy_curve=request.energy_curve,
            ),
            candidates=[
                _model_candidate(item, rank)
                for rank, item in enumerate(baseline.candidates, start=1)
            ],
        )
        candidate_ids = [item.track_id for item in model_input.candidates]
        baseline_ranked_ids = candidate_ids[: request.candidate_limit]
        baseline_ranked_set = set(baseline_ranked_ids)
        baseline_selected_ids = [
            item.track_id
            for item in baseline_selected
            if item.track_id in baseline_ranked_set
        ]
        result = self._executor(
            build_structured_request(
                _PLAYLIST_TASK,
                model_input,
                ModelPlaylistOutput,
                output_example={
                    "schema_version": MODEL_PLAYLIST_OUTPUT_CONTRACT,
                    "ranked_track_ids": baseline_ranked_ids,
                    "selected_track_ids": baseline_selected_ids,
                },
                max_output_tokens=_MAX_MODEL_OUTPUT_TOKENS,
                schema_transform=lambda schema: _closed_playlist_schema(
                    schema,
                    candidate_ids=candidate_ids,
                    candidate_limit=request.candidate_limit,
                ),
            )
        )
        if not result.succeeded or result.payload is None:
            raise SuggestionEngineError(_safe_execution_error(result.error_code))
        if result.finish_reason in {"length", "max_tokens"}:
            raise SuggestionEngineError("model_output_incomplete")
        try:
            model_output = ModelPlaylistOutput.model_validate(result.payload)
        except ValidationError as exc:
            raise SuggestionEngineError(
                "model_output_schema_invalid",
                diagnostic=safe_validation_diagnostic(exc, ModelPlaylistOutput),
            ) from exc

        if len(model_output.ranked_track_ids) > request.candidate_limit:
            raise SuggestionEngineError("model_output_candidate_limit_exceeded")
        candidate_by_id = {item.track_id: item for item in baseline.candidates}
        referenced_ids = set(model_output.ranked_track_ids) | set(
            model_output.selected_track_ids
        )
        if not referenced_ids <= set(candidate_by_id):
            raise SuggestionEngineError("model_output_unknown_track")

        selected_positions = {
            track_id: position
            for position, track_id in enumerate(
                model_output.selected_track_ids,
                start=1,
            )
        }
        candidates = [
            candidate_by_id[track_id].model_copy(
                update={
                    "default_selected": track_id in selected_positions,
                    "sequence_position": selected_positions.get(track_id),
                }
            )
            for track_id in model_output.ranked_track_ids
        ]
        selected = [candidate_by_id[track_id] for track_id in model_output.selected_track_ids]
        return PlaylistSuggestionResponse(
            engine=self.engine_id,
            library_tracks=baseline.library_tracks,
            eligible_tracks=baseline.eligible_tracks,
            intent=baseline.intent,
            plan=PlaylistPlan(
                energy_curve=request.energy_curve,
                selected_tracks=len(selected),
                selected_duration_s=round(
                    sum(item.length_s for item in selected),
                    3,
                ),
                audio_profile_tracks=sum(
                    item.audio_signal is not None for item in candidates
                ),
            ),
            candidates=candidates,
        )
