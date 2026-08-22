import pytest
from pydantic import ValidationError

from app.assistant.evaluation import load_evaluation_suite
from app.assistant.model_eq import EqQualitySuite, load_eq_quality_suite
from app.assistant.model_evaluation import (
    EQ_QUALITY_EVALUATION,
    PLAYLIST_QUALITY_EVALUATION,
    TAG_CLEANUP_QUALITY_EVALUATION,
    TAGGING_QUALITY_EVALUATION,
    bundled_evaluation_suite_paths,
)
from app.assistant.model_tag_cleanup import (
    TagCleanupQualitySuite,
    load_tag_cleanup_quality_suite,
)
from app.assistant.model_tagger import TagQualitySuite, load_tag_quality_suite


def test_bundled_model_quality_suites_exist_and_match_definitions() -> None:
    paths = bundled_evaluation_suite_paths()

    assert {path.name for path in paths} == {
        "eq-assistant-v1.json",
        "music-tagging-v1.json",
        "playlist-local-v1.json",
        "tag-cleanup-v1.json",
    }
    assert all(path.is_file() for path in paths)
    assert (
        load_evaluation_suite(PLAYLIST_QUALITY_EVALUATION.suite_path).id
        == PLAYLIST_QUALITY_EVALUATION.suite_id
    )
    assert (
        load_tag_quality_suite(TAGGING_QUALITY_EVALUATION.suite_path).id
        == TAGGING_QUALITY_EVALUATION.suite_id
    )
    assert (
        load_tag_cleanup_quality_suite(TAG_CLEANUP_QUALITY_EVALUATION.suite_path).id
        == TAG_CLEANUP_QUALITY_EVALUATION.suite_id
    )
    assert (
        load_eq_quality_suite(EQ_QUALITY_EVALUATION.suite_path).id
        == EQ_QUALITY_EVALUATION.suite_id
    )


def test_tagging_quality_suite_rejects_duplicate_case_ids() -> None:
    payload = load_tag_quality_suite(
        TAGGING_QUALITY_EVALUATION.suite_path
    ).model_dump(mode="json")
    payload["cases"].append(payload["cases"][0])

    with pytest.raises(ValidationError, match="case IDs must be unique"):
        TagQualitySuite.model_validate(payload)


def test_cleanup_quality_suite_rejects_unknown_expected_sources() -> None:
    payload = load_tag_cleanup_quality_suite(
        TAG_CLEANUP_QUALITY_EVALUATION.suite_path
    ).model_dump(mode="json")
    payload["cases"][0]["required_pairs"][0]["source"] = "missing source"

    with pytest.raises(ValidationError, match="unknown sources"):
        TagCleanupQualitySuite.model_validate(payload)


def test_eq_quality_suite_rejects_inverted_gain_ranges() -> None:
    payload = load_eq_quality_suite(EQ_QUALITY_EVALUATION.suite_path).model_dump(
        mode="json"
    )
    payload["cases"][0]["expectations"][0].update(
        minimum_gain_db=2.0,
        maximum_gain_db=-2.0,
    )

    with pytest.raises(ValidationError, match="minimum cannot exceed"):
        EqQualitySuite.model_validate(payload)
