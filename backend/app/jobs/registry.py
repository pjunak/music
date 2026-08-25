from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from typing import Any, Literal, Protocol


class JobExecutionContext(Protocol):
    job_id: str
    progress_current: int
    progress_total: int | None

    def update_progress(
        self,
        current: int,
        total: int | None = None,
        *,
        phase: str = "",
        message: str = "",
    ) -> None: ...

    def checkpoint_result(self, result: dict[str, Any]) -> None: ...

    def check_cancelled(self) -> None: ...


JobHandler = Callable[[JobExecutionContext, dict[str, Any]], dict[str, Any] | None]
JobLane = Literal["local", "provider"]


@dataclass(frozen=True)
class JobRegistration:
    kind: str
    handler: JobHandler
    restartable: bool
    lane: JobLane


_REGISTRY: dict[str, JobRegistration] = {}


def register_job_handler(
    kind: str,
    handler: JobHandler,
    *,
    restartable: bool,
    lane: JobLane | None = None,
) -> None:
    if not kind or len(kind) > 128:
        raise ValueError("job kind must contain between 1 and 128 characters")
    existing = _REGISTRY.get(kind)
    if existing is not None and existing.handler is not handler:
        raise ValueError(f"job handler already registered for {kind!r}")
    _REGISTRY[kind] = JobRegistration(
        kind=kind,
        handler=handler,
        restartable=restartable,
        lane=lane or ("provider" if kind.startswith("assistant.model") else "local"),
    )


def get_job_handler(kind: str) -> JobRegistration | None:
    return _REGISTRY.get(kind)


def require_job_handler(kind: str) -> JobRegistration:
    registration = get_job_handler(kind)
    if registration is None:
        raise ValueError(f"no background job handler registered for {kind!r}")
    return registration


def job_lane_for_kind(kind: str) -> JobLane:
    registration = get_job_handler(kind)
    return registration.lane if registration is not None else "local"
