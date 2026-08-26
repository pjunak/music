"""Bounded process workers for CPU-heavy whole-track context analysis."""

import multiprocessing
import time
from collections.abc import Callable, Iterator, Sequence
from concurrent.futures import FIRST_COMPLETED, Future, ProcessPoolExecutor, wait
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

from app.assistant.audio_context import AudioContextDocument, analyze_audio_context
from app.assistant.audio_signal import AudioSignalError
from app.assistant.voice_analysis import VoiceAnalysis, analyze_voice

_POLL_SECONDS = 0.25

class _CancelEvent(Protocol):
    def is_set(self) -> bool: ...

    def set(self) -> None: ...


_worker_cancel_event: _CancelEvent | None = None


class _WorkerAnalysisCancelled(Exception):
    pass


@dataclass(frozen=True)
class ContextAnalysisTask:
    track_id: int
    path: str
    include_voice: bool = True


@dataclass(frozen=True)
class ContextAnalysisResult:
    track_id: int
    document: AudioContextDocument | None = None
    error: str | None = None
    fatal: bool = False


@dataclass(frozen=True)
class VoiceAnalysisTask:
    track_id: int
    path: str


@dataclass(frozen=True)
class VoiceAnalysisResult:
    track_id: int
    analysis: VoiceAnalysis | None = None
    elapsed_seconds: float = 0.0
    error: str | None = None
    fatal: bool = False


def _initialize_worker(cancel_event: _CancelEvent) -> None:
    global _worker_cancel_event
    _worker_cancel_event = cancel_event


def _check_worker_cancelled() -> None:
    if _worker_cancel_event is not None and _worker_cancel_event.is_set():
        raise _WorkerAnalysisCancelled


def _analyze_track(task: ContextAnalysisTask) -> ContextAnalysisResult:
    try:
        document = analyze_audio_context(
            Path(task.path),
            check_cancelled=_check_worker_cancelled,
            include_voice=task.include_voice,
        )
    except _WorkerAnalysisCancelled:
        raise
    except (AudioSignalError, OSError) as exc:
        return ContextAnalysisResult(
            track_id=task.track_id,
            error=f"{type(exc).__name__}: {exc}",
        )
    except Exception as exc:
        return ContextAnalysisResult(
            track_id=task.track_id,
            error=f"{type(exc).__name__}: {exc}".strip()[:2_000],
            fatal=True,
        )
    return ContextAnalysisResult(track_id=task.track_id, document=document)


def _analyze_voice_track(task: VoiceAnalysisTask) -> VoiceAnalysisResult:
    started = time.perf_counter()
    try:
        analysis = analyze_voice(
            Path(task.path),
            check_cancelled=_check_worker_cancelled,
        )
    except _WorkerAnalysisCancelled:
        raise
    except Exception as exc:
        return VoiceAnalysisResult(
            track_id=task.track_id,
            elapsed_seconds=time.perf_counter() - started,
            error=f"{type(exc).__name__}: {exc}".strip()[:2_000],
            fatal=True,
        )
    return VoiceAnalysisResult(
        track_id=task.track_id,
        analysis=analysis,
        elapsed_seconds=time.perf_counter() - started,
    )


def _process_map_unordered[TaskT, ResultT](
    worker: Callable[[TaskT], ResultT],
    tasks: Sequence[TaskT],
    *,
    max_workers: int,
    max_tasks_per_worker: int | None = None,
    check_cancelled: Callable[[], None],
) -> Iterator[ResultT]:
    """Run a bounded spawn-based process map and propagate cancellation.

    Only one task per worker is submitted at a time. This bounds queued work,
    makes cancellation responsive, and avoids forking the already-threaded
    FastAPI process. Workers receive a shared event so the audio decoder can
    stop cooperatively while the parent retains ownership of all persistence.
    Native analyzers may retain allocations between tracks, so callers can
    recycle a worker after a bounded number of completed tasks.
    """

    if max_workers < 1:
        raise ValueError("max_workers must be at least one")
    if max_tasks_per_worker is not None and max_tasks_per_worker < 1:
        raise ValueError("max_tasks_per_worker must be at least one")
    if not tasks:
        return

    process_context = multiprocessing.get_context("spawn")
    cancel_event = process_context.Event()
    executor = ProcessPoolExecutor(
        max_workers=min(max_workers, len(tasks)),
        mp_context=process_context,
        initializer=_initialize_worker,
        initargs=(cancel_event,),
        max_tasks_per_child=max_tasks_per_worker,
    )
    pending: dict[Future[ResultT], None] = {}
    task_iterator = iter(tasks)

    def fill_workers() -> None:
        while len(pending) < max_workers:
            try:
                task = next(task_iterator)
            except StopIteration:
                return
            pending[executor.submit(worker, task)] = None

    try:
        check_cancelled()
        fill_workers()
        while pending:
            done, _ = wait(
                pending,
                timeout=_POLL_SECONDS,
                return_when=FIRST_COMPLETED,
            )
            check_cancelled()
            for future in done:
                del pending[future]
                yield future.result()
            fill_workers()
    finally:
        if pending:
            cancel_event.set()
            for future in pending:
                future.cancel()
        executor.shutdown(wait=True, cancel_futures=True)


def analyze_tracks_in_processes(
    tasks: Sequence[ContextAnalysisTask],
    *,
    max_workers: int,
    max_tasks_per_worker: int | None = None,
    check_cancelled: Callable[[], None],
) -> Iterator[ContextAnalysisResult]:
    yield from _process_map_unordered(
        _analyze_track,
        tasks,
        max_workers=max_workers,
        max_tasks_per_worker=max_tasks_per_worker,
        check_cancelled=check_cancelled,
    )


def analyze_voice_tracks_in_processes(
    tasks: Sequence[VoiceAnalysisTask],
    *,
    max_workers: int,
    max_tasks_per_worker: int | None = None,
    check_cancelled: Callable[[], None],
) -> Iterator[VoiceAnalysisResult]:
    yield from _process_map_unordered(
        _analyze_voice_track,
        tasks,
        max_workers=max_workers,
        max_tasks_per_worker=max_tasks_per_worker,
        check_cancelled=check_cancelled,
    )
