from datetime import datetime

from sqlalchemy import CheckConstraint, ForeignKey, String
from sqlalchemy.orm import Mapped, mapped_column

from app.models.base import Base, UtcDateTime, utcnow


class TrackAnalysisTagReview(Base):
    """Operator decision for one tag emitted by one analysis profile.

    The source signature binds the decision to the exact evidence that was
    reviewed. A newer profile can therefore surface the tag for review again
    while leaving manual tags untouched until the operator makes a new choice.
    """

    __tablename__ = "track_analysis_tag_reviews"
    __table_args__ = (
        CheckConstraint(
            "decision IN ('accepted', 'rejected')",
            name="ck_track_analysis_tag_reviews_decision",
        ),
    )

    track_id: Mapped[int] = mapped_column(
        ForeignKey("tracks.id", ondelete="CASCADE"), primary_key=True
    )
    analyzer_id: Mapped[str] = mapped_column(String(128), primary_key=True)
    tag: Mapped[str] = mapped_column(String(64), primary_key=True)
    source_signature: Mapped[str] = mapped_column(String(128), nullable=False)
    decision: Mapped[str] = mapped_column(String(16), nullable=False)
    reviewed_at: Mapped[datetime] = mapped_column(
        UtcDateTime, default=utcnow, nullable=False
    )
