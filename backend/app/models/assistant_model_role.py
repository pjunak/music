from datetime import datetime

from sqlalchemy import Boolean, ForeignKey, Integer, String
from sqlalchemy.orm import Mapped, mapped_column

from app.models.base import Base, UtcDateTime, utcnow


class AssistantModelRole(Base):
    """One optional task role mapped to a verified connection and model."""

    __tablename__ = "assistant_model_roles"

    role_id: Mapped[str] = mapped_column(String(64), primary_key=True)
    connection_id: Mapped[str] = mapped_column(
        ForeignKey("assistant_provider_connections.id", ondelete="RESTRICT"),
        nullable=False,
        index=True,
    )
    model_id: Mapped[str] = mapped_column(String(256), nullable=False)
    enabled: Mapped[bool] = mapped_column(Boolean, nullable=False, default=False)
    timeout_seconds: Mapped[int] = mapped_column(Integer, nullable=False, default=30)
    max_output_tokens: Mapped[int] = mapped_column(Integer, nullable=False, default=2_000)
    updated_at: Mapped[datetime] = mapped_column(
        UtcDateTime, default=utcnow, onupdate=utcnow, nullable=False
    )
