from datetime import datetime

from sqlalchemy import Float, ForeignKey, String, Text
from sqlalchemy.orm import Mapped, mapped_column

from app.models.base import Base, UtcDateTime, utcnow


class TrackAnalysis(Base):
    """Latest durable analysis profile for one indexed track.

    The analyzer ID versions the meaning of the numeric axes. A source
    signature detects metadata changes, while `job_id` makes force-runs
    restartable without repeating rows already committed by the same job.
    """

    __tablename__ = "track_analyses"

    track_id: Mapped[int] = mapped_column(
        ForeignKey("tracks.id", ondelete="CASCADE"), primary_key=True
    )
    analyzer_id: Mapped[str] = mapped_column(String(128), primary_key=True)
    source_signature: Mapped[str] = mapped_column(String(64), nullable=False)
    job_id: Mapped[str] = mapped_column(String(32), nullable=False, index=True)
    energy: Mapped[float] = mapped_column(Float, nullable=False)
    brightness: Mapped[float] = mapped_column(Float, nullable=False)
    tension: Mapped[float] = mapped_column(Float, nullable=False)
    moods_json: Mapped[str] = mapped_column(Text, nullable=False, default="[]")
    evidence_json: Mapped[str] = mapped_column(Text, nullable=False, default="[]")
    metrics_json: Mapped[str] = mapped_column(Text, nullable=False, default="{}")
    confidence: Mapped[str] = mapped_column(String(16), nullable=False)
    updated_at: Mapped[datetime] = mapped_column(
        UtcDateTime, default=utcnow, nullable=False
    )
