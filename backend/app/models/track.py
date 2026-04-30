from datetime import datetime

from sqlalchemy import Float, Integer, String
from sqlalchemy.orm import Mapped, mapped_column

from app.models.base import Base, UtcDateTime, utcnow


class Track(Base):
    """A single audio file under MUSIC_DIR.

    `path` is canonical, relative to MUSIC_DIR with forward slashes —
    invariant under platform changes (Windows dev / Linux server). The
    indexer maintains the row whenever it walks the filesystem; mtime
    + size_bytes are used to detect changes without re-reading tags.
    """

    __tablename__ = "tracks"

    id: Mapped[int] = mapped_column(primary_key=True)
    path: Mapped[str] = mapped_column(String(1024), nullable=False, unique=True, index=True)

    title: Mapped[str] = mapped_column(String(512), nullable=False, default="")
    artist: Mapped[str] = mapped_column(String(512), nullable=False, default="")
    album_artist: Mapped[str] = mapped_column(String(512), nullable=False, default="")
    album: Mapped[str] = mapped_column(String(512), nullable=False, default="")
    track_no: Mapped[int | None] = mapped_column(Integer, nullable=True)
    disc_no: Mapped[int | None] = mapped_column(Integer, nullable=True)
    year: Mapped[int | None] = mapped_column(Integer, nullable=True)
    genre: Mapped[str] = mapped_column(String(128), nullable=False, default="")
    length_s: Mapped[float] = mapped_column(Float, nullable=False, default=0.0)
    bpm: Mapped[int | None] = mapped_column(Integer, nullable=True)

    # User-entered, DB-only fields. Decoupled from the on-disk tags so the
    # operator can label tracks without touching ID3 (and without losing
    # those labels when a tag-rich file is re-tagged or replaced upstream).
    # `display_title` overrides `title` for UI rendering when set.
    # `origin` is free-form provenance — game/film/album name beyond what
    # ID3 cleanly expresses (e.g. "Skyrim", "The Witcher 3").
    display_title: Mapped[str] = mapped_column(String(512), nullable=False, default="")
    origin: Mapped[str] = mapped_column(String(512), nullable=False, default="")

    size_bytes: Mapped[int] = mapped_column(Integer, nullable=False)
    mtime: Mapped[int] = mapped_column(Integer, nullable=False)
    added_at: Mapped[datetime] = mapped_column(UtcDateTime, default=utcnow, nullable=False)
