"""Local, provider-neutral measurements derived from decoded audio samples.

This module deliberately reports signal measurements and conservative proxies.
It does not claim to recognize instruments, genres, scenes, or moods. Those
semantic decisions remain a separate, reviewable layer.
"""

from __future__ import annotations

import math
import shutil
import subprocess
import sys
import threading
import time
import wave
from array import array
from collections.abc import Callable, Iterator, Sequence
from contextlib import contextmanager
from dataclasses import dataclass
from itertools import pairwise
from pathlib import Path

_TARGET_SAMPLE_RATE = 8_000
_FRAMES_PER_CHUNK = 8_192
_FFMPEG_ERROR_LIMIT = 16_384
_MIN_ANALYZABLE_SECONDS = 0.1


class AudioSignalError(RuntimeError):
    """A track could not be decoded into a usable signal."""


@dataclass(frozen=True)
class AudioSignalMeasurements:
    duration_s: float
    sample_rate_hz: int
    rms_dbfs: float
    peak_dbfs: float
    level_spread_db: float
    activity_ratio: float
    zero_crossing_rate: float
    high_frequency_ratio: float
    onset_rate_hz: float
    tempo_bpm: float | None
    tempo_confidence: float

    def as_json(self) -> dict[str, str | int | float | None]:
        return {
            "schema": "local-audio/v1",
            "duration_s": round(self.duration_s, 3),
            "sample_rate_hz": self.sample_rate_hz,
            "rms_dbfs": round(self.rms_dbfs, 3),
            "peak_dbfs": round(self.peak_dbfs, 3),
            "level_spread_db": round(self.level_spread_db, 3),
            "activity_ratio": round(self.activity_ratio, 6),
            "zero_crossing_rate": round(self.zero_crossing_rate, 6),
            "high_frequency_ratio": round(self.high_frequency_ratio, 6),
            "onset_rate_hz": round(self.onset_rate_hz, 6),
            "tempo_bpm": None if self.tempo_bpm is None else round(self.tempo_bpm, 3),
            "tempo_confidence": round(self.tempo_confidence, 6),
        }


@dataclass(frozen=True)
class AudioSignalProfile:
    energy: float
    brightness: float
    tension: float
    evidence: tuple[str, ...]
    confidence: str
    metrics: dict[str, str | int | float | None]


def _clamp(value: float, minimum: float = 0.0, maximum: float = 1.0) -> float:
    return max(minimum, min(maximum, value))


def _normalize(value: float, low: float, high: float) -> float:
    if high <= low:
        raise ValueError("normalization range must be increasing")
    return _clamp((value - low) / (high - low))


def _dbfs(amplitude: float) -> float:
    return 20.0 * math.log10(max(amplitude, 1e-6))


def _quantile(sorted_values: Sequence[float], fraction: float) -> float:
    if not sorted_values:
        return 0.0
    position = _clamp(fraction) * (len(sorted_values) - 1)
    lower = math.floor(position)
    upper = math.ceil(position)
    if lower == upper:
        return sorted_values[lower]
    weight = position - lower
    return sorted_values[lower] * (1.0 - weight) + sorted_values[upper] * weight


def _median(values: Sequence[float]) -> float:
    return _quantile(sorted(values), 0.5)


def _estimate_tempo(
    window_levels: Sequence[float],
    windows_per_second: float,
    duration_s: float,
) -> tuple[float | None, float]:
    if duration_s < 15.0 or len(window_levels) < 16:
        return None, 0.0

    envelope = [
        max(0.0, current - previous)
        for previous, current in pairwise(window_levels)
    ]
    if not envelope:
        return None, 0.0
    floor = _median(envelope)
    envelope = [max(0.0, value - floor) for value in envelope]
    energy = sum(value * value for value in envelope)
    if energy <= 1e-9:
        return None, 0.0

    min_lag = max(1, round(windows_per_second * 60.0 / 200.0))
    max_lag = min(len(envelope) // 2, round(windows_per_second * 60.0 / 40.0))
    if max_lag < min_lag:
        return None, 0.0

    best_lag: int | None = None
    best_score = 0.0
    for lag in range(min_lag, max_lag + 1):
        left = envelope[lag:]
        right = envelope[:-lag]
        numerator = sum(a * b for a, b in zip(left, right, strict=True))
        left_energy = sum(value * value for value in left)
        right_energy = sum(value * value for value in right)
        denominator = math.sqrt(left_energy * right_energy)
        if denominator <= 1e-12:
            continue
        score = numerator / denominator
        if score > best_score:
            best_score = score
            best_lag = lag

    confidence = _clamp(best_score)
    if best_lag is None or confidence < 0.2:
        return None, confidence
    return 60.0 * windows_per_second / best_lag, confidence


class _SignalAccumulator:
    def __init__(self, sample_rate: int) -> None:
        if sample_rate <= 0:
            raise AudioSignalError("decoder reported an invalid sample rate")
        self.sample_rate = sample_rate
        self.window_size = max(64, round(sample_rate * 0.05))
        self.total_samples = 0
        self.sum_squares = 0.0
        self.sum_difference_squares = 0.0
        self.peak = 0.0
        self.zero_crossings = 0
        self.previous: float | None = None
        self.window_samples = 0
        self.window_sum_squares = 0.0
        self.window_levels: list[float] = []

    def add(self, samples: Sequence[float]) -> None:
        for raw in samples:
            sample = _clamp(float(raw), -1.0, 1.0)
            squared = sample * sample
            self.total_samples += 1
            self.sum_squares += squared
            self.peak = max(self.peak, abs(sample))
            if self.previous is not None:
                difference = sample - self.previous
                self.sum_difference_squares += difference * difference
                if (sample < 0.0 <= self.previous) or (self.previous < 0.0 <= sample):
                    self.zero_crossings += 1
            self.previous = sample

            self.window_samples += 1
            self.window_sum_squares += squared
            if self.window_samples >= self.window_size:
                self._finish_window()

    def _finish_window(self) -> None:
        if self.window_samples:
            self.window_levels.append(
                math.sqrt(self.window_sum_squares / self.window_samples)
            )
        self.window_samples = 0
        self.window_sum_squares = 0.0

    def finish(self) -> AudioSignalMeasurements:
        self._finish_window()
        duration_s = self.total_samples / self.sample_rate
        if duration_s < _MIN_ANALYZABLE_SECONDS or self.total_samples < 2:
            raise AudioSignalError("decoded audio is empty or too short to analyze")

        rms = math.sqrt(self.sum_squares / self.total_samples)
        activity_floor = max(10 ** (-50.0 / 20.0), rms * 0.1)
        active_levels = [level for level in self.window_levels if level >= activity_floor]
        activity_ratio = (
            len(active_levels) / len(self.window_levels) if self.window_levels else 0.0
        )
        active_db = sorted(_dbfs(level) for level in active_levels)
        level_spread_db = max(
            0.0,
            _quantile(active_db, 0.9) - _quantile(active_db, 0.1),
        )

        level_deltas = [
            max(0.0, current - previous)
            for previous, current in pairwise(self.window_levels)
        ]
        delta_median = _median(level_deltas)
        delta_deviations = [abs(value - delta_median) for value in level_deltas]
        onset_threshold = max(0.002, delta_median + 3.0 * _median(delta_deviations))
        onset_count = sum(value > onset_threshold for value in level_deltas)
        onset_rate_hz = onset_count / duration_s

        windows_per_second = self.sample_rate / self.window_size
        tempo_bpm, tempo_confidence = _estimate_tempo(
            self.window_levels,
            windows_per_second,
            duration_s,
        )
        zero_crossing_rate = self.zero_crossings / max(1, self.total_samples - 1)
        high_frequency_ratio = math.sqrt(
            self.sum_difference_squares / max(4.0 * self.sum_squares, 1e-12)
        )

        return AudioSignalMeasurements(
            duration_s=duration_s,
            sample_rate_hz=self.sample_rate,
            rms_dbfs=_dbfs(rms),
            peak_dbfs=_dbfs(self.peak),
            level_spread_db=level_spread_db,
            activity_ratio=activity_ratio,
            zero_crossing_rate=_clamp(zero_crossing_rate),
            high_frequency_ratio=_clamp(high_frequency_ratio),
            onset_rate_hz=onset_rate_hz,
            tempo_bpm=tempo_bpm,
            tempo_confidence=tempo_confidence,
        )


def _pcm_sample(data: bytes, offset: int, width: int) -> float:
    if width == 1:
        return (data[offset] - 128) / 128.0
    if width == 2:
        return int.from_bytes(data[offset : offset + 2], "little", signed=True) / 32768.0
    if width == 3:
        raw = int.from_bytes(data[offset : offset + 3], "little", signed=False)
        if raw & 0x800000:
            raw -= 1 << 24
        return raw / 8388608.0
    if width == 4:
        return (
            int.from_bytes(data[offset : offset + 4], "little", signed=True)
            / 2147483648.0
        )
    raise AudioSignalError(f"unsupported PCM sample width: {width * 8} bits")


def _wav_samples(data: bytes, channels: int, width: int) -> list[float]:
    frame_width = channels * width
    if channels <= 0 or len(data) % frame_width:
        raise AudioSignalError("WAV data contains an incomplete PCM frame")
    samples: list[float] = []
    for frame_offset in range(0, len(data), frame_width):
        total = 0.0
        for channel in range(channels):
            total += _pcm_sample(data, frame_offset + channel * width, width)
        samples.append(total / channels)
    return samples


@contextmanager
def _open_wav_pcm(path: Path) -> Iterator[tuple[int, Iterator[list[float]]]]:
    try:
        with wave.open(str(path), "rb") as source:
            if source.getcomptype() != "NONE":
                raise AudioSignalError(
                    f"unsupported compressed WAV encoding: {source.getcomptype()}"
                )
            channels = source.getnchannels()
            width = source.getsampwidth()
            sample_rate = source.getframerate()

            def chunks() -> Iterator[list[float]]:
                while data := source.readframes(_FRAMES_PER_CHUNK):
                    yield _wav_samples(data, channels, width)

            yield sample_rate, chunks()
    except (OSError, EOFError, wave.Error) as exc:
        raise AudioSignalError(f"WAV decoder could not open the file: {exc}") from exc


@contextmanager
def _open_ffmpeg_pcm(
    path: Path,
    executable: str,
    sample_rate: int = _TARGET_SAMPLE_RATE,
) -> Iterator[tuple[int, Iterator[list[float]]]]:
    command = [
        executable,
        "-v",
        "error",
        "-nostdin",
        "-threads",
        "1",
        "-i",
        str(path),
        "-map",
        "0:a:0",
        "-vn",
        "-ac",
        "1",
        "-ar",
        str(sample_rate),
        "-f",
        "s16le",
        "-acodec",
        "pcm_s16le",
        "pipe:1",
    ]
    try:
        process = subprocess.Popen(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError as exc:
        raise AudioSignalError(f"FFmpeg could not start: {exc}") from exc
    stdout = process.stdout
    stderr = process.stderr
    assert stdout is not None
    assert stderr is not None

    error_bytes = bytearray()

    def drain_errors() -> None:
        while chunk := stderr.read(4_096):
            remaining = _FFMPEG_ERROR_LIMIT - len(error_bytes)
            if remaining > 0:
                error_bytes.extend(chunk[:remaining])

    error_thread = threading.Thread(target=drain_errors, daemon=True)
    error_thread.start()

    def chunks() -> Iterator[list[float]]:
        pending = b""
        while data := stdout.read(_FRAMES_PER_CHUNK * 2):
            payload = pending + data
            even_length = len(payload) - (len(payload) % 2)
            pending = payload[even_length:]
            values = array("h")
            values.frombytes(payload[:even_length])
            if sys.byteorder != "little":
                values.byteswap()
            yield [value / 32768.0 for value in values]
        if pending:
            raise AudioSignalError("FFmpeg returned an incomplete PCM sample")

    body_failed = False
    try:
        yield sample_rate, chunks()
    except BaseException:
        body_failed = True
        raise
    finally:
        stdout.close()
        if process.poll() is None:
            if body_failed:
                process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        error_thread.join(timeout=5)
        stderr.close()

    if process.returncode != 0:
        detail = error_bytes.decode("utf-8", errors="replace").strip()
        raise AudioSignalError(detail or f"FFmpeg exited with code {process.returncode}")


@contextmanager
def _open_mono_pcm(
    path: Path,
    sample_rate: int = _TARGET_SAMPLE_RATE,
) -> Iterator[tuple[int, Iterator[list[float]]]]:
    executable = shutil.which("ffmpeg")
    if executable is not None:
        with _open_ffmpeg_pcm(path, executable, sample_rate) as decoded:
            yield decoded
        return
    if path.suffix.lower() == ".wav":
        with _open_wav_pcm(path) as decoded:
            yield decoded
        return
    raise AudioSignalError(
        "FFmpeg is unavailable; local fallback decoding supports PCM WAV files only"
    )


def analyze_audio_file(
    path: Path,
    *,
    check_cancelled: Callable[[], None] | None = None,
) -> AudioSignalProfile:
    """Decode one file and return versioned, bounded signal measurements."""

    if not path.is_file():
        raise AudioSignalError("audio file is missing")
    cancellation_check = check_cancelled or (lambda: None)
    with _open_mono_pcm(path) as (sample_rate, chunks):
        accumulator = _SignalAccumulator(sample_rate)
        cancellation_check()
        last_cancellation_check = time.monotonic()
        for samples in chunks:
            now = time.monotonic()
            if now - last_cancellation_check >= 0.25:
                cancellation_check()
                last_cancellation_check = now
            accumulator.add(samples)
        cancellation_check()
        measurements = accumulator.finish()

    energy = 0.75 * _normalize(measurements.rms_dbfs, -36.0, -9.0)
    energy += 0.25 * measurements.activity_ratio
    brightness = 0.65 * _normalize(measurements.high_frequency_ratio, 0.03, 0.45)
    brightness += 0.35 * _normalize(measurements.zero_crossing_rate, 0.01, 0.20)
    transient_activity = _normalize(measurements.onset_rate_hz, 0.2, 4.0)
    if measurements.tempo_bpm is None:
        tension = 0.65 * transient_activity + 0.35 * brightness
    else:
        tempo_activity = _normalize(measurements.tempo_bpm, 60.0, 180.0)
        tension = 0.50 * transient_activity + 0.25 * brightness
        tension += 0.25 * tempo_activity * measurements.tempo_confidence

    if measurements.duration_s >= 45.0 and measurements.activity_ratio >= 0.5:
        confidence = "high"
    elif measurements.duration_s >= 10.0 and measurements.activity_ratio >= 0.2:
        confidence = "medium"
    else:
        confidence = "low"

    tempo_evidence = (
        "No stable tempo estimate was found"
        if measurements.tempo_bpm is None
        else (
            f"Tempo estimate {measurements.tempo_bpm:.1f} BPM "
            f"({measurements.tempo_confidence:.0%} periodicity confidence)"
        )
    )
    evidence = (
        f"Signal level: {measurements.rms_dbfs:.1f} dBFS RMS, "
        f"{measurements.peak_dbfs:.1f} dBFS peak",
        f"Short-window level spread: {measurements.level_spread_db:.1f} dB",
        f"High-frequency proxy: {measurements.high_frequency_ratio:.3f}; "
        f"zero-crossing rate: {measurements.zero_crossing_rate:.3f}",
        f"Transient onset rate: {measurements.onset_rate_hz:.2f} per second",
        tempo_evidence,
        "Signal proxies do not identify instruments, genre, scene, or mood.",
    )
    return AudioSignalProfile(
        energy=round(_clamp(energy), 6),
        brightness=round(_clamp(brightness), 6),
        tension=round(_clamp(tension), 6),
        evidence=evidence,
        confidence=confidence,
        metrics=measurements.as_json(),
    )
