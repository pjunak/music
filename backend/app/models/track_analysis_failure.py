from datetime import datetime

from sqlalchemy import ForeignKey, String, Text
from sqlalchemy.orm import Mapped, mapped_column

from app.models.base import Base, UtcDateTime, utcnow


class TrackAnalysisFailure(Base):
    """Latest failed attempt for one track and versioned analyzer.

    Failures are separate from usable profiles so a decoder error can be
    checkpointed without manufacturing zero-valued signal data. A later
    successful attempt removes the row.
    """

    __tablename__ = "track_analysis_failures"

    track_id: Mapped[int] = mapped_column(
        ForeignKey("tracks.id", ondelete="CASCADE"), primary_key=True
    )
    analyzer_id: Mapped[str] = mapped_column(String(128), primary_key=True)
    source_signature: Mapped[str] = mapped_column(String(64), nullable=False)
    job_id: Mapped[str] = mapped_column(String(32), nullable=False, index=True)
    error: Mapped[str] = mapped_column(Text, nullable=False)
    updated_at: Mapped[datetime] = mapped_column(
        UtcDateTime, default=utcnow, nullable=False
    )
