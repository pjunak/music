from __future__ import annotations

from pathlib import Path

import pytest

from app.assistant import voice_analysis
from app.core.config import get_settings


def test_voice_analyzer_is_explicitly_unconfigured_by_default(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(get_settings(), "assistant_voice_model_path", None)

    result = voice_analysis.analyze_voice(Path("unused.wav"))

    assert result.summary == {
        "status": "not_classified",
        "voice_probability": None,
        "vocal_coverage": None,
        "note": (
            "Local voice classification is not enabled. Spectral measurements "
            "are retained, but they are not presented as voice detection."
        ),
    }
    assert result.stage == {"status": "not_configured", "required": False}
    assert voice_analysis.voice_analyzer_signature() is None


def test_voice_analyzer_aggregates_bounded_window_predictions(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    model_path = tmp_path / voice_analysis.VOICE_MODEL_FILENAME
    model_path.write_bytes(b"model fixture")
    monkeypatch.setattr(get_settings(), "assistant_voice_model_path", model_path)
    monkeypatch.setattr(
        voice_analysis,
        "_model_file_hash",
        lambda _path: voice_analysis.VOICE_MODEL_SHA256,
    )
    monkeypatch.setattr(voice_analysis, "_runtime_available", lambda: True)
    monkeypatch.setattr(
        voice_analysis,
        "_run_essentia_model",
        lambda *_args: [[[0.1, 0.9]], [[0.3, 0.7]], [[0.8, 0.2]]],
    )
    cancellation_checks = 0

    def check_cancelled() -> None:
        nonlocal cancellation_checks
        cancellation_checks += 1

    result = voice_analysis.analyze_voice(
        tmp_path / "track.wav",
        check_cancelled=check_cancelled,
    )

    assert result.summary["status"] == "classified"
    assert result.summary["voice_probability"] == 0.6
    assert result.summary["vocal_coverage"] == 0.66667
    assert "Mean normalized voice score 60%" in str(result.summary["note"])
    assert result.stage == {
        "status": "complete",
        "required": False,
        "analyzer_id": voice_analysis.VOICE_ANALYZER_ID,
        "model_sha256": voice_analysis.VOICE_MODEL_SHA256,
        "prediction_windows": 3,
        "classes": ["instrumental", "voice"],
    }
    assert cancellation_checks == 2


def test_voice_analyzer_rejects_an_unpinned_model(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    model_path = tmp_path / "untrusted.pb"
    model_path.write_bytes(b"not the supported model")
    monkeypatch.setattr(get_settings(), "assistant_voice_model_path", model_path)

    result = voice_analysis.analyze_voice(model_path)

    assert result.summary["status"] == "unavailable"
    assert result.summary["voice_probability"] is None
    assert result.stage["reason"] == "unsupported_model"


def test_voice_analyzer_failure_does_not_fail_the_remaining_context(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    model_path = tmp_path / voice_analysis.VOICE_MODEL_FILENAME
    model_path.write_bytes(b"model fixture")
    monkeypatch.setattr(get_settings(), "assistant_voice_model_path", model_path)
    monkeypatch.setattr(
        voice_analysis,
        "_model_file_hash",
        lambda _path: voice_analysis.VOICE_MODEL_SHA256,
    )
    monkeypatch.setattr(voice_analysis, "_runtime_available", lambda: True)

    def fail(*_args: object) -> object:
        raise RuntimeError("private absolute path must not be exposed")

    monkeypatch.setattr(voice_analysis, "_run_essentia_model", fail)

    result = voice_analysis.analyze_voice(tmp_path / "track.wav")

    assert result.summary["status"] == "unavailable"
    assert "private absolute path" not in str(result.summary)
    assert result.stage["reason"] == "inference_failed"
    assert result.stage["error_type"] == "RuntimeError"


def test_voice_signature_is_path_free_and_tracks_runtime_availability(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    model_path = tmp_path / voice_analysis.VOICE_MODEL_FILENAME
    model_path.write_bytes(b"model fixture")
    monkeypatch.setattr(get_settings(), "assistant_voice_model_path", model_path)
    monkeypatch.setattr(voice_analysis, "_runtime_available", lambda: False)

    signature = voice_analysis.voice_analyzer_signature()

    assert signature is not None
    assert str(tmp_path) not in signature
    assert signature.endswith(":runtime-missing")
    monkeypatch.setattr(voice_analysis, "_runtime_available", lambda: True)
    assert voice_analysis.voice_analyzer_signature() != signature
