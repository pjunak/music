from datetime import UTC, datetime
from typing import Annotated

from fastapi import Cookie, Depends, HTTPException, status
from sqlalchemy.orm import Session

from app.core.config import get_settings
from app.core.db import get_db
from app.models.auth_session import AuthSession
from app.models.user import User


def _resolve_user(
    db: Session, session_cookie: str | None
) -> User | None:
    """Look up the user behind the session cookie. Returns None for absent
    or invalid cookies (caller decides whether that's allowed)."""
    if not session_cookie:
        return None
    session = db.get(AuthSession, session_cookie)
    if session is None:
        return None
    now = datetime.now(UTC)
    if session.expires_at <= now:
        # Opportunistic cleanup: an expired row is worthless and the table
        # would otherwise grow unbounded for a long-lived install.
        db.delete(session)
        db.commit()
        return None
    user = db.get(User, session.user_id)
    if user is None:
        return None
    session.last_seen = now
    db.commit()
    return user


def get_current_user(
    db: Annotated[Session, Depends(get_db)],
    session_cookie: Annotated[str | None, Cookie(alias="music_session")] = None,
) -> User:
    user = _resolve_user(db, session_cookie)
    if user is None:
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED, detail="not authenticated"
        )
    return user


def get_optional_user(
    db: Annotated[Session, Depends(get_db)],
    session_cookie: Annotated[str | None, Cookie(alias="music_session")] = None,
) -> User | None:
    """Soft auth — returns the user if logged in, None for guests. Used by
    Player-relevant endpoints (track stream, cover art, single-track
    metadata, SFX file) so a logged-out viewer (e.g. a TV bookmark) can
    still play whatever the DM has queued up. Mutating endpoints continue
    to use `CurrentUser` and reject guests with 401."""
    return _resolve_user(db, session_cookie)


CurrentUser = Annotated[User, Depends(get_current_user)]
OptionalUser = Annotated[User | None, Depends(get_optional_user)]
DbSession = Annotated[Session, Depends(get_db)]

# Re-export so routers don't have to import get_settings directly for cookie config.
settings = get_settings()
