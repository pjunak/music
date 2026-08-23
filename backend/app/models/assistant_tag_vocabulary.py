from datetime import datetime

from sqlalchemy import Integer, String, Text
from sqlalchemy.orm import Mapped, mapped_column

from app.models.base import Base, UtcDateTime, utcnow


class AssistantTagVocabulary(Base):
    """The operator-owned database vocabulary used for library mood tagging."""

    __tablename__ = "assistant_tag_vocabularies"

    key: Mapped[str] = mapped_column(String(32), primary_key=True)
    revision: Mapped[int] = mapped_column(Integer, nullable=False, default=1)
    seed_version: Mapped[int] = mapped_column(Integer, nullable=False, default=4)
    document_json: Mapped[str] = mapped_column(Text, nullable=False)
    updated_at: Mapped[datetime] = mapped_column(
        UtcDateTime,
        default=utcnow,
        onupdate=utcnow,
        nullable=False,
    )
