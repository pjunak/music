from datetime import UTC, datetime
from typing import Annotated

from fastapi import APIRouter, Cookie, HTTPException, Response, status
from pydantic import BaseModel, Field
from sqlalchemy import select

from app.api.deps import CurrentUser, DbSession, settings
from app.core.security import (
    generate_session_token,
    session_expiry,
    verify_password,
)
from app.models.auth_session import AuthSession
from app.models.user import User

router = APIRouter(prefix="/api/auth", tags=["auth"])


class LoginRequest(BaseModel):
    username: str = Field(min_length=1, max_length=64)
    password: str = Field(min_length=1, max_length=256)


class UserInfo(BaseModel):
    id: int
    username: str


def _set_session_cookie(response: Response, token: str) -> None:
    response.set_cookie(
        key=settings.session_cookie_name,
        value=token,
        max_age=settings.session_ttl_days * 24 * 3600,
        httponly=True,
        samesite="lax",
        secure=settings.session_cookie_secure,
        domain=settings.session_cookie_domain or None,
        path="/",
    )


@router.post("/login", response_model=UserInfo)
def login(payload: LoginRequest, response: Response, db: DbSession) -> UserInfo:
    user = db.scalar(select(User).where(User.username == payload.username))
    if user is None or not verify_password(user.password_hash, payload.password):
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED, detail="invalid credentials"
        )

    token = generate_session_token()
    now = datetime.now(UTC)
    session = AuthSession(
        token=token,
        user_id=user.id,
        created_at=now,
        expires_at=session_expiry(settings.session_ttl_days),
        last_seen=now,
    )
    db.add(session)
    db.commit()

    _set_session_cookie(response, token)
    return UserInfo(id=user.id, username=user.username)


@router.post("/logout", status_code=status.HTTP_204_NO_CONTENT)
def logout(response: Response, user: CurrentUser, db: DbSession) -> None:
    # Single-user setup: nuke every session for this user rather than
    # threading the exact token from the cookie down to the handler.
    db.query(AuthSession).filter(AuthSession.user_id == user.id).delete()
    db.commit()
    response.delete_cookie(settings.session_cookie_name, path="/")


@router.get("/me", response_model=UserInfo)
def me(user: CurrentUser) -> UserInfo:
    return UserInfo(id=user.id, username=user.username)


# Session prefix length used to uniquely identify a session row in the API
# without exposing the full token. With 96-char hex tokens, 12 chars is more
# than enough to avoid collisions for a single user's session set.
_SESSION_PREFIX_LEN = 12


class ActiveSession(BaseModel):
    token_prefix: str
    created_at: datetime
    expires_at: datetime
    last_seen: datetime
    is_current: bool


@router.get("/sessions", response_model=list[ActiveSession])
def list_sessions(
    user: CurrentUser,
    db: DbSession,
    session_cookie: Annotated[str | None, Cookie(alias="music_session")] = None,
) -> list[ActiveSession]:
    """Active sessions for the current user, newest first. Used by the
    Settings → Active sessions panel so the operator can see what's logged
    in (e.g. forgotten browser tabs on a TV) and revoke individual ones."""
    rows = (
        db.query(AuthSession)
        .filter(AuthSession.user_id == user.id)
        .order_by(AuthSession.last_seen.desc())
        .all()
    )
    out: list[ActiveSession] = []
    for s in rows:
        out.append(
            ActiveSession(
                token_prefix=s.token[:_SESSION_PREFIX_LEN],
                created_at=s.created_at,
                expires_at=s.expires_at,
                last_seen=s.last_seen,
                is_current=session_cookie is not None and s.token == session_cookie,
            )
        )
    return out


@router.delete(
    "/sessions/{token_prefix}", status_code=status.HTTP_204_NO_CONTENT
)
def revoke_session(
    token_prefix: str,
    user: CurrentUser,
    db: DbSession,
) -> None:
    """Revoke a single session by its token prefix. Refuses to act if the
    prefix matches more than one row to avoid revoking the wrong one."""
    if len(token_prefix) < 8:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="token prefix too short",
        )
    matches = (
        db.query(AuthSession)
        .filter(
            AuthSession.user_id == user.id,
            AuthSession.token.startswith(token_prefix),
        )
        .all()
    )
    if not matches:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="no matching session",
        )
    if len(matches) > 1:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail="prefix matches multiple sessions; pass a longer prefix",
        )
    db.delete(matches[0])
    db.commit()
