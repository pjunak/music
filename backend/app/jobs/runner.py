import asyncio
import json
import logging
import threading
from contextlib import suppress
from typing import Any

from sqlalchemy import CursorResult, select, update

from app.core.db import SessionLocal
from app.jobs.registry import JobExecutionContext, JobLane, get_job_handler, job_lane_for_kind
from app.models.background_job import BackgroundJob
from app.models.base import utcnow

logger = logging.getLogger(__name__)


class JobCancelled(Exception):
    pass


class JobRunnerStopping(Exception):
    pass


class JobContext(JobExecutionContext):
    def __init__(
        self,
        job_id: str,
        stopping: threading.Event,
        progress_current: int,
        progress_total: int | None,
    ) -> None:
        self.job_id = job_id
        self._stopping = stopping
        self.progress_current = progress_current
        self.progress_total = progress_total

    def update_progress(
        self,
        current: int,
        total: int | None = None,
        *,
        phase: str = "",
        message: str = "",
    ) -> None:
        if current < 0 or (total is not None and (total < 0 or current > total)):
            raise ValueError("job progress must satisfy 0 <= current <= total")
        with SessionLocal() as db:
            job = db.get(BackgroundJob, self.job_id)
            if job is None:
                raise JobCancelled("job record was removed")
            job.progress_current = current
            job.progress_total = total
            job.progress_phase = phase[:128]
            job.progress_message = message[:512]
            job.updated_at = utcnow()
            cancellation_requested = job.status == "cancel_requested"
            db.commit()
        self.progress_current = current
        self.progress_total = total
        if cancellation_requested:
            raise JobCancelled
        if self._stopping.is_set():
            raise JobRunnerStopping

    def check_cancelled(self) -> None:
        with SessionLocal() as db:
            job = db.get(BackgroundJob, self.job_id)
            if job is None or job.status == "cancel_requested":
                raise JobCancelled
        if self._stopping.is_set():
            raise JobRunnerStopping

    def checkpoint_result(self, result: dict[str, Any]) -> None:
        serialized = json.dumps(result, ensure_ascii=False, separators=(",", ":"))
        with SessionLocal() as db:
            job = db.get(BackgroundJob, self.job_id)
            if job is None:
                raise JobCancelled("job record was removed")
            job.result_json = serialized
            job.updated_at = utcnow()
            cancellation_requested = job.status == "cancel_requested"
            db.commit()
        if cancellation_requested:
            raise JobCancelled
        if self._stopping.is_set():
            raise JobRunnerStopping


class BackgroundJobRunner:
    """Separate cooperative workers for local and provider-owned jobs.

    Handlers execute in worker threads so filesystem, CPU, and network work
    cannot block FastAPI's event loop. Local jobs remain serialized so
    whole-library scans do not compete for disk and SQLite writes. Provider
    jobs use a second serialized lane so a bounded remote timeout cannot stall
    unrelated local analysis.
    """

    def __init__(self) -> None:
        self._tasks: dict[JobLane, asyncio.Task[None]] = {}
        self._loop: asyncio.AbstractEventLoop | None = None
        self._wake_events: dict[JobLane, asyncio.Event] = {}
        self._stopping = threading.Event()

    async def start(self) -> None:
        if any(not task.done() for task in self._tasks.values()):
            return
        self._loop = asyncio.get_running_loop()
        self._wake_events = {"local": asyncio.Event(), "provider": asyncio.Event()}
        self._stopping = threading.Event()
        await asyncio.to_thread(self.recover_interrupted_jobs)
        self._tasks = {
            lane: self._loop.create_task(
                self._run(lane),
                name=f"background-jobs-{lane}",
            )
            for lane in ("local", "provider")
        }

    async def stop(self) -> None:
        tasks = list(self._tasks.values())
        if not tasks:
            return
        self._stopping.set()
        self.wake()
        try:
            await asyncio.wait_for(asyncio.gather(*tasks), timeout=10)
        except TimeoutError:
            logger.error("background job runner did not stop cooperatively")
            for task in tasks:
                task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)
        finally:
            self._tasks = {}
            self._loop = None
            self._wake_events = {}

    def wake(self, lane: JobLane | None = None) -> None:
        loop = self._loop
        if loop is None or loop.is_closed():
            return
        events = (
            [self._wake_events[lane]]
            if lane is not None and lane in self._wake_events
            else list(self._wake_events.values())
        )
        for event in events:
            loop.call_soon_threadsafe(event.set)

    def recover_interrupted_jobs(self) -> None:
        now = utcnow()
        with SessionLocal() as db:
            jobs = list(
                db.scalars(
                    select(BackgroundJob).where(
                        BackgroundJob.status.in_(("running", "cancel_requested"))
                    )
                ).all()
            )
            for job in jobs:
                if job.status == "cancel_requested":
                    job.status = "cancelled"
                    job.progress_phase = "Cancelled"
                    job.finished_at = now
                else:
                    registration = get_job_handler(job.kind)
                    if registration is not None and registration.restartable:
                        job.status = "queued"
                        job.progress_phase = "Queued after server restart"
                        job.started_at = None
                    else:
                        job.status = "failed"
                        job.error = "Job was interrupted by a server restart."
                        job.progress_phase = "Interrupted"
                        job.finished_at = now
                job.updated_at = now
            db.commit()

    async def _run(self, lane: JobLane) -> None:
        wake_event = self._wake_events[lane]
        while not self._stopping.is_set():
            wake_event.clear()
            job_id = await asyncio.to_thread(self._claim_next, lane)
            if job_id is None:
                with suppress(TimeoutError):
                    await asyncio.wait_for(wake_event.wait(), timeout=5)
                continue
            await asyncio.to_thread(self._execute, job_id)

    def _claim_next(self, lane: JobLane) -> str | None:
        with SessionLocal() as db:
            while True:
                queued = db.execute(
                    select(BackgroundJob.id, BackgroundJob.kind)
                    .where(BackgroundJob.status == "queued")
                    .order_by(BackgroundJob.created_at, BackgroundJob.id)
                ).all()
                candidates = [
                    job_id
                    for job_id, kind in queued
                    if job_lane_for_kind(kind) == lane
                ]
                if not candidates:
                    return None
                for job_id in candidates:
                    now = utcnow()
                    claimed = db.execute(
                        update(BackgroundJob)
                        .where(
                            BackgroundJob.id == job_id,
                            BackgroundJob.status == "queued",
                        )
                        .values(
                            status="running",
                            started_at=now,
                            finished_at=None,
                            error=None,
                            progress_phase="Starting",
                            updated_at=now,
                            attempts=BackgroundJob.attempts + 1,
                        )
                    )
                    db.commit()
                    if isinstance(claimed, CursorResult) and claimed.rowcount == 1:
                        return job_id

    def _execute(self, job_id: str) -> None:
        with SessionLocal() as db:
            job = db.get(BackgroundJob, job_id)
            if job is None:
                return
            registration = get_job_handler(job.kind)
            progress_current = job.progress_current
            progress_total = job.progress_total
            try:
                parameters = json.loads(job.parameters_json)
            except (TypeError, json.JSONDecodeError):
                parameters = None

        if registration is None:
            self._finish_failed(job_id, "No handler is registered for this job type.")
            return
        if not isinstance(parameters, dict):
            self._finish_failed(job_id, "Stored job parameters are invalid.")
            return

        context = JobContext(
            job_id,
            self._stopping,
            progress_current,
            progress_total,
        )
        try:
            result = registration.handler(context, parameters)
            context.check_cancelled()
        except JobCancelled:
            self._finish_cancelled(job_id)
        except JobRunnerStopping:
            self._finish_interrupted(job_id, restartable=registration.restartable)
        except Exception as exc:
            logger.exception("background job %s (%s) failed", job_id, registration.kind)
            detail = f"{type(exc).__name__}: {exc}".strip()[:2000]
            self._finish_failed(job_id, detail)
        else:
            self._finish_succeeded(job_id, result or {})

    def _finish_succeeded(self, job_id: str, result: dict[str, Any]) -> None:
        with SessionLocal() as db:
            job = db.get(BackgroundJob, job_id)
            if job is None:
                return
            now = utcnow()
            if job.status == "cancel_requested":
                job.status = "cancelled"
                job.progress_phase = "Cancelled"
                job.updated_at = now
                job.finished_at = now
                db.commit()
                return
            job.status = "succeeded"
            job.result_json = json.dumps(
                result, ensure_ascii=False, separators=(",", ":")
            )
            job.progress_phase = "Complete"
            job.error = None
            job.updated_at = now
            job.finished_at = now
            db.commit()

    def _finish_failed(self, job_id: str, error: str) -> None:
        with SessionLocal() as db:
            job = db.get(BackgroundJob, job_id)
            if job is None:
                return
            now = utcnow()
            if job.status == "cancel_requested":
                job.status = "cancelled"
                job.progress_phase = "Cancelled"
                job.updated_at = now
                job.finished_at = now
                db.commit()
                return
            job.status = "failed"
            job.error = error
            job.progress_phase = "Failed"
            job.updated_at = now
            job.finished_at = now
            db.commit()

    def _finish_cancelled(self, job_id: str) -> None:
        with SessionLocal() as db:
            job = db.get(BackgroundJob, job_id)
            if job is None:
                return
            now = utcnow()
            job.status = "cancelled"
            job.progress_phase = "Cancelled"
            job.updated_at = now
            job.finished_at = now
            db.commit()

    def _finish_interrupted(self, job_id: str, *, restartable: bool) -> None:
        with SessionLocal() as db:
            job = db.get(BackgroundJob, job_id)
            if job is None:
                return
            now = utcnow()
            if job.status == "cancel_requested":
                job.status = "cancelled"
                job.progress_phase = "Cancelled"
                job.finished_at = now
            elif restartable:
                job.status = "queued"
                job.progress_phase = "Queued for restart"
                job.started_at = None
            else:
                job.status = "failed"
                job.error = "Job was interrupted during server shutdown."
                job.progress_phase = "Interrupted"
                job.finished_at = now
            job.updated_at = now
            db.commit()


job_runner = BackgroundJobRunner()
