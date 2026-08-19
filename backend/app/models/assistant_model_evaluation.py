from datetime import datetime

from sqlalchemy import ForeignKey, Integer, String
from sqlalchemy.orm import Mapped, mapped_column

from app.models.base import Base, UtcDateTime, utcnow


class AssistantModelEvaluation(Base):
    """Current quality result for one task-specific model evaluation."""

    __tablename__ = "assistant_model_evaluations"

    role_id: Mapped[str] = mapped_column(
        ForeignKey("assistant_model_roles.role_id", ondelete="CASCADE"),
        primary_key=True,
    )
    evaluation_id: Mapped[str] = mapped_column(String(128), primary_key=True)
    role_fingerprint: Mapped[str] = mapped_column(String(64), nullable=False)
    status: Mapped[str] = mapped_column(String(16), nullable=False)
    suite_id: Mapped[str] = mapped_column(String(64), nullable=False)
    engine_id: Mapped[str] = mapped_column(String(128), nullable=False)
    passed_cases: Mapped[int] = mapped_column(Integer, nullable=False)
    total_cases: Mapped[int] = mapped_column(Integer, nullable=False)
    job_id: Mapped[str] = mapped_column(
        ForeignKey("background_jobs.id", ondelete="RESTRICT"),
        nullable=False,
        index=True,
    )
    evaluated_at: Mapped[datetime] = mapped_column(
        UtcDateTime, default=utcnow, nullable=False
    )
