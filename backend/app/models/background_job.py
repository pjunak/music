from datetime import datetime

from sqlalchemy import Integer, String, Text
from sqlalchemy.orm import Mapped, mapped_column

from app.models.base import Base, UtcDateTime, utcnow


class BackgroundJob(Base):
    """Durable record for server-owned work that outlives an HTTP request.

    Parameters and results are JSON documents owned by the registered job
    handler. Progress and lifecycle fields stay generic so the same runner can
    support library analysis, cleanup assistance, and future model calls.
    """

    __tablename__ = "background_jobs"

    id: Mapped[str] = mapped_column(String(32), primary_key=True)
    kind: Mapped[str] = mapped_column(String(128), nullable=False, index=True)
    status: Mapped[str] = mapped_column(String(32), nullable=False, index=True)
    parameters_json: Mapped[str] = mapped_column(Text, nullable=False, default="{}")
    result_json: Mapped[str | None] = mapped_column(Text, nullable=True)
    error: Mapped[str | None] = mapped_column(Text, nullable=True)
    progress_current: Mapped[int] = mapped_column(Integer, nullable=False, default=0)
    progress_total: Mapped[int | None] = mapped_column(Integer, nullable=True)
    progress_phase: Mapped[str] = mapped_column(String(128), nullable=False, default="")
    progress_message: Mapped[str] = mapped_column(String(512), nullable=False, default="")
    attempts: Mapped[int] = mapped_column(Integer, nullable=False, default=0)
    retry_of_id: Mapped[str | None] = mapped_column(String(32), nullable=True, index=True)
    created_at: Mapped[datetime] = mapped_column(
        UtcDateTime, default=utcnow, nullable=False, index=True
    )
    updated_at: Mapped[datetime] = mapped_column(
        UtcDateTime, default=utcnow, nullable=False
    )
    started_at: Mapped[datetime | None] = mapped_column(UtcDateTime, nullable=True)
    finished_at: Mapped[datetime | None] = mapped_column(UtcDateTime, nullable=True)
