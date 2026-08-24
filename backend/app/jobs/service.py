from __future__ import annotations

import json
import threading
from typing import Any, cast
from uuid import uuid4

from sqlalchemy import CursorResult, select, update
from sqlalchemy.orm import Session

from app.jobs.registry import require_job_handler
from app.models.background_job import BackgroundJob
from app.models.base import utcnow

ACTIVE_JOB_STATUSES = ("queued", "running", "cancel_requested")
_enqueue_lock = threading.Lock()


def enqueue_job(
    db: Session,
    kind: str,
    parameters: dict[str, Any],
    *,
    retry_of_id: str | None = None,
) -> BackgroundJob:
    require_job_handler(kind)
    serialized = json.dumps(parameters, ensure_ascii=False, separators=(",", ":"))
    job = BackgroundJob(
        id=uuid4().hex,
        kind=kind,
        status="queued",
        parameters_json=serialized,
        retry_of_id=retry_of_id,
        progress_phase="Queued",
    )
    db.add(job)
    db.commit()
    db.refresh(job)
    return job


def find_active_job(db: Session, kind: str) -> BackgroundJob | None:
    return db.scalar(
        select(BackgroundJob)
        .where(
            BackgroundJob.kind == kind,
            BackgroundJob.status.in_(ACTIVE_JOB_STATUSES),
        )
        .order_by(BackgroundJob.created_at.desc())
    )


def enqueue_unique_active_job(
    db: Session,
    kind: str,
    parameters: dict[str, Any],
) -> tuple[BackgroundJob, bool]:
    """Return the active job of this kind, or atomically enqueue one.

    The product runs as one FastAPI process; this lock closes the only race
    between simultaneous operator requests without introducing a second queue.
    """

    with _enqueue_lock:
        active = find_active_job(db, kind)
        if active is not None:
            return active, False
        return enqueue_job(db, kind, parameters), True


def request_cancellation(
    db: Session, job_id: str
) -> tuple[BackgroundJob | None, bool]:
    """Atomically cancel a queued job or signal a running handler."""

    job = db.get(BackgroundJob, job_id)
    if job is None:
        return None, False
    now = utcnow()
    queued = cast(
        "CursorResult[Any]",
        db.execute(
            update(BackgroundJob)
            .where(
                BackgroundJob.id == job_id,
                BackgroundJob.status == "queued",
            )
            .values(
                status="cancelled",
                progress_phase="Cancelled",
                finished_at=now,
                updated_at=now,
            )
        ),
    )
    changed = queued.rowcount == 1
    if not changed:
        running = cast(
            "CursorResult[Any]",
            db.execute(
                update(BackgroundJob)
                .where(
                    BackgroundJob.id == job_id,
                    BackgroundJob.status == "running",
                )
                .values(
                    status="cancel_requested",
                    progress_phase="Cancelling",
                    updated_at=now,
                )
            ),
        )
        changed = running.rowcount == 1
    db.commit()
    return db.get(BackgroundJob, job_id, populate_existing=True), changed
