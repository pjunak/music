from __future__ import annotations

import os
import struct
import time
import wave
from pathlib import Path

import pytest

from app.assistant.context_workers import (
    ContextAnalysisTask,
    _check_worker_cancelled,
    _process_map_unordered,
    analyze_tracks_in_processes,
)


def _timed_probe(value: int) -> tuple[int, float, float, int]:
    started = time.monotonic()
    time.sleep(0.4)
    return os.getpid(), started, time.monotonic(), value


def _cancellable_probe(value: int) -> int:
    while True:
        _check_worker_cancelled()
        time.sleep(0.02)
    return value


class _ProbeCancelled(Exception):
    pass


def _write_probe_wav(path: Path) -> None:
    sample_rate = 8_000
    samples = [round(8_000 * ((index % 32) / 16.0 - 1.0)) for index in range(sample_rate)]
    with wave.open(str(path), "wb") as output:
        output.setnchannels(1)
        output.setsampwidth(2)
        output.setframerate(sample_rate)
        output.writeframes(struct.pack(f"<{len(samples)}h", *samples))


def test_process_map_runs_work_on_two_overlapping_processes() -> None:
    results = list(
        _process_map_unordered(
            _timed_probe,
            [1, 2, 3, 4],
            max_workers=2,
            check_cancelled=lambda: None,
        )
    )

    assert sorted(result[3] for result in results) == [1, 2, 3, 4]
    assert len({result[0] for result in results}) == 2
    assert any(
        left[0] != right[0]
        and min(left[2], right[2]) > max(left[1], right[1])
        for left in results
        for right in results
    )


def test_process_map_signals_workers_before_waiting_for_shutdown() -> None:
    checks = 0

    def check_cancelled() -> None:
        nonlocal checks
        checks += 1
        if checks >= 2:
            raise _ProbeCancelled

    started = time.monotonic()
    with pytest.raises(_ProbeCancelled):
        list(
            _process_map_unordered(
                _cancellable_probe,
                [1, 2],
                max_workers=2,
                check_cancelled=check_cancelled,
            )
        )
    assert time.monotonic() - started < 5


def test_audio_context_documents_cross_the_process_boundary(tmp_path: Path) -> None:
    first = tmp_path / "first.wav"
    second = tmp_path / "second.wav"
    _write_probe_wav(first)
    _write_probe_wav(second)

    results = list(
        analyze_tracks_in_processes(
            [
                ContextAnalysisTask(track_id=1, path=str(first)),
                ContextAnalysisTask(track_id=2, path=str(second)),
            ],
            max_workers=2,
            check_cancelled=lambda: None,
        )
    )

    assert {result.track_id for result in results} == {1, 2}
    assert all(result.document is not None for result in results)
    assert all(result.error is None and not result.fatal for result in results)
    assert all(
        result.document is not None and result.document.performance is not None
        for result in results
    )
