"""Comprehensive, factual context distilled from a full audio recording.

The output intentionally contains no mood, scene, setting, period, genre, or
instrument tags.  It preserves temporal behaviour in bounded trajectories and
sections so a later, review-only classifier can make those semantic choices.
"""

import json
import math
import re
import shutil
import subprocess
import time
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from itertools import pairwise
from pathlib import Path

import numpy as np
import numpy.typing as npt

from app.assistant.audio_signal import AudioSignalError, _open_mono_pcm
from app.assistant.voice_analysis import analyze_voice

type _FloatArray = npt.NDArray[np.float64]

CONTEXT_SAMPLE_RATE = 16_000
CONTEXT_FRAME_SECONDS = 0.5
CONTEXT_TIMELINE_SECONDS = 2.0
CONTEXT_ANALYZER_ID = "local-context/v1"
# Keep the public v1 evidence contract while making implementation changes
# explicit in staleness fingerprints. local-context/v2 remains reserved for
# recalibrated measurement semantics rather than a performance-only rewrite.
CONTEXT_IMPLEMENTATION_ID = "local-context/v1+numpy-rfft/v1"
_FFT_SIZE = 2_048
_MAX_SECTIONS = 10
_MIN_SECTION_SECONDS = 10.0
_LOUDNORM_JSON = re.compile(r"\{\s*\"input_i\".*?\}", re.DOTALL)
_SPECTRUM_WINDOW: _FloatArray = np.hanning(_FFT_SIZE)


def _clamp(value: float, low: float = 0.0, high: float = 1.0) -> float:
    return max(low, min(high, value))


def _normalize(value: float, low: float, high: float) -> float:
    if high <= low:
        raise ValueError("normalization range must increase")
    return _clamp((value - low) / (high - low))


def _dbfs(amplitude: float) -> float:
    return 20.0 * math.log10(max(amplitude, 1e-7))


def _quantile(values: Sequence[float], fraction: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    position = _clamp(fraction) * (len(ordered) - 1)
    lower = math.floor(position)
    upper = math.ceil(position)
    if lower == upper:
        return ordered[lower]
    weight = position - lower
    return ordered[lower] * (1.0 - weight) + ordered[upper] * weight


def _mean(values: Sequence[float]) -> float:
    return sum(values) / len(values) if values else 0.0


def _median(values: Sequence[float]) -> float:
    return _quantile(values, 0.5)


def _round(value: float, digits: int = 5) -> float:
    return round(value, digits)


@dataclass(frozen=True)
class _Spectrum:
    centroid_hz: float
    bandwidth_hz: float
    rolloff_hz: float
    flatness: float
    bass_ratio: float
    mid_ratio: float
    high_ratio: float
    peak_concentration: float


def _spectrum(
    samples: Sequence[float] | _FloatArray,
    sample_rate: int,
) -> _Spectrum:
    values = np.asarray(samples, dtype=np.float64)
    if len(samples) >= _FFT_SIZE:
        start = (len(samples) - _FFT_SIZE) // 2
        selected = values[start : start + _FFT_SIZE]
    else:
        selected = np.pad(values, (0, _FFT_SIZE - len(samples)))
    # rfft uses the native pocketfft path and avoids constructing the mirrored
    # half of a real-valued spectrum. Drop Nyquist to preserve the established
    # v1 bin contract (the previous implementation returned exactly 1024 bins).
    transformed = np.fft.rfft(selected * _SPECTRUM_WINDOW)[:-1]
    powers = np.maximum(1e-18, np.square(transformed.real) + np.square(transformed.imag))
    total = float(np.sum(powers, dtype=np.float64))
    if total <= 1e-12:
        return _Spectrum(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0)
    bin_hz = sample_rate / _FFT_SIZE
    frequencies = np.arange(len(powers), dtype=np.float64) * bin_hz
    centroid = float(np.sum(frequencies * powers, dtype=np.float64) / total)
    variance = float(
        np.sum(np.square(frequencies - centroid) * powers, dtype=np.float64) / total
    )
    threshold = total * 0.85
    rolloff_index = min(
        int(np.searchsorted(np.cumsum(powers), threshold, side="left")),
        len(frequencies) - 1,
    )
    rolloff = float(frequencies[rolloff_index])
    geometric = math.exp(float(np.mean(np.log(powers))))
    flatness = _clamp(geometric / (total / len(powers)))

    def band(low: float, high: float) -> float:
        selected_powers = powers[(frequencies >= low) & (frequencies < high)]
        return float(np.sum(selected_powers, dtype=np.float64) / total)

    strongest_count = max(4, len(powers) // 100)
    strongest = float(np.sum(np.sort(powers)[-strongest_count:], dtype=np.float64) / total)
    return _Spectrum(
        centroid_hz=centroid,
        bandwidth_hz=math.sqrt(max(0.0, variance)),
        rolloff_hz=rolloff,
        flatness=flatness,
        bass_ratio=band(20.0, 250.0),
        mid_ratio=band(250.0, 2_000.0),
        high_ratio=band(2_000.0, sample_rate / 2.0),
        peak_concentration=_clamp(strongest),
    )


@dataclass(frozen=True)
class _Frame:
    start_s: float
    duration_s: float
    loudness_dbfs: float
    peak_dbfs: float
    zero_crossing_rate: float
    difference_ratio: float
    spectrum: _Spectrum


class _ContextAccumulator:
    def __init__(self, sample_rate: int) -> None:
        if sample_rate <= 0:
            raise AudioSignalError("decoder reported an invalid sample rate")
        self.sample_rate = sample_rate
        self.frame_size = max(256, round(sample_rate * CONTEXT_FRAME_SECONDS))
        self.short_size = max(64, round(sample_rate * 0.05))
        self.total_samples = 0
        self.total_squares = 0.0
        self.peak = 0.0
        self.framed_samples = 0
        self.pending: _FloatArray = np.empty(0, dtype=np.float64)
        self.short_samples = 0
        self.short_squares = 0.0
        self.short_levels: list[float] = []
        self.frames: list[_Frame] = []
        self.spectrum_seconds = 0.0

    def add(self, samples: Sequence[float]) -> None:
        values = np.asarray(samples, dtype=np.float64)
        if values.size == 0:
            return
        values = np.clip(values, -1.0, 1.0)
        self.total_samples += int(values.size)
        self.total_squares += float(np.sum(np.square(values), dtype=np.float64))
        self.peak = max(self.peak, float(np.max(np.abs(values))))
        self._add_short_levels(values)

        combined = np.concatenate((self.pending, values)) if self.pending.size else values
        framed_count = (combined.size // self.frame_size) * self.frame_size
        if framed_count:
            for frame in combined[:framed_count].reshape(-1, self.frame_size):
                self._finish_frame(frame)
        self.pending = combined[framed_count:].copy()

    def _add_short_levels(self, values: _FloatArray) -> None:
        position = 0
        if self.short_samples:
            take = min(self.short_size - self.short_samples, int(values.size))
            block = values[:take]
            self.short_squares += float(np.sum(np.square(block), dtype=np.float64))
            self.short_samples += take
            position = take
            if self.short_samples == self.short_size:
                self.short_levels.append(math.sqrt(self.short_squares / self.short_samples))
                self.short_samples = 0
                self.short_squares = 0.0

        remaining = values[position:]
        full_count = remaining.size // self.short_size
        if full_count:
            full_end = full_count * self.short_size
            blocks = remaining[:full_end].reshape(full_count, self.short_size)
            means = np.mean(np.square(blocks), axis=1)
            self.short_levels.extend(np.sqrt(means).tolist())
            remaining = remaining[full_end:]
        if remaining.size:
            self.short_samples = int(remaining.size)
            self.short_squares = float(np.sum(np.square(remaining), dtype=np.float64))

    def _finish_frame(self, samples: _FloatArray) -> None:
        if samples.size == 0:
            return
        squares = float(np.sum(np.square(samples), dtype=np.float64))
        rms = math.sqrt(squares / samples.size)
        peak = float(np.max(np.abs(samples)))
        previous = samples[:-1]
        current = samples[1:]
        zero_crossings = int(
            np.count_nonzero(
                ((current < 0.0) & (previous >= 0.0))
                | ((previous < 0.0) & (current >= 0.0))
            )
        )
        differences = np.diff(samples)
        difference_squares = float(np.sum(np.square(differences), dtype=np.float64))
        start_s = self.framed_samples / self.sample_rate
        spectrum_started = time.perf_counter()
        spectrum = _spectrum(samples, self.sample_rate)
        self.spectrum_seconds += time.perf_counter() - spectrum_started
        self.frames.append(
            _Frame(
                start_s=start_s,
                duration_s=samples.size / self.sample_rate,
                loudness_dbfs=_dbfs(rms),
                peak_dbfs=_dbfs(peak),
                zero_crossing_rate=zero_crossings / max(1, samples.size - 1),
                difference_ratio=math.sqrt(difference_squares / max(4.0 * squares, 1e-12)),
                spectrum=spectrum,
            )
        )
        self.framed_samples += int(samples.size)

    def finish(self) -> tuple[list[_Frame], list[float], dict[str, float]]:
        if self.short_samples:
            self.short_levels.append(math.sqrt(self.short_squares / self.short_samples))
        if self.pending.size:
            self._finish_frame(self.pending)
            self.pending = np.empty(0, dtype=np.float64)
        duration = self.total_samples / self.sample_rate
        if duration < 0.1 or not self.frames:
            raise AudioSignalError("decoded audio is empty or too short to analyze")
        return (
            self.frames,
            self.short_levels,
            {
                "duration_s": duration,
                "decoded_sample_rate_hz": float(self.sample_rate),
                "rms_dbfs": _dbfs(math.sqrt(self.total_squares / self.total_samples)),
                "peak_dbfs": _dbfs(self.peak),
            },
        )


def _estimate_tempo(levels: Sequence[float], windows_per_second: float) -> tuple[float | None, float]:
    if len(levels) < round(windows_per_second * 12.0):
        return None, 0.0
    envelope = [max(0.0, current - previous) for previous, current in pairwise(levels)]
    floor = _median(envelope)
    envelope = [max(0.0, value - floor) for value in envelope]
    energy = sum(value * value for value in envelope)
    if energy <= 1e-10:
        return None, 0.0
    min_lag = max(1, round(windows_per_second * 60.0 / 200.0))
    max_lag = min(len(envelope) // 2, round(windows_per_second * 60.0 / 40.0))
    best_lag: int | None = None
    best = 0.0
    for lag in range(min_lag, max_lag + 1):
        left = envelope[lag:]
        right = envelope[:-lag]
        numerator = sum(a * b for a, b in zip(left, right, strict=True))
        denominator = math.sqrt(
            sum(value * value for value in left) * sum(value * value for value in right)
        )
        if denominator > 1e-12 and numerator / denominator > best:
            best = numerator / denominator
            best_lag = lag
    confidence = _clamp(best)
    if best_lag is None or confidence < 0.2:
        return None, confidence
    return 60.0 * windows_per_second / best_lag, confidence


def _tempo_curve(short_levels: Sequence[float], duration_s: float) -> list[dict[str, float]]:
    windows_per_second = 20.0
    window = round(windows_per_second * 30.0)
    hop = round(windows_per_second * 15.0)
    if len(short_levels) < window:
        tempo, confidence = _estimate_tempo(short_levels, windows_per_second)
        return [] if tempo is None else [{"at_fraction": 0.5, "bpm": _round(tempo, 2), "confidence": _round(confidence)}]
    points: list[dict[str, float]] = []
    for start in range(0, len(short_levels) - window + 1, hop):
        tempo, confidence = _estimate_tempo(short_levels[start : start + window], windows_per_second)
        if tempo is None:
            continue
        center_s = (start + window / 2.0) / windows_per_second
        points.append(
            {
                "at_fraction": _round(_clamp(center_s / max(duration_s, 0.001))),
                "bpm": _round(tempo, 2),
                "confidence": _round(confidence),
            }
        )
    return points


def _trajectory(values: Sequence[float]) -> dict[str, float | str]:
    if not values:
        return {
            "typical": 0.0,
            "low": 0.0,
            "high": 0.0,
            "range": 0.0,
            "variability": 0.0,
            "slope": 0.0,
            "start": 0.0,
            "end": 0.0,
            "peak_at_fraction": 0.0,
            "high_fraction": 0.0,
            "shape": "unknown",
        }
    count = len(values)
    edge = max(1, count // 10)
    start = _mean(values[:edge])
    end = _mean(values[-edge:])
    x_mean = 0.5
    y_mean = _mean(values)
    denominator = sum(((index / max(1, count - 1)) - x_mean) ** 2 for index in range(count))
    slope = (
        sum(
            ((index / max(1, count - 1)) - x_mean) * (value - y_mean)
            for index, value in enumerate(values)
        )
        / denominator
        if denominator > 0.0
        else 0.0
    )
    deltas = [abs(current - previous) for previous, current in pairwise(values)]
    low = _quantile(values, 0.1)
    high = _quantile(values, 0.9)
    variability = _clamp(0.65 * (high - low) + 0.35 * min(1.0, _median(deltas) * 6.0))
    quarters = [
        _mean(values[round(count * start_fraction) : max(round(count * end_fraction), round(count * start_fraction) + 1)])
        for start_fraction, end_fraction in ((0.0, 0.25), (0.25, 0.5), (0.5, 0.75), (0.75, 1.0))
    ]
    if high - low < 0.12 and variability < 0.18:
        shape = "steady"
    elif variability > 0.58:
        shape = "volatile"
    elif quarters[1] > quarters[0] + 0.15 and quarters[2] > quarters[3] + 0.15:
        shape = "arch"
    elif quarters[1] < quarters[0] - 0.15 and quarters[2] < quarters[3] - 0.15:
        shape = "dip_then_recovery"
    elif end - start > 0.28:
        shape = "gradual_rise" if variability < 0.4 else "stepped_build"
    elif start - end > 0.28:
        shape = "gradual_fall" if variability < 0.4 else "stepped_release"
    elif variability > 0.32:
        shape = "alternating"
    elif slope > 0.12:
        shape = "rising"
    elif slope < -0.12:
        shape = "falling"
    else:
        shape = "mixed"
    peak_index = max(range(count), key=lambda index: values[index])
    threshold = _quantile(values, 0.75)
    return {
        "typical": _round(_median(values)),
        "low": _round(low),
        "high": _round(high),
        "range": _round(high - low),
        "variability": _round(variability),
        "slope": _round(slope),
        "start": _round(start),
        "end": _round(end),
        "peak_at_fraction": _round(peak_index / max(1, count - 1)),
        "high_fraction": _round(sum(value >= threshold for value in values) / count),
        "shape": shape,
    }


def _timeline_frames(frames: Sequence[_Frame], short_levels: Sequence[float]) -> list[dict[str, float]]:
    positive_deltas = [max(0.0, current - previous) for previous, current in pairwise(short_levels)]
    median = _median(positive_deltas)
    mad = _median([abs(value - median) for value in positive_deltas])
    onset_threshold = max(0.0015, median + 3.0 * mad)
    onset_flags = [value > onset_threshold for value in positive_deltas]
    short_per_frame = max(1, round(CONTEXT_FRAME_SECONDS / 0.05))
    rows: list[dict[str, float]] = []
    previous_bands: tuple[float, float, float] | None = None
    for index, frame in enumerate(frames):
        start = index * short_per_frame
        local_onsets = sum(onset_flags[start : start + short_per_frame])
        onset_rate = local_onsets / max(frame.duration_s, 0.001)
        bands = (frame.spectrum.bass_ratio, frame.spectrum.mid_ratio, frame.spectrum.high_ratio)
        flux = (
            sum(abs(left - right) for left, right in zip(bands, previous_bands, strict=True))
            if previous_bands is not None
            else 0.0
        )
        previous_bands = bands
        loudness = _normalize(frame.loudness_dbfs, -44.0, -8.0)
        brightness = _clamp(frame.spectrum.centroid_hz / max(1.0, frame.spectrum.rolloff_hz, 6_000.0))
        rhythmic = _clamp(0.65 * _normalize(onset_rate, 0.0, 5.0) + 0.35 * _normalize(flux, 0.0, 0.6))
        occupied = sum(ratio >= 0.08 for ratio in bands) / 3.0
        density = _clamp(0.45 * loudness + 0.30 * occupied + 0.25 * _normalize(frame.spectrum.bandwidth_hz, 300.0, 3_500.0))
        intensity = _clamp(0.55 * loudness + 0.25 * rhythmic + 0.20 * _normalize(frame.difference_ratio, 0.02, 0.45))
        rows.append(
            {
                "start_s": _round(frame.start_s, 3),
                "duration_s": _round(frame.duration_s, 3),
                "loudness": _round(loudness),
                "intensity": _round(intensity),
                "rhythmic_drive": _round(rhythmic),
                "brightness": _round(brightness),
                "density": _round(density),
                "spectral_flux": _round(_normalize(flux, 0.0, 0.6)),
                "bass_ratio": _round(frame.spectrum.bass_ratio),
                "mid_ratio": _round(frame.spectrum.mid_ratio),
                "high_ratio": _round(frame.spectrum.high_ratio),
                "spectral_flatness": _round(frame.spectrum.flatness),
                "peak_concentration": _round(frame.spectrum.peak_concentration),
            }
        )
    return rows


def _downsample_timeline(rows: Sequence[dict[str, float]]) -> list[dict[str, float]]:
    group_size = max(1, round(CONTEXT_TIMELINE_SECONDS / CONTEXT_FRAME_SECONDS))
    output: list[dict[str, float]] = []
    for start in range(0, len(rows), group_size):
        group = rows[start : start + group_size]
        keys = [key for key in group[0] if key not in {"start_s", "duration_s"}]
        output.append(
            {
                "start_s": group[0]["start_s"],
                "duration_s": _round(sum(row["duration_s"] for row in group), 3),
                **{key: _round(_mean([row[key] for row in group])) for key in keys},
            }
        )
    return output


def _change_boundaries(rows: Sequence[dict[str, float]]) -> list[int]:
    if len(rows) < 12:
        return [0, len(rows)]
    window = 4
    scores: list[tuple[float, int]] = []
    keys = ("intensity", "rhythmic_drive", "brightness", "density", "spectral_flux")
    for index in range(window, len(rows) - window):
        before = rows[index - window : index]
        after = rows[index : index + window]
        score = _mean(
            [
                abs(_mean([row[key] for row in before]) - _mean([row[key] for row in after]))
                for key in keys
            ]
        )
        scores.append((score, index))
    score_values = [score for score, _ in scores]
    threshold = max(0.12, _quantile(score_values, 0.75) + _median([abs(score - _median(score_values)) for score in score_values]))
    minimum = max(2, round(_MIN_SECTION_SECONDS / CONTEXT_FRAME_SECONDS))
    selected: list[int] = []
    for score, index in sorted(scores, reverse=True):
        if score < threshold or len(selected) >= _MAX_SECTIONS - 1:
            break
        if index < minimum or len(rows) - index < minimum:
            continue
        if all(abs(index - existing) >= minimum for existing in selected):
            selected.append(index)
    return [0, *sorted(selected), len(rows)]


def _section_summary(
    rows: Sequence[dict[str, float]],
    boundaries: Sequence[int],
    tempo_points: Sequence[dict[str, float]],
    duration_s: float,
) -> list[dict[str, object]]:
    sections: list[dict[str, object]] = []
    for number, (start, end) in enumerate(pairwise(boundaries), start=1):
        group = rows[start:end]
        start_s = group[0]["start_s"]
        end_s = group[-1]["start_s"] + group[-1]["duration_s"]
        center_fraction = ((start_s + end_s) / 2.0) / max(duration_s, 0.001)
        nearest_tempo = (
            min(tempo_points, key=lambda point: abs(point["at_fraction"] - center_fraction))
            if tempo_points
            else None
        )
        section: dict[str, object] = {
            "id": f"s{number}",
            "start_s": _round(start_s, 3),
            "end_s": _round(end_s, 3),
            "start_fraction": _round(start_s / max(duration_s, 0.001)),
            "end_fraction": _round(end_s / max(duration_s, 0.001)),
            "intensity": _round(_median([row["intensity"] for row in group])),
            "rhythmic_drive": _round(_median([row["rhythmic_drive"] for row in group])),
            "brightness": _round(_median([row["brightness"] for row in group])),
            "density": _round(_median([row["density"] for row in group])),
            "changes_from_previous": [],
            "repeats_section_ids": [],
        }
        if nearest_tempo is not None:
            section["tempo_bpm"] = nearest_tempo["bpm"]
            section["tempo_confidence"] = nearest_tempo["confidence"]
        else:
            section["tempo_bpm"] = None
            section["tempo_confidence"] = 0.0
        if sections:
            previous = sections[-1]
            changes: list[str] = []
            for key, up, down in (
                ("intensity", "more_intense", "less_intense"),
                ("rhythmic_drive", "more_rhythmic", "less_rhythmic"),
                ("brightness", "brighter", "darker_spectrum"),
                ("density", "denser", "sparser"),
            ):
                delta = _object_float(section[key]) - _object_float(previous[key])
                if delta >= 0.14:
                    changes.append(up)
                elif delta <= -0.14:
                    changes.append(down)
            section["changes_from_previous"] = changes
        for earlier in sections[:-1]:
            squared_differences = [
                (_object_float(section[key]) - _object_float(earlier[key])) ** 2
                for key in ("intensity", "rhythmic_drive", "brightness", "density")
            ]
            distance = math.sqrt(math.fsum(squared_differences) / 4.0)
            if distance <= 0.10:
                cast_ids = section["repeats_section_ids"]
                assert isinstance(cast_ids, list)
                cast_ids.append(earlier["id"])
        sections.append(section)
    return sections


def _run_json_command(command: list[str], timeout: float = 30.0) -> dict[str, object] | None:
    try:
        result = subprocess.run(
            command,
            capture_output=True,
            check=False,
            timeout=timeout,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if result.returncode != 0:
        return None
    try:
        value = json.loads(result.stdout)
    except json.JSONDecodeError:
        return None
    return value if isinstance(value, dict) else None


def _technical_probe(path: Path) -> dict[str, object]:
    executable = shutil.which("ffprobe")
    if executable is None:
        return {"probe_status": "unavailable", "file_extension": path.suffix.lower()}
    result = _run_json_command(
        [
            executable,
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name,sample_rate,channels,channel_layout,bit_rate",
            "-show_entries",
            "format=format_name,duration,bit_rate",
            "-of",
            "json",
            str(path),
        ]
    )
    if result is None:
        return {"probe_status": "failed", "file_extension": path.suffix.lower()}
    streams = result.get("streams")
    stream = streams[0] if isinstance(streams, list) and streams and isinstance(streams[0], dict) else {}
    file_format = result.get("format") if isinstance(result.get("format"), dict) else {}
    assert isinstance(stream, dict)
    assert isinstance(file_format, dict)
    return {
        "probe_status": "complete",
        "file_extension": path.suffix.lower(),
        "codec": stream.get("codec_name"),
        "sample_rate_hz": _safe_int(stream.get("sample_rate")),
        "channels": _safe_int(stream.get("channels")),
        "channel_layout": stream.get("channel_layout"),
        "bit_rate": _safe_int(stream.get("bit_rate") or file_format.get("bit_rate")),
        "container": file_format.get("format_name"),
    }


def _safe_int(value: object) -> int | None:
    if not isinstance(value, (str, bytes, bytearray, int, float)):
        return None
    try:
        parsed = int(value)
    except (TypeError, ValueError):
        return None
    return parsed if parsed >= 0 else None


def _ebu_loudness(
    path: Path,
    *,
    check_cancelled: Callable[[], None] | None = None,
) -> dict[str, float] | None:
    executable = shutil.which("ffmpeg")
    if executable is None:
        return None
    cancellation_check = check_cancelled or (lambda: None)
    try:
        process = subprocess.Popen(
            [
                executable,
                "-hide_banner",
                "-nostats",
                "-v",
                "info",
                "-nostdin",
                "-i",
                str(path),
                "-map",
                "0:a:0",
                "-af",
                "loudnorm=I=-24:LRA=7:TP=-2:print_format=json",
                "-f",
                "null",
                "-",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
    except OSError:
        return None
    deadline = time.monotonic() + 1_800
    try:
        while True:
            try:
                _, stderr = process.communicate(timeout=0.25)
                break
            except subprocess.TimeoutExpired:
                cancellation_check()
                if time.monotonic() >= deadline:
                    process.kill()
                    process.communicate()
                    return None
    except BaseException:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        raise
    if process.returncode != 0:
        return None
    matches = _LOUDNORM_JSON.findall(stderr)
    if not matches:
        return None
    try:
        raw = json.loads(matches[-1])
        parsed = {
            "integrated_lufs": float(raw["input_i"]),
            "loudness_range_lu": float(raw["input_lra"]),
            "true_peak_dbtp": float(raw["input_tp"]),
            "relative_threshold_lufs": float(raw["input_thresh"]),
        }
    except (KeyError, TypeError, ValueError, json.JSONDecodeError):
        return None
    return parsed if all(math.isfinite(value) for value in parsed.values()) else None


@dataclass(frozen=True)
class AudioContextPerformance:
    audio_seconds: float
    elapsed_seconds: float
    stage_seconds: dict[str, float]


@dataclass(frozen=True)
class AudioContextDocument:
    confidence: str
    completeness: str
    summary: dict[str, object]
    timeline: list[dict[str, float]]
    sections: list[dict[str, object]]
    technical: dict[str, object]
    stages: dict[str, object]
    performance: AudioContextPerformance | None = None


def analyze_audio_context(
    path: Path,
    *,
    check_cancelled: Callable[[], None] | None = None,
) -> AudioContextDocument:
    """Decode a full recording and build bounded temporal context."""

    analysis_started = time.perf_counter()
    if not path.is_file():
        raise AudioSignalError("audio file is missing")
    cancellation_check = check_cancelled or (lambda: None)

    probe_started = time.perf_counter()
    technical = _technical_probe(path)
    stage_seconds = {"probe": time.perf_counter() - probe_started}

    decode_started = time.perf_counter()
    with _open_mono_pcm(path, CONTEXT_SAMPLE_RATE) as (sample_rate, chunks):
        accumulator = _ContextAccumulator(sample_rate)
        last_check = time.monotonic()
        for samples in chunks:
            now = time.monotonic()
            if now - last_check >= 0.25:
                cancellation_check()
                last_check = now
            accumulator.add(samples)
        cancellation_check()
        frames, short_levels, global_metrics = accumulator.finish()
    decode_and_frame_seconds = time.perf_counter() - decode_started
    stage_seconds["spectrum"] = accumulator.spectrum_seconds
    stage_seconds["decode_and_frames"] = max(
        0.0,
        decode_and_frame_seconds - accumulator.spectrum_seconds,
    )

    feature_started = time.perf_counter()
    rows = _timeline_frames(frames, short_levels)
    duration_s = global_metrics["duration_s"]
    tempo_points = _tempo_curve(short_levels, duration_s)
    boundaries = _change_boundaries(rows)
    sections = _section_summary(rows, boundaries, tempo_points, duration_s)
    downsampled = _downsample_timeline(rows)

    trajectories = {
        key: _trajectory([row[key] for row in rows])
        for key in ("loudness", "intensity", "rhythmic_drive", "brightness", "density", "spectral_flux")
    }
    tempo_bpms = [point["bpm"] for point in tempo_points if point["confidence"] >= 0.25]
    tempo_summary: dict[str, object] = {
        "status": "measured" if tempo_bpms else "unresolved",
        "typical_bpm": _round(_median(tempo_bpms), 2) if tempo_bpms else None,
        "low_bpm": _round(_quantile(tempo_bpms, 0.1), 2) if tempo_bpms else None,
        "high_bpm": _round(_quantile(tempo_bpms, 0.9), 2) if tempo_bpms else None,
        "variability": (
            _round(_clamp((_quantile(tempo_bpms, 0.9) - _quantile(tempo_bpms, 0.1)) / 60.0))
            if tempo_bpms
            else None
        ),
        "points": tempo_points[:20],
    }

    stage_seconds["feature_summary"] = time.perf_counter() - feature_started
    voice_started = time.perf_counter()
    voice_analysis = analyze_voice(path, check_cancelled=cancellation_check)
    stage_seconds["voice"] = time.perf_counter() - voice_started

    loudness_started = time.perf_counter()
    loudness = _ebu_loudness(path, check_cancelled=cancellation_check)
    stage_seconds["ebu_loudness"] = time.perf_counter() - loudness_started
    cancellation_check()

    finalize_started = time.perf_counter()
    if loudness is not None:
        technical["loudness"] = {"status": "ebu_r128", **loudness}
    else:
        technical["loudness"] = {
            "status": "dbfs_proxy",
            "rms_dbfs": _round(global_metrics["rms_dbfs"], 3),
            "peak_dbfs": _round(global_metrics["peak_dbfs"], 3),
        }
    technical["decoded_sample_rate_hz"] = int(global_metrics["decoded_sample_rate_hz"])
    technical["duration_s"] = _round(duration_s, 3)

    active_fraction = sum(row["loudness"] > 0.08 for row in rows) / len(rows)
    confidence = "high" if duration_s >= 30.0 and active_fraction >= 0.25 else "medium" if duration_s >= 5.0 else "low"
    repeated_sections = sum(bool(section["repeats_section_ids"]) for section in sections)
    summary: dict[str, object] = {
        "schema_version": CONTEXT_ANALYZER_ID,
        "duration_s": _round(duration_s, 3),
        "confidence": confidence,
        "trajectories": trajectories,
        "tempo": tempo_summary,
        "structure": {
            "section_count": len(sections),
            "major_change_count": max(0, len(sections) - 1),
            "repeated_section_count": repeated_sections,
            "development": (
                "repetitive" if repeated_sections >= 2 else "sectional" if len(sections) >= 3 else "continuous"
            ),
        },
        "voice": voice_analysis.summary,
        "evidence": [
            _trajectory_evidence("Intensity", trajectories["intensity"]),
            _trajectory_evidence("Rhythmic drive", trajectories["rhythmic_drive"]),
            _tempo_evidence(tempo_summary),
            f"{len(sections)} acoustic section{'s' if len(sections) != 1 else ''} with {max(0, len(sections) - 1)} major transition{'s' if len(sections) - 1 != 1 else ''}.",
        ],
    }
    stages: dict[str, object] = {
        "decode": {"status": "complete"},
        "signal": {"status": "complete", "frame_seconds": CONTEXT_FRAME_SECONDS},
        "spectrum": {
            "status": "complete",
            "fft_size": _FFT_SIZE,
            "implementation": "numpy-rfft/v1",
        },
        "tempo": {"status": tempo_summary["status"]},
        "structure": {"status": "complete"},
        "loudness": {"status": "complete" if loudness is not None else "proxy"},
        "voice": voice_analysis.stage,
    }
    stage_seconds["finalize"] = time.perf_counter() - finalize_started
    elapsed_seconds = time.perf_counter() - analysis_started
    return AudioContextDocument(
        confidence=confidence,
        completeness="full",
        summary=summary,
        timeline=downsampled,
        sections=sections,
        technical=technical,
        stages=stages,
        performance=AudioContextPerformance(
            audio_seconds=duration_s,
            elapsed_seconds=elapsed_seconds,
            stage_seconds=stage_seconds,
        ),
    )


def _trajectory_evidence(label: str, trajectory: dict[str, float | str]) -> str:
    return (
        f"{label}: {trajectory['shape']} trajectory; typical {float(trajectory['typical']):.0%}, "
        f"range {float(trajectory['low']):.0%}-{float(trajectory['high']):.0%}, "
        f"peak at {float(trajectory['peak_at_fraction']):.0%} of the track."
    )


def _tempo_evidence(tempo: dict[str, object]) -> str:
    if tempo["status"] != "measured":
        return "No sufficiently stable tempo trajectory was resolved."
    return (
        f"Tempo trajectory: {_object_float(tempo['low_bpm']):.1f}-{_object_float(tempo['high_bpm']):.1f} BPM; "
        f"typical {_object_float(tempo['typical_bpm']):.1f} BPM."
    )


def _object_float(value: object) -> float:
    if not isinstance(value, (str, bytes, bytearray, int, float)):
        return 0.0
    try:
        parsed = float(value)
    except (TypeError, ValueError):
        return 0.0
    return parsed if math.isfinite(parsed) else 0.0
