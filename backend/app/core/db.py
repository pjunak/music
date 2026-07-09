from collections.abc import Generator
from typing import Any

from sqlalchemy import create_engine, event
from sqlalchemy.orm import Session, sessionmaker

from app.core.config import get_settings

_settings = get_settings()

_connect_args = {"check_same_thread": False} if _settings.database_url.startswith("sqlite") else {}

engine = create_engine(
    _settings.database_url,
    connect_args=_connect_args,
    future=True,
)


@event.listens_for(engine, "connect")
def _sqlite_pragmas(dbapi_connection: Any, _record: Any) -> None:
    """Per-connection SQLite PRAGMAs.

    - ``foreign_keys=ON``: SQLite ignores ``ondelete="CASCADE"`` otherwise, so
      ``FOREIGN KEY`` declarations would be advisory only and orphans would
      accumulate silently.
    - ``journal_mode=WAL``: readers don't block the writer (and vice versa), so
      a position-report write no longer contends with a concurrent search read.
      DB-level and persistent once set; re-asserting per connection is cheap.
    - ``busy_timeout=5000``: with ``check_same_thread=False`` and threadpool
      workers each opening connections, a momentary write lock would otherwise
      surface as an immediate ``database is locked`` error; wait up to 5 s."""
    if engine.dialect.name != "sqlite":
        return
    cursor = dbapi_connection.cursor()
    cursor.execute("PRAGMA foreign_keys=ON")
    cursor.execute("PRAGMA journal_mode=WAL")
    cursor.execute("PRAGMA busy_timeout=5000")
    cursor.close()


SessionLocal = sessionmaker(bind=engine, autoflush=False, autocommit=False, future=True)


def get_db() -> Generator[Session, None, None]:
    db = SessionLocal()
    try:
        yield db
    finally:
        db.close()
