"""Optional, local music voice/instrumental classification.

The default context builder remains dependency-free.  When an operator points
the application at the exact supported Essentia MusiCNN model, this module
adds bounded classifier evidence without making network requests or turning
the result into a semantic tag.
"""

import hashlib
import math
import subprocess
import sys
from collections.abc import Callable, Iterable, Sequence
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path
from typing import Any

from app.core.config import get_settings

VOICE_ANALYZER_ID = "essentia-musicnn-voice/v1"
VOICE_MODEL_FILENAME = "voice_instrumental-musicnn-msd-2.pb"
VOICE_MODEL_SHA256 = "b734bca3fc99257cf0088211b44bd36e8a26fbb1f9ce67e1e97d39f188094b0a"
_RUNTIME_PROBE_TIMEOUT_SECONDS = 30.0
_RUNTIME_PROBE_CODE = """\
from essentia import standard

names = ("MonoLoader", "TensorflowPredictMusiCNN")
raise SystemExit(0 if all(callable(getattr(standard, name, None)) for name in names) else 1)
"""


@dataclass(frozen=True)
class VoiceAnalysis:
    summary: dict[str, object]
    stage: dict[str, object]


class _VoiceRuntimeUnavailable(Exception):
    pass


def _not_classified() -> VoiceAnalysis:
    return VoiceAnalysis(
        summary={
            "status": "not_classified",
            "voice_probability": None,
            "vocal_coverage": None,
            "note": (
                "Local voice classification is not enabled. Spectral measurements "
                "are retained, but they are not presented as voice detection."
            ),
        },
        stage={"status": "not_configured", "required": False},
    )


def deferred_voice_analysis() -> VoiceAnalysis:
    """Describe a voice stage that will run after the signal-analysis pass."""

    return VoiceAnalysis(
        summary={
            "status": "not_classified",
            "voice_probability": None,
            "vocal_coverage": None,
            "note": "Voice detection is waiting for the separate second analysis pass.",
        },
        stage={
            "status": "pending",
            "required": False,
            "analyzer_id": VOICE_ANALYZER_ID,
        },
    )


def _unavailable(reason: str, note: str, *, error_type: str | None = None) -> VoiceAnalysis:
    stage: dict[str, object] = {
        "status": "unavailable",
        "required": False,
        "analyzer_id": VOICE_ANALYZER_ID,
        "reason": reason,
        "model_filename": VOICE_MODEL_FILENAME,
    }
    if error_type is not None:
        stage["error_type"] = error_type
    return VoiceAnalysis(
        summary={
            "status": "unavailable",
            "voice_probability": None,
            "vocal_coverage": None,
            "note": note,
        },
        stage=stage,
    )


@lru_cache(maxsize=8)
def _cached_file_hash(path_text: str, size: int, mtime_ns: int) -> str:
    del size, mtime_ns
    digest = hashlib.sha256()
    with Path(path_text).open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _model_file_hash(path: Path) -> str:
    stat = path.stat()
    return _cached_file_hash(str(path.resolve()), stat.st_size, stat.st_mtime_ns)


@lru_cache(maxsize=1)
def _runtime_available() -> bool:
    """Probe the native runtime without retaining TensorFlow in the web process.

    Importing ``essentia.standard`` loads the bundled TensorFlow runtime and a
    substantial native-memory footprint.  Status and source-signature checks
    run in the parent FastAPI process, which never performs track inference;
    keeping that import there duplicates the memory retained by every analysis
    worker.  A short-lived interpreter gives the same import/capability check
    while releasing its native memory when the probe exits.
    """

    try:
        result = subprocess.run(
            [sys.executable, "-c", _RUNTIME_PROBE_CODE],
            check=False,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=_RUNTIME_PROBE_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.TimeoutExpired):
        return False
    return result.returncode == 0


def voice_analyzer_signature() -> str | None:
    """Return a path-free identity used to invalidate stored track context."""

    model_path = get_settings().assistant_voice_model_path
    if model_path is None:
        return None
    try:
        model_hash = _model_file_hash(model_path)
    except OSError:
        model_hash = "missing"
    runtime = "runtime-present" if _runtime_available() else "runtime-missing"
    return f"{VOICE_ANALYZER_ID}:{model_hash}:{runtime}"


def voice_analyzer_status() -> dict[str, object]:
    """Return a path-free deployment preflight for the analysis UI."""

    model_path = get_settings().assistant_voice_model_path
    base: dict[str, object] = {
        "analyzer_id": VOICE_ANALYZER_ID,
        "model_filename": VOICE_MODEL_FILENAME,
        "model_sha256": VOICE_MODEL_SHA256,
    }
    if model_path is None:
        return {**base, "status": "not_configured", "reason": None}
    if not model_path.is_file():
        return {**base, "status": "unavailable", "reason": "model_missing"}
    try:
        model_hash = _model_file_hash(model_path)
    except OSError:
        return {**base, "status": "unavailable", "reason": "model_unreadable"}
    if model_hash != VOICE_MODEL_SHA256:
        return {**base, "status": "unavailable", "reason": "unsupported_model"}
    if not _runtime_available():
        return {**base, "status": "unavailable", "reason": "runtime_missing"}
    return {**base, "status": "ready", "reason": None}


def _numeric_pair(value: object) -> tuple[float, float] | None:
    if isinstance(value, (str, bytes, bytearray)) or not isinstance(value, Sequence):
        return None
    if len(value) != 2:
        return None
    try:
        instrumental = float(value[0])
        voice = float(value[1])
    except (TypeError, ValueError):
        return None
    if not all(math.isfinite(item) and 0.0 <= item <= 1.0 for item in (instrumental, voice)):
        return None
    return instrumental, voice


def _prediction_pairs(value: object) -> Iterable[tuple[float, float]]:
    if hasattr(value, "tolist"):
        value = value.tolist()
    pair = _numeric_pair(value)
    if pair is not None:
        yield pair
        return
    if isinstance(value, (str, bytes, bytearray)) or not isinstance(value, Sequence):
        return
    for item in value:
        yield from _prediction_pairs(item)


def _summarize_predictions(predictions: object) -> tuple[float, float, int]:
    probabilities: list[float] = []
    for instrumental_score, voice_score in _prediction_pairs(predictions):
        total = instrumental_score + voice_score
        if total <= 1e-9:
            continue
        probabilities.append(voice_score / total)
    if not probabilities:
        raise ValueError("voice classifier returned no valid prediction windows")
    voice_score = sum(probabilities) / len(probabilities)
    vocal_coverage = sum(value >= 0.5 for value in probabilities) / len(probabilities)
    return voice_score, vocal_coverage, len(probabilities)


def _classification_note(voice_score: float, vocal_coverage: float) -> str:
    if voice_score >= 0.65 and vocal_coverage >= 0.6:
        label = "Voice is present across most analyzed windows."
    elif voice_score >= 0.55 and vocal_coverage >= 0.2:
        label = "Voice is present in part of the recording."
    elif voice_score <= 0.35 and vocal_coverage <= 0.2:
        label = "The recording is predominantly instrumental."
    else:
        label = "The classifier found mixed or uncertain voice evidence."
    return (
        f"{label} Mean normalized voice score {voice_score:.0%}; "
        f"voice-leading window coverage {vocal_coverage:.0%}."
    )


@lru_cache(maxsize=2)
def _load_predictor(model_path: str, model_hash: str) -> tuple[Callable[..., Any], Any]:
    del model_hash
    try:
        from essentia.standard import (  # type: ignore[import-not-found]
            MonoLoader,
            TensorflowPredictMusiCNN,
        )
    except (ImportError, OSError) as exc:
        raise _VoiceRuntimeUnavailable from exc

    predictor = TensorflowPredictMusiCNN(
        graphFilename=model_path,
        output="model/Sigmoid",
    )
    return MonoLoader, predictor


def _run_essentia_model(path: Path, model_path: Path, model_hash: str) -> object:
    mono_loader, predictor = _load_predictor(str(model_path.resolve()), model_hash)
    audio = mono_loader(filename=str(path), sampleRate=16_000, resampleQuality=4)()
    return predictor(audio)


def analyze_voice(
    path: Path,
    *,
    check_cancelled: Callable[[], None] | None = None,
) -> VoiceAnalysis:
    """Run the configured optional classifier and return bounded factual evidence."""

    model_path = get_settings().assistant_voice_model_path
    if model_path is None:
        return _not_classified()
    if not model_path.is_file():
        return _unavailable(
            "model_missing",
            (
                f"{VOICE_MODEL_FILENAME} is missing from the configured model mount. "
                "Install the checksum-pinned model, then rebuild this track."
            ),
        )
    try:
        model_hash = _model_file_hash(model_path)
    except OSError as exc:
        return _unavailable(
            "model_unreadable",
            "The configured local voice-classifier model could not be read.",
            error_type=type(exc).__name__,
        )
    if model_hash != VOICE_MODEL_SHA256:
        return _unavailable(
            "unsupported_model",
            "The configured voice model does not match the supported classifier checksum.",
        )
    cancellation_check = check_cancelled or (lambda: None)
    cancellation_check()
    try:
        predictions = _run_essentia_model(path, model_path, model_hash)
        voice_score, vocal_coverage, window_count = _summarize_predictions(predictions)
    except _VoiceRuntimeUnavailable as exc:
        cause = exc.__cause__
        return _unavailable(
            "runtime_missing",
            "The supported voice model is configured, but the optional Essentia runtime is missing.",
            error_type=type(cause).__name__ if cause is not None else None,
        )
    except Exception as exc:  # Optional stage: retain the rest of the factual context.
        return _unavailable(
            "inference_failed",
            "The local voice classifier failed; the remaining track context is still available.",
            error_type=type(exc).__name__,
        )
    cancellation_check()
    return VoiceAnalysis(
        summary={
            "status": "classified",
            # Keep the stored/wire key across context versions. Its value is a
            # normalized model score, not a calibrated real-world probability.
            "voice_probability": round(voice_score, 5),
            "vocal_coverage": round(vocal_coverage, 5),
            "note": _classification_note(voice_score, vocal_coverage),
        },
        stage={
            "status": "complete",
            "required": False,
            "analyzer_id": VOICE_ANALYZER_ID,
            "model_sha256": model_hash,
            "prediction_windows": window_count,
            "classes": ["instrumental", "voice"],
        },
    )
