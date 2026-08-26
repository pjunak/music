import subprocess
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
    assert voice_analysis.voice_analyzer_status()["status"] == "not_configured"


def test_voice_analyzer_status_reports_a_missing_deployment_model(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        get_settings(),
        "assistant_voice_model_path",
        tmp_path / voice_analysis.VOICE_MODEL_FILENAME,
    )

    status = voice_analysis.voice_analyzer_status()

    assert status == {
        "analyzer_id": voice_analysis.VOICE_ANALYZER_ID,
        "status": "unavailable",
        "reason": "model_missing",
        "model_filename": voice_analysis.VOICE_MODEL_FILENAME,
        "model_sha256": voice_analysis.VOICE_MODEL_SHA256,
    }


def test_voice_runtime_preflight_uses_a_disposable_interpreter(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[tuple[list[str], dict[str, object]]] = []

    def run_probe(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[bytes]:
        calls.append((command, kwargs))
        return subprocess.CompletedProcess(command, 0)

    voice_analysis._runtime_available.cache_clear()
    monkeypatch.setattr(voice_analysis.subprocess, "run", run_probe)
    try:
        assert voice_analysis._runtime_available() is True
        assert voice_analysis._runtime_available() is True
    finally:
        voice_analysis._runtime_available.cache_clear()

    assert len(calls) == 1
    command, kwargs = calls[0]
    assert command == [
        voice_analysis.sys.executable,
        "-c",
        voice_analysis._RUNTIME_PROBE_CODE,
    ]
    assert kwargs == {
        "check": False,
        "stdin": subprocess.DEVNULL,
        "stdout": subprocess.DEVNULL,
        "stderr": subprocess.DEVNULL,
        "timeout": voice_analysis._RUNTIME_PROBE_TIMEOUT_SECONDS,
    }


def test_voice_runtime_preflight_treats_probe_timeout_as_unavailable(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def time_out(*_args: object, **_kwargs: object) -> subprocess.CompletedProcess[bytes]:
        raise subprocess.TimeoutExpired("python", 30)

    voice_analysis._runtime_available.cache_clear()
    monkeypatch.setattr(voice_analysis.subprocess, "run", time_out)
    try:
        assert voice_analysis._runtime_available() is False
    finally:
        voice_analysis._runtime_available.cache_clear()


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

    def unexpected_runtime_probe() -> bool:
        raise AssertionError("analysis workers must import the runtime only for inference")

    monkeypatch.setattr(voice_analysis, "_runtime_available", unexpected_runtime_probe)
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


def test_voice_analyzer_reports_a_worker_import_failure_as_runtime_missing(
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

    def fail(*_args: object) -> object:
        try:
            raise ImportError("native runtime is unavailable")
        except ImportError as exc:
            raise voice_analysis._VoiceRuntimeUnavailable from exc

    monkeypatch.setattr(voice_analysis, "_run_essentia_model", fail)

    result = voice_analysis.analyze_voice(tmp_path / "track.wav")

    assert result.summary["status"] == "unavailable"
    assert result.stage["reason"] == "runtime_missing"
    assert result.stage["error_type"] == "ImportError"


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
