from __future__ import annotations

import json
from collections.abc import Mapping
from pathlib import Path
from typing import cast

import pytest

from app.assistant.engine import SuggestionEngineError, TrackAnalysisProfile
from app.assistant.evaluation import (
    EvaluationSignalProfile,
    EvaluationTrack,
    PlaylistEvaluationCase,
    evaluate_playlist_engine,
    load_evaluation_suite,
)
from app.assistant.local import local_playlist_planner
from app.assistant.model_playlist import (
    MODEL_PLAYLIST_OUTPUT_CONTRACT,
    ModelPlaylistPlanner,
)
from app.assistant.providers.execution import (
    StructuredModelRequest,
    StructuredModelResult,
)
from app.assistant.schemas import PlaylistSuggestionRequest

SUITE_PATH = (
    Path(__file__).parents[1]
    / "app"
    / "assistant"
    / "evaluation_suites"
    / "playlist-local-v1.json"
)


def _case_inputs(
    case: PlaylistEvaluationCase,
) -> tuple[
    list[EvaluationTrack],
    Mapping[int, TrackAnalysisProfile],
    Mapping[int, tuple[str, ...]],
    Mapping[int, EvaluationSignalProfile],
]:
    tracks = list(case.tracks)
    profiles = {
        track.id: track.analysis.to_engine_profile()
        for track in tracks
        if track.analysis is not None
    }
    manual_tags = {
        track.id: tuple(track.manual_tags) for track in tracks if track.manual_tags
    }
    signals = {
        track.id: track.signal for track in tracks if track.signal is not None
    }
    return tracks, profiles, manual_tags, signals


class CapturingExecutor:
    def __init__(self, payload: dict[str, object]) -> None:
        self.payload = payload
        self.requests: list[StructuredModelRequest] = []

    def __call__(self, request: StructuredModelRequest) -> StructuredModelResult:
        self.requests.append(request)
        return StructuredModelResult(True, None, self.payload)


class ReferenceExecutor:
    """Deterministic fixture model used to exercise the complete engine contract."""

    def __call__(self, request: StructuredModelRequest) -> StructuredModelResult:
        payload = json.loads(request.user_prompt)
        candidates = payload["candidates"]
        limit = payload["request"]["candidate_limit"]
        ranked = candidates[:limit]
        selected: list[dict[str, object]] = []
        selected_seconds = 0.0
        target_seconds = payload["request"]["target_minutes"] * 60
        for candidate in ranked:
            if selected_seconds >= target_seconds:
                break
            selected.append(candidate)
            length = candidate["length_s"]
            selected_seconds += length if length > 0 else 180.0

        curve = payload["request"]["energy_curve"]
        if curve == "rising":
            selected.sort(key=lambda item: cast(float, item["planning_energy"]))
        elif curve == "falling":
            selected.sort(key=lambda item: -cast(float, item["planning_energy"]))
        return StructuredModelResult(
            True,
            None,
            {
                "schema_version": MODEL_PLAYLIST_OUTPUT_CONTRACT,
                "ranked_track_ids": [item["track_id"] for item in ranked],
                "selected_track_ids": [item["track_id"] for item in selected],
            },
        )


def test_model_planner_sends_reduced_candidates_and_reconstructs_sources() -> None:
    case = load_evaluation_suite(SUITE_PATH).cases[0]
    tracks, profiles, manual_tags, signals = _case_inputs(case)
    baseline = local_playlist_planner.suggest(
        tracks,
        case.request,
        profiles=profiles,
        manual_tags=manual_tags,
        signal_profiles=signals,
    )
    executor = CapturingExecutor(
        {
            "schema_version": MODEL_PLAYLIST_OUTPUT_CONTRACT,
            "ranked_track_ids": [102, 101],
            "selected_track_ids": [101, 102],
        }
    )
    planner = ModelPlaylistPlanner(executor)

    response = planner.suggest(
        tracks,
        case.request,
        profiles=profiles,
        manual_tags=manual_tags,
        signal_profiles=signals,
    )

    assert len(executor.requests) == 1
    request_payload = json.loads(executor.requests[0].user_prompt)
    assert request_payload["schema_version"] == "assistant-playlist-planner-input/v2"
    assert request_payload["local_plan"]["selected_track_ids"] == [101, 102]
    assert request_payload["candidates"][0]["local_rank"] == 1
    assert executor.requests[0].output_schema_name == (
        "assistant-playlist-planner-response"
    )
    assert executor.requests[0].output_schema is not None
    assert all("path" not in candidate for candidate in request_payload["candidates"])
    assert "untrusted data" in executor.requests[0].system_prompt
    assert "Example JSON shape" in executor.requests[0].system_prompt
    assert response.engine == "model-playlist-planner/v2"
    assert [candidate.track_id for candidate in response.candidates] == [102, 101]
    source_by_id = {track.id: track for track in tracks}
    baseline_by_id = {candidate.track_id: candidate for candidate in baseline.candidates}
    for candidate in response.candidates:
        source = source_by_id[candidate.track_id]
        trusted = baseline_by_id[candidate.track_id]
        assert candidate.path == source.path
        assert candidate.title == source.title
        assert candidate.manual_tags == source.manual_tags
        assert candidate.match_score == trusted.match_score
        assert candidate.reasons == trusted.reasons
    positions = {
        candidate.track_id: candidate.sequence_position
        for candidate in response.candidates
    }
    assert positions == {102: 2, 101: 1}
    assert response.plan.selected_tracks == 2
    assert response.plan.selected_duration_s == 600.0


@pytest.mark.parametrize(
    "payload,error_code",
    [
        (
            {
                "schema_version": MODEL_PLAYLIST_OUTPUT_CONTRACT,
                "ranked_track_ids": [999_999],
                "selected_track_ids": [],
            },
            "model_output_unknown_track",
        ),
        (
            {
                "schema_version": MODEL_PLAYLIST_OUTPUT_CONTRACT,
                "ranked_track_ids": [101, 101],
                "selected_track_ids": [101],
            },
            "model_output_schema_invalid",
        ),
        (
            {
                "schema_version": MODEL_PLAYLIST_OUTPUT_CONTRACT,
                "ranked_track_ids": ["101"],
                "selected_track_ids": [],
            },
            "model_output_schema_invalid",
        ),
    ],
)
def test_model_planner_fails_closed_on_untrusted_ids(
    payload: dict[str, object],
    error_code: str,
) -> None:
    case = load_evaluation_suite(SUITE_PATH).cases[0]
    tracks, profiles, manual_tags, signals = _case_inputs(case)
    planner = ModelPlaylistPlanner(CapturingExecutor(payload))

    with pytest.raises(SuggestionEngineError) as error:
        planner.suggest(
            tracks,
            case.request,
            profiles=profiles,
            manual_tags=manual_tags,
            signal_profiles=signals,
        )

    assert error.value.code == error_code
    if error_code == "model_output_schema_invalid":
        assert error.value.diagnostic is not None
        assert ":" in error.value.diagnostic


def test_model_planner_contains_provider_failure_as_safe_code() -> None:
    case = load_evaluation_suite(SUITE_PATH).cases[0]

    def fail(_request: StructuredModelRequest) -> StructuredModelResult:
        return StructuredModelResult(False, "timeout")

    planner = ModelPlaylistPlanner(fail)

    result = evaluate_playlist_engine(
        planner,
        load_evaluation_suite(SUITE_PATH).model_copy(
            update={"id": "one-model-case", "cases": [case]}
        ),
    )

    assert result.passed is False
    assert result.cases[0].failures == ["engine error: model_execution_timeout"]
    assert result.cases[0].response_fingerprint == ""


def test_model_planner_rejects_explicitly_truncated_output() -> None:
    case = load_evaluation_suite(SUITE_PATH).cases[0]
    tracks, profiles, manual_tags, signals = _case_inputs(case)

    def truncated(_request: StructuredModelRequest) -> StructuredModelResult:
        return StructuredModelResult(
            True,
            None,
            {
                "schema_version": MODEL_PLAYLIST_OUTPUT_CONTRACT,
                "ranked_track_ids": [101],
                "selected_track_ids": [101],
            },
            finish_reason="length",
        )

    with pytest.raises(SuggestionEngineError) as error:
        ModelPlaylistPlanner(truncated).suggest(
            tracks,
            case.request,
            profiles=profiles,
            manual_tags=manual_tags,
            signal_profiles=signals,
        )

    assert error.value.code == "model_output_incomplete"


def test_model_planner_enforces_original_candidate_limit() -> None:
    case = load_evaluation_suite(SUITE_PATH).cases[0]
    extra = case.tracks[-1].model_copy(
        update={
            "id": 106,
            "path": "Forest/Second Morning.flac",
            "title": "Second Morning",
        }
    )
    expanded = case.model_copy(update={"tracks": [*case.tracks, extra]})
    tracks, profiles, manual_tags, signals = _case_inputs(expanded)
    planner = ModelPlaylistPlanner(
        CapturingExecutor(
            {
                "schema_version": MODEL_PLAYLIST_OUTPUT_CONTRACT,
                "ranked_track_ids": [101, 102, 103, 104, 105, 106],
                "selected_track_ids": [101, 102],
            }
        )
    )

    with pytest.raises(SuggestionEngineError) as error:
        planner.suggest(
            tracks,
            case.request,
            profiles=profiles,
            manual_tags=manual_tags,
            signal_profiles=signals,
        )

    assert error.value.code == "model_output_candidate_limit_exceeded"


def test_model_planner_skips_provider_when_local_filter_has_no_candidates() -> None:
    case = load_evaluation_suite(SUITE_PATH).cases[0]
    tracks, profiles, manual_tags, signals = _case_inputs(case)
    request = PlaylistSuggestionRequest(
        prompt="impossible tempo",
        candidate_limit=5,
        min_bpm=900,
        include_unknown_bpm=False,
    )

    def should_not_run(_request: StructuredModelRequest) -> StructuredModelResult:
        raise AssertionError("model executor must not run for an empty candidate pool")

    response = ModelPlaylistPlanner(should_not_run).suggest(
        tracks,
        request,
        profiles=profiles,
        manual_tags=manual_tags,
        signal_profiles=signals,
    )

    assert response.engine == "model-playlist-planner/v2"
    assert response.eligible_tracks == 0
    assert response.candidates == []


def test_reference_model_planner_passes_provider_neutral_suite() -> None:
    suite = load_evaluation_suite(SUITE_PATH)

    result = evaluate_playlist_engine(ModelPlaylistPlanner(ReferenceExecutor()), suite)

    assert result.passed is True
    assert result.engine_id == "model-playlist-planner/v2"
    assert result.summary.passed_cases == 9
    assert all(case.metrics.contract_valid for case in result.cases)
    assert all(case.metrics.deterministic is True for case in result.cases)
