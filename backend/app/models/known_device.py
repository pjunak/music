from datetime import datetime

from sqlalchemy import Boolean, String
from sqlalchemy.orm import Mapped, mapped_column

from app.models.base import Base, UtcDateTime, utcnow


class KnownDevice(Base):
    """Persistent identity for a browser/appliance that has connected.

    Keyed on `client_id` — a stable per-install token the client mints once and
    keeps in localStorage (unlike the ephemeral per-connection device id, which
    is reborn on every reconnect). This row survives restarts so the operator's
    audio-output designation sticks to a physical device across refreshes.

    `is_output` is the *manual* designation "this device may act as an audio
    output". It defaults False and is **never** set implicitly — a device only
    becomes output-eligible when the operator toggles it in the device list.
    That default is the invariant that stops a refreshed tab from spontaneously
    becoming a speaker.
    """

    __tablename__ = "known_devices"

    client_id: Mapped[str] = mapped_column(String(64), primary_key=True)
    name: Mapped[str] = mapped_column(String(128), default="", nullable=False)
    is_output: Mapped[bool] = mapped_column(Boolean, default=False, nullable=False)
    created_at: Mapped[datetime] = mapped_column(
        UtcDateTime, default=utcnow, nullable=False
    )
    last_seen: Mapped[datetime] = mapped_column(
        UtcDateTime, default=utcnow, onupdate=utcnow, nullable=False
    )
