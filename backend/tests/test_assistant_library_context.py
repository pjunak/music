import math
import struct
import time
import wave
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from fastapi.testclient import TestClient
from sqlalchemy import delete

from app.assistant.audio_context import (
    AudioContextDocument,
    AudioContextPerformance,
    _estimate_tempo,
    _Frame,
    _spectrum,
    _timeline_frames,
    _trajectory,
    analyze_audio_context,
)
from app.assistant.context_workers import ContextAnalysisResult
from app.assistant.library_context import LOCAL_CONTEXT_ANALYZER_ID
from app.assistant.voice_analysis import VoiceAnalysis
from app.core.config import get_settings
from app.core.db import SessionLocal
from app.models.track_analysis_failure import TrackAnalysisFailure
from app.models.track_context import TrackContext


@pytest.fixture(autouse=True)
def _clean_context_state() -> Iterator[None]:
    def clean() -> None:
        with SessionLocal() as db:
            db.execute(delete(TrackContext))
            db.execute(
                delete(TrackAnalysisFailure).where(
                    TrackAnalysisFailure.analyzer_id == LOCAL_CONTEXT_ANALYZER_ID
                )
            )
            db.commit()

    clean()
    yield
    clean()


def _wait_for_job(client: TestClient, job_id: str) -> dict[str, Any]:
    deadline = time.monotonic() + 5
    latest: dict[str, Any] = {}
    while time.monotonic() < deadline:
        response = client.get(f"/api/jobs/{job_id}")
        assert response.status_code == 200, response.text
        latest = response.json()
        if latest["status"] in {"succeeded", "failed", "cancelled"}:
            return latest
        time.sleep(0.02)
    raise AssertionError(f"context job did not finish; latest={latest}")


def _document() -> AudioContextDocument:
    trajectory = {
        "typical": 0.55,
        "low": 0.2,
        "high": 0.88,
        "range": 0.68,
        "variability": 0.4,
        "slope": 0.5,
        "start": 0.25,
        "end": 0.82,
        "peak_at_fraction": 0.9,
        "high_fraction": 0.25,
        "shape": "gradual_rise",
    }
    return AudioContextDocument(
        confidence="high",
        completeness="full",
        summary={
            "schema_version": LOCAL_CONTEXT_ANALYZER_ID,
            "duration_s": 120.0,
            "confidence": "high",
            "trajectories": {
                key: trajectory
                for key in (
                    "loudness",
                    "intensity",
                    "rhythmic_drive",
                    "brightness",
                    "density",
                    "spectral_flux",
                )
            },
            "tempo": {
                "status": "unresolved",
                "typical_bpm": None,
                "low_bpm": None,
                "high_bpm": None,
                "variability": None,
                "points": [],
            },
            "structure": {
                "section_count": 1,
                "major_change_count": 0,
                "repeated_section_count": 0,
                "development": "continuous",
            },
            "voice": {
                "status": "not_classified",
                "voice_probability": None,
                "vocal_coverage": None,
                "note": "No classifier configured.",
            },
            "evidence": ["Intensity rises across the track."],
        },
        timeline=[{"start_s": 0.0, "duration_s": 2.0, "intensity": 0.25}],
        sections=[
            {
                "id": "s1",
                "start_fraction": 0.0,
                "end_fraction": 1.0,
                "intensity": 0.55,
                "rhythmic_drive": 0.55,
                "brightness": 0.55,
                "density": 0.55,
                "tempo_bpm": None,
                "tempo_confidence": 0.0,
                "changes_from_previous": [],
                "repeats_section_ids": [],
            }
        ],
        technical={"probe_status": "unavailable"},
        stages={"voice": {"status": "not_configured", "required": False}},
        performance=AudioContextPerformance(
            audio_seconds=120.0,
            elapsed_seconds=6.0,
            stage_seconds={
                "probe": 0.1,
                "decode_and_frames": 1.0,
                "spectrum": 2.0,
                "feature_summary": 0.5,
                "voice": 1.0,
                "ebu_loudness": 1.0,
                "finalize": 0.4,
            },
        ),
    )


def test_library_context_endpoints_require_authentication(client: TestClient) -> None:
    assert client.get("/api/assistant/library-context/summary").status_code == 401
    assert client.get("/api/assistant/library-context/tracks/1").status_code == 401
    assert (
        client.post(
            "/api/assistant/library-context/jobs",
            json={"force": False, "scope": {"type": "all"}},
        ).status_code
        == 401
    )


def test_context_job_is_scoped_checkpointed_and_browsable(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
    seeded_track_id: int,
) -> None:
    calls = 0

    def analyze(_path: Path, *, check_cancelled=None) -> AudioContextDocument:  # type: ignore[no-untyped-def]
        nonlocal calls
        calls += 1
        if check_cancelled is not None:
            check_cancelled()
        return _document()

    monkeypatch.setattr("app.assistant.library_context.analyze_audio_context", analyze)
    payload = {
        "force": False,
        "scope": {"type": "tracks", "track_ids": [seeded_track_id]},
    }
    started = auth_client.post("/api/assistant/library-context/jobs", json=payload)
    assert started.status_code == 202, started.text
    first = _wait_for_job(auth_client, started.json()["id"])

    assert first["status"] == "succeeded", first
    assert first["result"]["updated"] == 1
    assert first["result"]["current_contexts"] == 1
    assert calls == 1

    detail = auth_client.get(
        f"/api/assistant/library-context/tracks/{seeded_track_id}"
    )
    assert detail.status_code == 200, detail.text
    assert detail.json()["status"] == "full"
    assert detail.json()["summary"]["trajectories"]["intensity"]["shape"] == (
        "gradual_rise"
    )
    assert "path" not in detail.json()

    unchanged = auth_client.post("/api/assistant/library-context/jobs", json=payload)
    assert unchanged.status_code == 202, unchanged.text
    second = _wait_for_job(auth_client, unchanged.json()["id"])
    assert second["result"]["updated"] == 0
    assert second["result"]["unchanged"] == 1
    assert calls == 1

    forced = auth_client.post(
        "/api/assistant/library-context/jobs",
        json={**payload, "force": True},
    )
    assert forced.status_code == 202, forced.text
    rebuilt = _wait_for_job(auth_client, forced.json()["id"])
    assert rebuilt["result"]["updated"] == 1
    assert calls == 2

    summary = auth_client.get("/api/assistant/library-context/summary")
    assert summary.status_code == 200, summary.text
    assert summary.json()["full_tracks"] == 1
    assert summary.json()["analyzer"] == LOCAL_CONTEXT_ANALYZER_ID


def test_context_job_uses_configured_process_workers_and_parent_checkpoints(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
    seeded_track_id: int,
    extra_seeded_track_ids: list[int],
) -> None:
    track_ids = [seeded_track_id, *extra_seeded_track_ids]
    captured: dict[str, object] = {}

    def analyze_in_processes(tasks, *, max_workers, check_cancelled):  # type: ignore[no-untyped-def]
        captured["track_ids"] = [task.track_id for task in tasks]
        captured["max_workers"] = max_workers
        check_cancelled()
        for task in reversed(tasks):
            yield ContextAnalysisResult(track_id=task.track_id, document=_document())

    monkeypatch.setattr(get_settings(), "assistant_library_context_workers", 3)
    monkeypatch.setattr(
        "app.assistant.library_context.analyze_tracks_in_processes",
        analyze_in_processes,
    )

    started = auth_client.post(
        "/api/assistant/library-context/jobs",
        json={
            "force": True,
            "scope": {"type": "tracks", "track_ids": track_ids},
        },
    )
    finished = _wait_for_job(auth_client, started.json()["id"])

    assert finished["status"] == "succeeded", finished
    assert finished["result"]["analysis_workers"] == 3
    assert finished["result"]["updated"] == 4
    assert finished["result"]["performance"]["tracks_profiled"] == 4
    assert finished["result"]["performance"]["audio_seconds"] == 480.0
    assert finished["result"]["performance"]["worker_seconds"] == 24.0
    assert finished["result"]["performance"]["dominant_stage"] == "spectrum"
    assert finished["result"]["performance"]["stage_seconds"]["spectrum"] == 8.0
    assert captured == {"track_ids": track_ids, "max_workers": 3}

    with SessionLocal() as db:
        rows = db.query(TrackContext).filter(TrackContext.track_id.in_(track_ids)).all()
    assert len(rows) == 4


def test_context_job_checkpoints_failures_and_retries(
    auth_client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
    seeded_track_id: int,
) -> None:
    from app.assistant.audio_signal import AudioSignalError

    monkeypatch.setattr(
        "app.assistant.library_context.analyze_audio_context",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            AudioSignalError("deliberate context failure")
        ),
    )
    payload = {
        "force": False,
        "scope": {"type": "tracks", "track_ids": [seeded_track_id]},
    }
    started = auth_client.post("/api/assistant/library-context/jobs", json=payload)
    failed = _wait_for_job(auth_client, started.json()["id"])
    assert failed["status"] == "succeeded"
    assert failed["result"]["failed"] == 1
    assert "deliberate context failure" in failed["result"]["failure_samples"][0][
        "error"
    ]

    monkeypatch.setattr(
        "app.assistant.library_context.analyze_audio_context",
        lambda *_args, **_kwargs: _document(),
    )
    retried = auth_client.post("/api/assistant/library-context/jobs", json=payload)
    finished = _wait_for_job(auth_client, retried.json()["id"])
    assert finished["result"]["updated"] == 1
    assert finished["result"]["failed"] == 0


def _write_developing_tone(path: Path, duration_s: float = 12.0) -> None:
    sample_rate = 16_000
    samples: list[int] = []
    for index in range(round(duration_s * sample_rate)):
        fraction = index / (duration_s * sample_rate)
        amplitude = 0.04 if fraction < 0.45 else 0.78
        pulse = 1.0
        if fraction >= 0.45:
            pulse = 1.0 if index % 8_000 < 1_000 else 0.25
        value = amplitude * pulse * math.sin(2.0 * math.pi * 440.0 * index / sample_rate)
        samples.append(round(max(-1.0, min(1.0, value)) * 32767.0))
    with wave.open(str(path), "wb") as output:
        output.setnchannels(1)
        output.setsampwidth(2)
        output.setframerate(sample_rate)
        output.writeframes(struct.pack(f"<{len(samples)}h", *samples))


def test_audio_context_describes_development_without_suggesting_tags(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source = tmp_path / "developing.wav"
    _write_developing_tone(source)
    monkeypatch.setattr("app.assistant.audio_context.shutil.which", lambda _name: None)

    document = analyze_audio_context(source)
    trajectories = document.summary["trajectories"]
    assert isinstance(trajectories, dict)
    intensity = trajectories["intensity"]
    assert isinstance(intensity, dict)
    assert float(intensity["end"]) > float(intensity["start"]) + 0.25
    assert float(intensity["variability"]) > 0.1
    assert document.summary["voice"] == {
        "status": "not_classified",
        "voice_probability": None,
        "vocal_coverage": None,
        "note": (
            "Local voice classification is not enabled. Spectral measurements "
            "are retained, but they are not presented as voice detection."
        ),
    }
    assert "tags" not in document.summary
    assert "moods" not in document.summary
    assert document.stages["spectrum"] == {
        "status": "complete",
        "fft_size": 2_048,
        "bands": 24,
        "implementation": "numpy-rfft+mel-profile/v2",
    }
    assert document.performance is not None
    assert document.performance.audio_seconds == pytest.approx(12.0)
    assert document.performance.elapsed_seconds > 0.0
    assert set(document.performance.stage_seconds) == {
        "probe",
        "decode_and_frames",
        "spectrum",
        "feature_summary",
        "voice",
        "ebu_loudness",
        "finalize",
    }


def test_numpy_spectrum_has_a_deterministic_v2_measurement_baseline() -> None:
    samples = [
        0.55 * math.sin(2.0 * math.pi * 440.0 * index / 16_000)
        + 0.2 * math.sin(2.0 * math.pi * 1_700.0 * index / 16_000)
        for index in range(8_000)
    ]

    spectrum = _spectrum(samples, 16_000)

    assert spectrum.centroid_hz == pytest.approx(777.1830341514549, rel=1e-11)
    assert spectrum.bandwidth_hz == pytest.approx(558.0481984202627, rel=1e-11)
    assert spectrum.rolloff_hz == 1_695.3125
    assert spectrum.flatness == pytest.approx(7.769654422762687e-14, rel=1e-8)
    assert spectrum.bass_ratio == pytest.approx(7.36736002722439e-10, rel=1e-8)
    assert spectrum.mid_ratio == pytest.approx(0.999999999247039, rel=1e-11)
    assert spectrum.high_ratio == pytest.approx(1.605474666777969e-11, rel=1e-8)
    assert spectrum.peak_concentration == pytest.approx(0.9998252798468766, rel=1e-11)
    assert spectrum.spectral_entropy == pytest.approx(0.1135929609507341, rel=1e-11)
    assert spectrum.band_coverage == pytest.approx(2 / 24)
    assert len(spectrum.profile) == 24


def _tone_frame(frequency_hz: float, *, start_s: float = 0.0) -> _Frame:
    samples = [
        0.5 * math.sin(2.0 * math.pi * frequency_hz * index / 16_000)
        for index in range(8_000)
    ]
    return _Frame(
        start_s=start_s,
        duration_s=0.5,
        loudness_dbfs=-10.0,
        peak_dbfs=-6.0,
        zero_crossing_rate=0.0,
        difference_ratio=0.0,
        spectrum=_spectrum(samples, 16_000),
    )


def test_v2_brightness_uses_a_perceptual_range_instead_of_a_hard_6khz_divisor() -> None:
    frames = [_tone_frame(frequency, start_s=index * 0.5) for index, frequency in enumerate((500, 1_500, 4_000))]

    rows = _timeline_frames(frames, [0.5] * 30)

    assert rows[0]["brightness"] == pytest.approx(0.195, abs=0.01)
    assert rows[1]["brightness"] == pytest.approx(0.55, abs=0.01)
    assert rows[2]["brightness"] == pytest.approx(0.937, abs=0.01)


def test_v2_spectral_change_detects_transitions_inside_the_old_mid_band() -> None:
    rows = _timeline_frames(
        [_tone_frame(500.0), _tone_frame(1_500.0, start_s=0.5)],
        [0.5] * 20,
    )

    assert rows[1]["spectral_flux"] > 0.9


def test_v2_tempo_resolves_accented_120_bpm_without_gain_dependence() -> None:
    levels = [
        (1.0 if (index // 10) % 2 == 0 else 0.35) if index % 10 == 0 else 0.05
        for index in range(600)
    ]

    tempo, confidence = _estimate_tempo(levels, 20.0)
    quieter_tempo, quieter_confidence = _estimate_tempo(
        [value * 0.1 for value in levels],
        20.0,
    )

    assert tempo == pytest.approx(120.0)
    assert quieter_tempo == pytest.approx(tempo)
    assert quieter_confidence == pytest.approx(confidence)
    assert 0.4 < confidence < 0.8


def test_v2_high_fraction_uses_an_absolute_level() -> None:
    low = _trajectory([0.2] * 100)
    high = _trajectory([0.8] * 100)
    ramp = _trajectory([index / 99 for index in range(100)])

    assert low["high_fraction"] == 0.0
    assert high["high_fraction"] == 1.0
    assert float(ramp["high_fraction"]) == pytest.approx(0.34)


def test_audio_context_includes_optional_voice_classifier_evidence(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source = tmp_path / "developing.wav"
    _write_developing_tone(source)
    monkeypatch.setattr("app.assistant.audio_context.shutil.which", lambda _name: None)
    monkeypatch.setattr(
        "app.assistant.audio_context.analyze_voice",
        lambda *_args, **_kwargs: VoiceAnalysis(
            summary={
                "status": "classified",
                "voice_probability": 0.82,
                "vocal_coverage": 0.75,
                "note": "Voice is present across most analyzed windows.",
            },
            stage={
                "status": "complete",
                "required": False,
                "analyzer_id": "essentia-musicnn-voice/v1",
            },
        ),
    )

    document = analyze_audio_context(source)

    assert document.summary["voice"] == {
        "status": "classified",
        "voice_probability": 0.82,
        "vocal_coverage": 0.75,
        "note": "Voice is present across most analyzed windows.",
    }
    assert document.stages["voice"] == {
        "status": "complete",
        "required": False,
        "analyzer_id": "essentia-musicnn-voice/v1",
    }


def test_context_source_signature_changes_with_voice_analyzer(
    db_session,  # type: ignore[no-untyped-def]
    seeded_track_id: int,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from app.assistant import library_context
    from app.models.track import Track

    track = db_session.get(Track, seeded_track_id)
    assert track is not None
    monkeypatch.setattr(library_context, "voice_analyzer_signature", lambda: None)
    without_voice = library_context.context_source_signature(track)
    monkeypatch.setattr(
        library_context,
        "voice_analyzer_signature",
        lambda: "essentia-musicnn-voice/v1:model:runtime-present",
    )

    assert library_context.context_source_signature(track) != without_voice


def test_context_source_signature_changes_with_implementation(
    db_session,  # type: ignore[no-untyped-def]
    seeded_track_id: int,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from app.assistant import library_context
    from app.models.track import Track

    track = db_session.get(Track, seeded_track_id)
    assert track is not None
    current = library_context.context_source_signature(track)
    monkeypatch.setattr(
        library_context,
        "CONTEXT_IMPLEMENTATION_ID",
        "local-context/v2+different-implementation/v1",
    )

    assert library_context.context_source_signature(track) != current
