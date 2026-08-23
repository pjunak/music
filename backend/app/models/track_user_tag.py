from datetime import datetime

from sqlalchemy import ForeignKey, String
from sqlalchemy.orm import Mapped, mapped_column

from app.models.base import Base, UtcDateTime, utcnow


class TrackUserTag(Base):
    """Operator-owned terrain, scene, or mood tag attached to one track.

    These rows are deliberately independent from file metadata and generated
    ``TrackAnalysis`` output. They never write ID3/Vorbis fields such as album,
    year, or genre, and re-indexing cannot overwrite the classification.
    """

    __tablename__ = "track_user_tags"

    track_id: Mapped[int] = mapped_column(
        ForeignKey("tracks.id", ondelete="CASCADE"), primary_key=True
    )
    tag: Mapped[str] = mapped_column(String(64), primary_key=True, index=True)
    created_at: Mapped[datetime] = mapped_column(
        UtcDateTime, default=utcnow, nullable=False
    )
