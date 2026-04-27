from datetime import UTC, datetime
from typing import Any

from sqlalchemy import DateTime as SADateTime
from sqlalchemy.orm import DeclarativeBase
from sqlalchemy.types import TypeDecorator


class Base(DeclarativeBase):
    pass


def utcnow() -> datetime:
    return datetime.now(UTC)


class UtcDateTime(TypeDecorator):
    """DateTime column that round-trips as timezone-aware UTC.

    SQLite drops tzinfo on read, which trips comparisons against
    ``datetime.now(UTC)``. This decorator normalizes both directions so the
    rest of the app can assume UTC-aware datetimes regardless of dialect.
    """

    impl = SADateTime(timezone=True)
    cache_ok = True

    def process_bind_param(self, value: datetime | None, dialect: Any) -> datetime | None:
        if value is None:
            return None
        if value.tzinfo is None:
            value = value.replace(tzinfo=UTC)
        return value.astimezone(UTC)

    def process_result_value(self, value: datetime | None, dialect: Any) -> datetime | None:
        if value is None:
            return None
        if value.tzinfo is None:
            return value.replace(tzinfo=UTC)
        return value.astimezone(UTC)
