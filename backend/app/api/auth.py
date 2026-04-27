from datetime import UTC, datetime

from fastapi import APIRouter, HTTPException, Response, status
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
