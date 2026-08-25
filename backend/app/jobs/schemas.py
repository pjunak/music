import json
from datetime import datetime
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field

from app.models.background_job import BackgroundJob

JobStatus = Literal[
    "queued",
    "running",
    "cancel_requested",
    "succeeded",
    "failed",
    "cancelled",
]


class StrictJobModel(BaseModel):
    model_config = ConfigDict(extra="forbid")


class BackgroundJobOut(StrictJobModel):
    id: str
    kind: str
    status: JobStatus
    parameters: dict[str, Any]
    result: dict[str, Any] | None
    error: str | None
    progress_current: int = Field(ge=0)
    progress_total: int | None = Field(default=None, ge=0)
    progress_phase: str
    progress_message: str
    attempts: int = Field(ge=0)
    retry_of_id: str | None
    created_at: datetime
    updated_at: datetime
    started_at: datetime | None
    finished_at: datetime | None


def _json_object(value: str | None) -> dict[str, Any] | None:
    if value is None:
        return None
    parsed = json.loads(value)
    if not isinstance(parsed, dict):
        raise ValueError("stored job JSON must be an object")
    return parsed


def job_out(job: BackgroundJob) -> BackgroundJobOut:
    return BackgroundJobOut(
        id=job.id,
        kind=job.kind,
        status=job.status,  # type: ignore[arg-type]
        parameters=_json_object(job.parameters_json) or {},
        result=_json_object(job.result_json),
        error=job.error,
        progress_current=job.progress_current,
        progress_total=job.progress_total,
        progress_phase=job.progress_phase,
        progress_message=job.progress_message,
        attempts=job.attempts,
        retry_of_id=job.retry_of_id,
        created_at=job.created_at,
        updated_at=job.updated_at,
        started_at=job.started_at,
        finished_at=job.finished_at,
    )
