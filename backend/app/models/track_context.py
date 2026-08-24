from datetime import datetime

from sqlalchemy import ForeignKey, String, Text
from sqlalchemy.orm import Mapped, mapped_column

from app.models.base import Base, UtcDateTime, utcnow


class TrackContext(Base):
    """Latest factual, provider-neutral context for one indexed recording.

    Context is deliberately stored outside ``track_analyses``: it describes
    measured audio and never contains suggested mood tags.  The analyzer ID
    versions the complete document contract while the source signature binds
    it to the indexed file identity.
    """

    __tablename__ = "track_contexts"

    track_id: Mapped[int] = mapped_column(
        ForeignKey("tracks.id", ondelete="CASCADE"), primary_key=True
    )
    analyzer_id: Mapped[str] = mapped_column(String(128), primary_key=True)
    source_signature: Mapped[str] = mapped_column(String(64), nullable=False)
    job_id: Mapped[str] = mapped_column(String(32), nullable=False, index=True)
    completeness: Mapped[str] = mapped_column(String(16), nullable=False)
    confidence: Mapped[str] = mapped_column(String(16), nullable=False)
    summary_json: Mapped[str] = mapped_column(Text, nullable=False)
    timeline_json: Mapped[str] = mapped_column(Text, nullable=False)
    sections_json: Mapped[str] = mapped_column(Text, nullable=False)
    technical_json: Mapped[str] = mapped_column(Text, nullable=False)
    stages_json: Mapped[str] = mapped_column(Text, nullable=False)
    updated_at: Mapped[datetime] = mapped_column(
        UtcDateTime, default=utcnow, nullable=False
    )
