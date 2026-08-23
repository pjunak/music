from __future__ import annotations

from collections.abc import Mapping, Sequence
from pathlib import Path

import pytest
from pydantic import ValidationError

from app.assistant.engine import (
    SuggestionEngineError,
    TrackAnalysisProfile,
    TrackLike,
    TrackSignalProfile,
)
from app.assistant.evaluation import (
    PlaylistEvaluationSuite,
    evaluate_playlist_engine,
    load_evaluation_suite,
)
from app.assistant.local import local_playlist_planner
from app.assistant.providers.service import ProviderServiceError
from app.assistant.schemas import PlaylistSuggestionRequest, PlaylistSuggestionResponse
from app.cli import main as cli_main

SUITE_PATH = (
    Path(__file__).parents[1]
    / "app"
    / "assistant"
    / "evaluation_suites"
    / "playlist-local-v1.json"
)


class ReorderedEngine:
    engine_id = "evaluation-reordered/v1"

    def __init__(self, *, alternate: bool = False) -> None:
        self.alternate = alternate
        self.calls = 0

    def suggest(
        self,
        tracks: Sequence[TrackLike],
        request: PlaylistSuggestionRequest,
        profiles: Mapping[int, TrackAnalysisProfile] | None = None,
        manual_tags: Mapping[int, Sequence[str]] | None = None,
        signal_profiles: Mapping[int, TrackSignalProfile] | None = None,
    ) -> PlaylistSuggestionResponse:
        response = local_playlist_planner.suggest(
            tracks,
            request,
            profiles=profiles,
            manual_tags=manual_tags,
            signal_profiles=signal_profiles,
        )
        self.calls += 1
        reverse = not self.alternate or self.calls % 2 == 0
        candidates = list(reversed(response.candidates)) if reverse else response.candidates
        return response.model_copy(
            update={"engine": self.engine_id, "candidates": candidates}
        )


class EquivalentCoreEngine:
    engine_id = "evaluation-equivalent-core/v1"

    def __init__(self) -> None:
        self.calls = 0

    def suggest(
        self,
        tracks: Sequence[TrackLike],
        request: PlaylistSuggestionRequest,
        profiles: Mapping[int, TrackAnalysisProfile] | None = None,
        manual_tags: Mapping[int, Sequence[str]] | None = None,
        signal_profiles: Mapping[int, TrackSignalProfile] | None = None,
    ) -> PlaylistSuggestionResponse:
        response = local_playlist_planner.suggest(
            tracks,
            request,
            profiles=profiles,
            manual_tags=manual_tags,
            signal_profiles=signal_profiles,
        )
        self.calls += 1
        candidates = list(response.candidates)
        if self.calls % 2 == 0:
            candidates = [candidates[1], candidates[0], *reversed(candidates[2:])]
        return response.model_copy(
            update={"engine": self.engine_id, "candidates": candidates}
        )


class ChangedPlaybackSequenceEngine:
    engine_id = "evaluation-changed-playback-sequence/v1"

    def __init__(self) -> None:
        self.calls = 0

    def suggest(
        self,
        tracks: Sequence[TrackLike],
        request: PlaylistSuggestionRequest,
        profiles: Mapping[int, TrackAnalysisProfile] | None = None,
        manual_tags: Mapping[int, Sequence[str]] | None = None,
        signal_profiles: Mapping[int, TrackSignalProfile] | None = None,
    ) -> PlaylistSuggestionResponse:
        response = local_playlist_planner.suggest(
            tracks,
            request,
            profiles=profiles,
            manual_tags=manual_tags,
            signal_profiles=signal_profiles,
        )
        self.calls += 1
        candidates = list(response.candidates)
        if self.calls % 2 == 0:
            selected = [
                candidate for candidate in candidates if candidate.default_selected
            ]
            swapped_positions = {
                selected[0].track_id: selected[1].sequence_position,
                selected[1].track_id: selected[0].sequence_position,
            }
            candidates = [
                candidate.model_copy(
                    update={
                        "sequence_position": swapped_positions.get(
                            candidate.track_id,
                            candidate.sequence_position,
                        )
                    }
                )
                for candidate in candidates
            ]
        return response.model_copy(
            update={"engine": self.engine_id, "candidates": candidates}
        )


class UnknownTrackEngine:
    engine_id = "evaluation-unknown-track/v1"

    def suggest(
        self,
        tracks: Sequence[TrackLike],
        request: PlaylistSuggestionRequest,
        profiles: Mapping[int, TrackAnalysisProfile] | None = None,
        manual_tags: Mapping[int, Sequence[str]] | None = None,
        signal_profiles: Mapping[int, TrackSignalProfile] | None = None,
    ) -> PlaylistSuggestionResponse:
        response = local_playlist_planner.suggest(
            tracks,
            request,
            profiles=profiles,
            manual_tags=manual_tags,
            signal_profiles=signal_profiles,
        )
        candidates = list(response.candidates)
        candidates[0] = candidates[0].model_copy(update={"track_id": 999_999})
        return response.model_copy(
            update={"engine": self.engine_id, "candidates": candidates}
        )


class RaisingEngine:
    engine_id = "evaluation-raising/v1"

    def suggest(
        self,
        tracks: Sequence[TrackLike],
        request: PlaylistSuggestionRequest,
        profiles: Mapping[int, TrackAnalysisProfile] | None = None,
        manual_tags: Mapping[int, Sequence[str]] | None = None,
        signal_profiles: Mapping[int, TrackSignalProfile] | None = None,
    ) -> PlaylistSuggestionResponse:
        raise RuntimeError("provider details stay out of the report")


class UnsafeCodedEngine(RaisingEngine):
    engine_id = "evaluation-unsafe-error/v1"

    def suggest(
        self,
        tracks: Sequence[TrackLike],
        request: PlaylistSuggestionRequest,
        profiles: Mapping[int, TrackAnalysisProfile] | None = None,
        manual_tags: Mapping[int, Sequence[str]] | None = None,
        signal_profiles: Mapping[int, TrackSignalProfile] | None = None,
    ) -> PlaylistSuggestionResponse:
        raise SuggestionEngineError("secret provider detail must not escape")


def one_case_suite(suite: PlaylistEvaluationSuite) -> PlaylistEvaluationSuite:
    case = suite.cases[0]
    repeated_case = case.model_copy(
        update={
            "thresholds": case.thresholds.model_copy(
                update={"require_deterministic": True}
            )
        }
    )
    return suite.model_copy(
        update={"id": "single-evaluation-case", "cases": [repeated_case]}
    )


def test_checked_in_playlist_evaluation_suite_passes() -> None:
    suite = load_evaluation_suite(SUITE_PATH)

    result = evaluate_playlist_engine(local_playlist_planner, suite)

    assert result.passed is True
    assert result.engine_id == "local-planner/v2"
    assert result.summary.cases == 9
    assert result.summary.passed_cases == 9
    assert result.summary.mean_precision_at_k == 1.0
    assert result.summary.mean_recall_at_k == 1.0
    assert result.summary.mean_order_pair_accuracy == 1.0
    assert all(case.metrics.contract_valid for case in result.cases)
    repeated_case_ids = {
        "manual-temple-tag-priority",
        "heroic-ritual-arc",
        "untrusted-candidate-text-limit",
    }
    assert {
        case.id
        for case in result.cases
        if case.metrics.deterministic is not None
    } == repeated_case_ids
    assert all(
        case.metrics.deterministic is True
        for case in result.cases
        if case.id in repeated_case_ids
    )


def test_evaluator_reports_ranking_regressions() -> None:
    suite = one_case_suite(load_evaluation_suite(SUITE_PATH))

    result = evaluate_playlist_engine(ReorderedEngine(), suite)

    assert result.passed is False
    assert result.summary.failed_cases == 1
    assert result.cases[0].metrics.precision_at_k == 0.0
    assert result.cases[0].metrics.contract_valid is True
    assert "precision_at_k below threshold" in result.cases[0].failures
    assert "recall_at_k below threshold" in result.cases[0].failures


def test_evaluator_checks_repeated_response_quality_and_top_candidate_set() -> None:
    suite = one_case_suite(load_evaluation_suite(SUITE_PATH))

    result = evaluate_playlist_engine(ReorderedEngine(alternate=True), suite)

    assert result.passed is False
    assert result.cases[0].metrics.deterministic is False
    assert result.cases[0].exact_response_match is False
    assert result.cases[0].repeated_top_track_ids == [103, 104]
    assert "repeated response precision_at_k below threshold" in result.cases[0].failures
    assert (
        "repeated response changed the top candidate set: "
        "first [101, 102], repeated [103, 104]"
    ) in result.cases[0].failures


def test_evaluator_accepts_equivalent_core_with_incidental_order_changes() -> None:
    suite = one_case_suite(load_evaluation_suite(SUITE_PATH))

    result = evaluate_playlist_engine(EquivalentCoreEngine(), suite)

    assert result.passed is True
    assert result.cases[0].metrics.deterministic is True
    assert result.cases[0].exact_response_match is False
    assert result.cases[0].top_track_ids == [101, 102]
    assert result.cases[0].repeated_top_track_ids == [102, 101]
    assert result.cases[0].selected_track_ids == [101, 102]
    assert result.cases[0].repeated_selected_track_ids == [101, 102]
    assert (
        result.cases[0].response_fingerprint
        != result.cases[0].repeated_response_fingerprint
    )
    assert result.cases[0].failures == []


def test_evaluator_rejects_changed_selected_playback_sequence() -> None:
    suite = one_case_suite(load_evaluation_suite(SUITE_PATH))

    result = evaluate_playlist_engine(ChangedPlaybackSequenceEngine(), suite)

    assert result.passed is False
    assert result.cases[0].metrics.deterministic is False
    assert result.cases[0].top_track_ids == result.cases[0].repeated_top_track_ids
    assert result.cases[0].selected_track_ids == [101, 102]
    assert result.cases[0].repeated_selected_track_ids == [102, 101]
    assert result.cases[0].failures == [
        "repeated response changed the selected playback sequence: "
        "first [101, 102], repeated [102, 101]"
    ]


def test_evaluator_rejects_unknown_model_track_ids() -> None:
    suite = one_case_suite(load_evaluation_suite(SUITE_PATH))

    result = evaluate_playlist_engine(UnknownTrackEngine(), suite)

    assert result.passed is False
    assert result.cases[0].metrics.unknown_candidate_count == 1
    assert result.cases[0].metrics.contract_valid is False
    assert "suggestion response violates the evaluation contract" in result.cases[0].failures


def test_evaluator_contains_engine_errors_per_case() -> None:
    suite = one_case_suite(load_evaluation_suite(SUITE_PATH))

    result = evaluate_playlist_engine(RaisingEngine(), suite)

    assert result.passed is False
    assert result.cases[0].metrics.contract_valid is False
    assert result.cases[0].failures == ["engine raised RuntimeError"]
    assert result.cases[0].response_fingerprint == ""


def test_evaluator_sanitizes_declared_engine_error_codes() -> None:
    suite = one_case_suite(load_evaluation_suite(SUITE_PATH))

    result = evaluate_playlist_engine(UnsafeCodedEngine(), suite)

    assert result.cases[0].failures == ["engine error: engine_failure"]
    assert "secret provider detail" not in result.model_dump_json()


def test_suite_validation_rejects_unknown_expectation_track() -> None:
    payload = load_evaluation_suite(SUITE_PATH).model_dump(mode="json")
    payload["cases"][0]["expectations"]["relevant_track_ids"] = [999_999]

    with pytest.raises(ValidationError, match="unknown track IDs"):
        PlaylistEvaluationSuite.model_validate(payload)


def test_playlist_evaluation_cli_supports_human_and_json_output(
    capsys: pytest.CaptureFixture[str],
) -> None:
    assert cli_main(["evaluate-playlists", str(SUITE_PATH)]) == 0
    human = capsys.readouterr().out
    assert "PASS local-dnd-playlist-baseline-v4" in human
    assert "9/9 cases passed" in human

    assert cli_main(["evaluate-playlists", str(SUITE_PATH), "--json"]) == 0
    output = capsys.readouterr().out
    assert '"schema_version": "playlist-evaluation-result/v1"' in output
    assert '"engine_id": "local-planner/v2"' in output


def test_playlist_evaluation_cli_rejects_invalid_suite(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    invalid = tmp_path / "invalid.json"
    invalid.write_text('{"schema_version":"wrong"}', encoding="utf-8")

    assert cli_main(["evaluate-playlists", str(invalid)]) == 2
    assert "Could not load evaluation suite" in capsys.readouterr().out


def test_configured_model_cli_requires_explicit_suite_disclosure(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    prepared = False

    def should_not_prepare() -> ReorderedEngine:
        nonlocal prepared
        prepared = True
        return ReorderedEngine()

    monkeypatch.setattr(
        "app.cli.playlist_evaluation._configured_model_engine",
        should_not_prepare,
    )

    result = cli_main(
        [
            "evaluate-playlists",
            str(SUITE_PATH),
            "--engine",
            "configured-model",
        ]
    )

    assert result == 2
    assert prepared is False
    assert "requires --send-suite-to-provider" in capsys.readouterr().out


def test_configured_model_cli_reports_safe_role_preparation_error(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    def fail() -> ReorderedEngine:
        raise ProviderServiceError("role_not_enabled", "private detail", 409)

    monkeypatch.setattr(
        "app.cli.playlist_evaluation._configured_model_engine",
        fail,
    )

    result = cli_main(
        [
            "evaluate-playlists",
            str(SUITE_PATH),
            "--engine",
            "configured-model",
            "--send-suite-to-provider",
        ]
    )

    assert result == 2
    output = capsys.readouterr().out
    assert "role_not_enabled" in output
    assert "private detail" not in output
