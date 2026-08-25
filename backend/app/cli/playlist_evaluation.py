import argparse
from functools import partial
from pathlib import Path

from pydantic import ValidationError

from app.assistant.engine import PlaylistSuggestionEngine
from app.assistant.evaluation import evaluate_playlist_engine, load_evaluation_suite
from app.assistant.local import local_playlist_planner
from app.assistant.model_playlist import ModelPlaylistPlanner
from app.assistant.providers.execution import execute_structured_model_request
from app.assistant.providers.service import ProviderServiceError, prepare_role_execution
from app.core.db import SessionLocal


def add_parser(sub: argparse._SubParsersAction) -> None:
    parser = sub.add_parser(
        "evaluate-playlists",
        help="Run a versioned playlist recommendation evaluation suite",
    )
    parser.add_argument("suite", type=Path, help="Path to a playlist-evaluation/v1 JSON suite")
    parser.add_argument(
        "--engine",
        choices=("local", "configured-model"),
        default="local",
        help="Planner to evaluate (default: local)",
    )
    parser.add_argument(
        "--send-suite-to-provider",
        action="store_true",
        help=(
            "Confirm that the suite request, synthetic titles, tags, and evidence may "
            "be sent to the configured playlist-planner provider"
        ),
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Print the full machine-readable result instead of a summary",
    )
    parser.set_defaults(handler=run)


def _configured_model_engine() -> PlaylistSuggestionEngine:
    with SessionLocal() as db:
        target = prepare_role_execution(db, "playlist_planner")
    return ModelPlaylistPlanner(
        partial(execute_structured_model_request, target)
    )


def run(args: argparse.Namespace) -> int:
    try:
        suite = load_evaluation_suite(args.suite)
    except (OSError, ValidationError) as exc:
        print(f"Could not load evaluation suite: {exc}")
        return 2

    engine: PlaylistSuggestionEngine = local_playlist_planner
    if args.engine == "configured-model":
        if not args.send_suite_to_provider:
            print(
                "Configured-model evaluation requires --send-suite-to-provider. "
                "Only run it with a synthetic suite you are willing to disclose."
            )
            return 2
        try:
            engine = _configured_model_engine()
        except ProviderServiceError as exc:
            print(f"Could not prepare configured playlist model ({exc.code}).")
            return 2

    result = evaluate_playlist_engine(engine, suite)
    if args.json:
        print(result.model_dump_json(indent=2))
    else:
        status = "PASS" if result.passed else "FAIL"
        print(
            f"{status} {result.suite_id} with {result.engine_id}: "
            f"{result.summary.passed_cases}/{result.summary.cases} cases passed"
        )
        for case in result.cases:
            case_status = "PASS" if case.passed else "FAIL"
            metrics = case.metrics
            print(
                f"  {case_status} {case.id}: precision={metrics.precision_at_k:.2f}, "
                f"recall={metrics.recall_at_k:.2f}, rr={metrics.reciprocal_rank:.2f}, "
                f"reasons={metrics.reason_coverage:.2f}"
            )
            for failure in case.failures:
                print(f"    - {failure}")
    return 0 if result.passed else 1
