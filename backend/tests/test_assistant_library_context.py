from __future__ import annotations

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

from app.assistant.audio_context import AudioContextDocument, analyze_audio_context
from app.assistant.library_context import LOCAL_CONTEXT_ANALYZER_ID
from app.assistant.voice_analysis import VoiceAnalysis
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
            "No calibrated local voice classifier is installed. Spectral measurements "
            "are retained, but they are not presented as voice detection."
        ),
    }
    assert "tags" not in document.summary
    assert "moods" not in document.summary


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
