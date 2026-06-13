from datetime import datetime

from sqlalchemy import Integer, String
from sqlalchemy.orm import Mapped, mapped_column

from app.models.base import Base, UtcDateTime, utcnow


class CleanupNameLookup(Base):
    """Cached MusicBrainz verdict for one name string.

    `loose_key` is the accent/case/punctuation-folded form the cleanup
    engine compares names with — one row answers "is this an artist or an
    album?" for every spelling variant that folds to the same key, forever.
    Names are only inserted after a *successful* lookup, so a failed query
    (network down, MB 503) is retried naturally on a later run.
    """

    __tablename__ = "cleanup_name_lookups"

    id: Mapped[int] = mapped_column(primary_key=True)
    loose_key: Mapped[str] = mapped_column(
        String(512), nullable=False, unique=True, index=True
    )
    name: Mapped[str] = mapped_column(String(512), nullable=False)
    artist_score: Mapped[int] = mapped_column(Integer, nullable=False, default=0)
    album_score: Mapped[int] = mapped_column(Integer, nullable=False, default=0)
    fetched_at: Mapped[datetime] = mapped_column(UtcDateTime, default=utcnow, nullable=False)
