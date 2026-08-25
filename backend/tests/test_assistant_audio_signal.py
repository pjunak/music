import math
import struct
import wave
from pathlib import Path

import pytest

from app.assistant.audio_signal import AudioSignalError, analyze_audio_file


def _write_tone(
    path: Path,
    *,
    frequency_hz: float,
    amplitude: float,
    duration_s: float = 2.0,
    bpm: float | None = None,
    sample_rate: int = 8_000,
) -> None:
    sample_count = round(duration_s * sample_rate)
    pulse_period = None if bpm is None else sample_rate * 60.0 / bpm
    samples: list[int] = []
    for index in range(sample_count):
        envelope = 1.0
        if pulse_period is not None:
            envelope = 1.0 if index % pulse_period < sample_rate * 0.04 else 0.0
        value = amplitude * envelope * math.sin(
            2.0 * math.pi * frequency_hz * index / sample_rate
        )
        samples.append(round(max(-1.0, min(1.0, value)) * 32767.0))
    with wave.open(str(path), "wb") as output:
        output.setnchannels(1)
        output.setsampwidth(2)
        output.setframerate(sample_rate)
        output.writeframes(struct.pack(f"<{len(samples)}h", *samples))


def test_signal_profiles_distinguish_level_and_frequency(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr("app.assistant.audio_signal.shutil.which", lambda _name: None)
    quiet = tmp_path / "quiet.wav"
    loud = tmp_path / "loud.wav"
    bright = tmp_path / "bright.wav"
    _write_tone(quiet, frequency_hz=220.0, amplitude=0.05)
    _write_tone(loud, frequency_hz=220.0, amplitude=0.8)
    _write_tone(bright, frequency_hz=2_500.0, amplitude=0.8)

    quiet_profile = analyze_audio_file(quiet)
    loud_profile = analyze_audio_file(loud)
    bright_profile = analyze_audio_file(bright)

    assert loud_profile.energy > quiet_profile.energy
    assert bright_profile.brightness > loud_profile.brightness
    assert loud_profile.metrics["schema"] == "local-audio/v1"
    assert loud_profile.metrics["sample_rate_hz"] == 8_000
    assert loud_profile.evidence[-1].startswith("Signal proxies do not identify")


def test_signal_profile_estimates_only_a_stable_tempo(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr("app.assistant.audio_signal.shutil.which", lambda _name: None)
    pulsed = tmp_path / "pulsed.wav"
    _write_tone(
        pulsed,
        frequency_hz=440.0,
        amplitude=0.8,
        duration_s=20.0,
        bpm=120.0,
    )

    profile = analyze_audio_file(pulsed)

    assert profile.metrics["tempo_bpm"] == pytest.approx(120.0, abs=1.0)
    assert float(profile.metrics["tempo_confidence"] or 0.0) >= 0.2


def test_signal_profile_reports_decoder_errors(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr("app.assistant.audio_signal.shutil.which", lambda _name: None)
    broken = tmp_path / "broken.wav"
    broken.write_bytes(b"not a wave file")

    with pytest.raises(AudioSignalError, match="WAV decoder could not open"):
        analyze_audio_file(broken)

    unsupported = tmp_path / "track.mp3"
    unsupported.write_bytes(b"not really mp3")
    with pytest.raises(AudioSignalError, match="FFmpeg is unavailable"):
        analyze_audio_file(unsupported)
