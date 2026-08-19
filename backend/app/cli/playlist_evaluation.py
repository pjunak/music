from __future__ import annotations

import argparse
from pathlib import Path

from pydantic import ValidationError

from app.assistant.evaluation import evaluate_playlist_engine, load_evaluation_suite
from app.assistant.local import local_playlist_planner


def add_parser(sub: argparse._SubParsersAction) -> None:
    parser = sub.add_parser(
        "evaluate-playlists",
        help="Run a versioned playlist recommendation evaluation suite",
    )
    parser.add_argument("suite", type=Path, help="Path to a playlist-evaluation/v1 JSON suite")
    parser.add_argument(
        "--json",
        action="store_true",
        help="Print the full machine-readable result instead of a summary",
    )
    parser.set_defaults(handler=run)


def run(args: argparse.Namespace) -> int:
    try:
        suite = load_evaluation_suite(args.suite)
    except (OSError, ValidationError) as exc:
        print(f"Could not load evaluation suite: {exc}")
        return 2

    result = evaluate_playlist_engine(local_playlist_planner, suite)
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
