from datetime import datetime

from sqlalchemy import Boolean, String, Text
from sqlalchemy.orm import Mapped, mapped_column

from app.models.base import Base, UtcDateTime, utcnow


class AssistantProviderConnection(Base):
    """Encrypted upstream model-provider connection owned by the installation."""

    __tablename__ = "assistant_provider_connections"

    id: Mapped[str] = mapped_column(String(32), primary_key=True)
    name: Mapped[str] = mapped_column(String(128), nullable=False, unique=True)
    adapter_id: Mapped[str] = mapped_column(String(64), nullable=False)
    base_url: Mapped[str] = mapped_column(String(2048), nullable=False)
    encrypted_api_key: Mapped[str] = mapped_column(Text, nullable=False)
    api_key_nonce: Mapped[str] = mapped_column(String(64), nullable=False)
    api_key_hint: Mapped[str] = mapped_column(String(32), nullable=False)
    allow_private_network: Mapped[bool] = mapped_column(
        Boolean, nullable=False, default=False
    )
    verification_status: Mapped[str] = mapped_column(
        String(16), nullable=False, default="never"
    )
    verification_error_code: Mapped[str | None] = mapped_column(
        String(64), nullable=True
    )
    verified_models_json: Mapped[str] = mapped_column(Text, nullable=False, default="[]")
    verified_capabilities_json: Mapped[str] = mapped_column(
        Text, nullable=False, default="[]"
    )
    last_verified_at: Mapped[datetime | None] = mapped_column(UtcDateTime, nullable=True)
    created_at: Mapped[datetime] = mapped_column(
        UtcDateTime, default=utcnow, nullable=False
    )
    updated_at: Mapped[datetime] = mapped_column(
        UtcDateTime, default=utcnow, onupdate=utcnow, nullable=False
    )
