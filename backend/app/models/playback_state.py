from datetime import datetime

from sqlalchemy import JSON
from sqlalchemy.orm import Mapped, mapped_column

from app.models.base import Base, UtcDateTime, utcnow


class PlaybackState(Base):
    """Singleton row (id=1) holding canonical player state.

    Persisted so playback survives restart. Held hot in memory by the sync
    service at runtime; DB is only touched on state change.
    """

    __tablename__ = "playback_state"

    id: Mapped[int] = mapped_column(primary_key=True, default=1)
    state_json: Mapped[dict] = mapped_column(JSON, nullable=False, default=dict)
    updated_at: Mapped[datetime] = mapped_column(
        UtcDateTime, default=utcnow, onupdate=utcnow, nullable=False
    )
