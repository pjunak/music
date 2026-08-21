from app.assistant.evaluation import load_evaluation_suite
from app.assistant.model_eq import load_eq_quality_suite
from app.assistant.model_evaluation import (
    EQ_QUALITY_EVALUATION,
    PLAYLIST_QUALITY_EVALUATION,
    TAG_CLEANUP_QUALITY_EVALUATION,
    TAGGING_QUALITY_EVALUATION,
    bundled_evaluation_suite_paths,
)
from app.assistant.model_tag_cleanup import load_tag_cleanup_quality_suite
from app.assistant.model_tagger import load_tag_quality_suite


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
