from typing import Annotated

from fastapi import APIRouter, HTTPException, Query, status
from sqlalchemy import select

from app.api.deps import CurrentUser, DbSession
from app.jobs.registry import get_job_handler
from app.jobs.runner import job_runner
from app.jobs.schemas import BackgroundJobOut, JobStatus, job_out
from app.jobs.service import enqueue_job, request_cancellation
from app.models.background_job import BackgroundJob

router = APIRouter(prefix="/api/jobs", tags=["jobs"])


def _get_job(db: DbSession, job_id: str) -> BackgroundJob:
    job = db.get(BackgroundJob, job_id)
    if job is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="job not found")
    return job


@router.get("", response_model=list[BackgroundJobOut])
def list_jobs(
    _user: CurrentUser,
    db: DbSession,
    kind: Annotated[str | None, Query(max_length=128)] = None,
    job_status: Annotated[JobStatus | None, Query(alias="status")] = None,
    limit: Annotated[int, Query(ge=1, le=100)] = 25,
) -> list[BackgroundJobOut]:
    query = select(BackgroundJob)
    if kind is not None:
        query = query.where(BackgroundJob.kind == kind)
    if job_status is not None:
        query = query.where(BackgroundJob.status == job_status)
    jobs = db.scalars(
        query.order_by(BackgroundJob.created_at.desc(), BackgroundJob.id.desc()).limit(
            limit
        )
    ).all()
    return [job_out(job) for job in jobs]


@router.get("/{job_id}", response_model=BackgroundJobOut)
def get_job(job_id: str, _user: CurrentUser, db: DbSession) -> BackgroundJobOut:
    return job_out(_get_job(db, job_id))


@router.post("/{job_id}/cancel", response_model=BackgroundJobOut)
def cancel_job(job_id: str, _user: CurrentUser, db: DbSession) -> BackgroundJobOut:
    job, changed = request_cancellation(db, job_id)
    if job is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="job not found")
    if not changed:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=f"job is already {job.status}",
        )
    job_runner.wake()
    return job_out(job)


@router.post("/{job_id}/retry", response_model=BackgroundJobOut)
def retry_job(job_id: str, _user: CurrentUser, db: DbSession) -> BackgroundJobOut:
    previous = _get_job(db, job_id)
    if previous.status not in {"failed", "cancelled"}:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail="only failed or cancelled jobs can be retried",
        )
    if get_job_handler(previous.kind) is None:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail="the job type is no longer available",
        )
    parameters = job_out(previous).parameters
    job = enqueue_job(db, previous.kind, parameters, retry_of_id=previous.id)
    job_runner.wake()
    return job_out(job)
