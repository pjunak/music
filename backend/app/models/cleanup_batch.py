from datetime import datetime

from sqlalchemy import String, Text
from sqlalchemy.orm import Mapped, mapped_column

from app.models.base import Base, UtcDateTime, utcnow


class CleanupBatch(Base):
    """Journal of one applied library-cleanup run.

    `items_json` is the list of changes that actually landed (renames with
    path before/after, tag writes with old/new value) — enough to revert
    the batch or to hand the operator a downloadable record. Chunked
    applies append to the same row. `reverted_at` marks a batch whose
    inverse has been applied; it can't be applied or reverted again.
    """

    __tablename__ = "cleanup_batches"

    id: Mapped[int] = mapped_column(primary_key=True)
    created_at: Mapped[datetime] = mapped_column(UtcDateTime, default=utcnow, nullable=False)
    scope_label: Mapped[str] = mapped_column(String(512), nullable=False, default="")
    items_json: Mapped[str] = mapped_column(Text, nullable=False, default="[]")
    reverted_at: Mapped[datetime | None] = mapped_column(UtcDateTime, nullable=True)
