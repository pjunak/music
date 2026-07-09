"""Auth API: login, logout, /me, expiry, cookie handling."""
from __future__ import annotations

from datetime import UTC, datetime, timedelta

import pytest
from fastapi.testclient import TestClient

from .conftest import TEST_PASSWORD, TEST_USERNAME


@pytest.fixture(autouse=True)
def _reset_login_throttle():
    """The login throttle is process-global; clear it between tests so failed-
    login tests don't leak their counters into each other."""
    from app.api.auth import _reset_login_throttle_for_tests

    _reset_login_throttle_for_tests()
    yield
    _reset_login_throttle_for_tests()


def _ensure_user() -> None:
    from sqlalchemy import select

    from app.core.db import SessionLocal
    from app.core.security import hash_password
    from app.models.user import User

    with SessionLocal() as db:
        if db.scalar(select(User).where(User.username == TEST_USERNAME)) is None:
            db.add(User(username=TEST_USERNAME, password_hash=hash_password(TEST_PASSWORD)))
            db.commit()


# --- /api/auth/login --------------------------------------------------------


def test_login_succeeds_with_valid_credentials(client: TestClient) -> None:
    _ensure_user()
    r = client.post(
        "/api/auth/login",
        json={"username": TEST_USERNAME, "password": TEST_PASSWORD},
    )
    assert r.status_code == 200
    body = r.json()
    assert body["username"] == TEST_USERNAME
    # Cookie set; secure flag follows the (test default = false) config.
    cookie = r.cookies.get("music_session")
    assert cookie is not None and len(cookie) > 16


def test_login_rejects_wrong_password(client: TestClient) -> None:
    _ensure_user()
    r = client.post(
        "/api/auth/login",
        json={"username": TEST_USERNAME, "password": "not-the-password"},
    )
    assert r.status_code == 401
    assert "music_session" not in r.cookies


def test_login_unknown_user_returns_401_not_500(client: TestClient) -> None:
    """The timing-equalizer path (dummy_verify for a missing user) must reject
    cleanly, not raise."""
    r = client.post(
        "/api/auth/login",
        json={"username": "no-such-account", "password": "whatever"},
    )
    assert r.status_code == 401


def test_login_throttles_repeated_failures(client: TestClient) -> None:
    """After 10 failures the throttle rejects with 429 *before* argon2 runs —
    capping both online guessing and the hash-flood DoS. It rejects even a
    correct password while tripped."""
    _ensure_user()
    for _ in range(10):
        r = client.post(
            "/api/auth/login",
            json={"username": TEST_USERNAME, "password": "wrong"},
        )
        assert r.status_code == 401
    blocked = client.post(
        "/api/auth/login",
        json={"username": TEST_USERNAME, "password": "wrong"},
    )
    assert blocked.status_code == 429
    still_blocked = client.post(
        "/api/auth/login",
        json={"username": TEST_USERNAME, "password": TEST_PASSWORD},
    )
    assert still_blocked.status_code == 429


def test_successful_login_clears_throttle_counter(client: TestClient) -> None:
    """A success resets the failure count, so a few typos before the right
    password don't leave the operator throttled."""
    _ensure_user()
    for _ in range(5):
        client.post(
            "/api/auth/login",
            json={"username": TEST_USERNAME, "password": "wrong"},
        )
    ok = client.post(
        "/api/auth/login",
        json={"username": TEST_USERNAME, "password": TEST_PASSWORD},
    )
    assert ok.status_code == 200
    # Counter cleared: another full run of failures is needed to trip again.
    r = client.post(
        "/api/auth/login",
        json={"username": TEST_USERNAME, "password": "wrong"},
    )
    assert r.status_code == 401


def test_login_rejects_unknown_user(client: TestClient) -> None:
    r = client.post(
        "/api/auth/login",
        json={"username": "ghost-user", "password": "irrelevant"},
    )
    assert r.status_code == 401


def test_login_validates_payload(client: TestClient) -> None:
    r = client.post("/api/auth/login", json={"username": "", "password": ""})
    assert r.status_code == 422


# --- /api/auth/me -----------------------------------------------------------


def test_me_returns_current_user_when_authed(auth_client: TestClient) -> None:
    r = auth_client.get("/api/auth/me")
    assert r.status_code == 200
    body = r.json()
    assert body["username"] == TEST_USERNAME


def test_me_rejects_unauthed_request(client: TestClient) -> None:
    r = client.get("/api/auth/me")
    assert r.status_code == 401


def test_me_rejects_garbage_cookie(client: TestClient) -> None:
    client.cookies.set("music_session", "this-is-not-a-real-token")
    r = client.get("/api/auth/me")
    assert r.status_code == 401


# --- /api/auth/logout -------------------------------------------------------


def test_logout_clears_session_and_cookie(auth_client: TestClient) -> None:
    r = auth_client.post("/api/auth/logout")
    assert r.status_code == 204
    # Subsequent /me call now fails — server invalidated the row.
    follow = auth_client.get("/api/auth/me")
    assert follow.status_code == 401


def test_logout_requires_auth(client: TestClient) -> None:
    r = client.post("/api/auth/logout")
    assert r.status_code == 401


# --- session expiry / cleanup ----------------------------------------------


def test_expired_session_is_rejected_and_pruned(client: TestClient) -> None:
    """A session past `expires_at` must be rejected on access AND its row
    deleted opportunistically so the table doesn't grow unbounded."""
    from sqlalchemy import select

    from app.core.db import SessionLocal
    from app.core.security import generate_session_token
    from app.models.auth_session import AuthSession

    _ensure_user()

    # Look up the user id and insert an already-expired session for them.
    from app.models.user import User

    token = generate_session_token()
    with SessionLocal() as db:
        user = db.scalar(select(User).where(User.username == TEST_USERNAME))
        assert user is not None
        past = datetime.now(UTC) - timedelta(minutes=1)
        db.add(
            AuthSession(
                token=token,
                user_id=user.id,
                created_at=past - timedelta(days=1),
                expires_at=past,
                last_seen=past,
            )
        )
        db.commit()

    client.cookies.set("music_session", token)
    r = client.get("/api/auth/me")
    assert r.status_code == 401

    # Row should now be gone.
    with SessionLocal() as db:
        assert db.get(AuthSession, token) is None


def test_login_creates_distinct_sessions_for_repeated_logins(client: TestClient) -> None:
    """Each successful login must mint a fresh token. Otherwise resetting
    a stuck client would resurrect the same expired token."""
    _ensure_user()

    first = client.post(
        "/api/auth/login",
        json={"username": TEST_USERNAME, "password": TEST_PASSWORD},
    )
    second = client.post(
        "/api/auth/login",
        json={"username": TEST_USERNAME, "password": TEST_PASSWORD},
    )
    assert first.status_code == 200
    assert second.status_code == 200
    assert first.cookies.get("music_session") != second.cookies.get("music_session")


def test_logout_invalidates_all_sessions_for_user(client: TestClient) -> None:
    """Single-user setup: logout drops every session row for that user so
    a stale token in another tab can't survive the operator hitting Logout."""
    _ensure_user()

    # Open two parallel sessions by logging in twice.
    a = TestClient(client.app)
    a.post(
        "/api/auth/login",
        json={"username": TEST_USERNAME, "password": TEST_PASSWORD},
    )
    b = TestClient(client.app)
    b.post(
        "/api/auth/login",
        json={"username": TEST_USERNAME, "password": TEST_PASSWORD},
    )
    assert a.get("/api/auth/me").status_code == 200
    assert b.get("/api/auth/me").status_code == 200

    a.post("/api/auth/logout")
    # Both sessions invalidated.
    assert a.get("/api/auth/me").status_code == 401
    assert b.get("/api/auth/me").status_code == 401


# --- /api/auth/sessions -----------------------------------------------------


def test_list_sessions_marks_current(client: TestClient) -> None:
    _ensure_user()
    a = TestClient(client.app)
    a.post(
        "/api/auth/login",
        json={"username": TEST_USERNAME, "password": TEST_PASSWORD},
    )
    b = TestClient(client.app)
    b.post(
        "/api/auth/login",
        json={"username": TEST_USERNAME, "password": TEST_PASSWORD},
    )
    r = a.get("/api/auth/sessions")
    assert r.status_code == 200
    rows = r.json()
    assert len(rows) == 2
    current = [row for row in rows if row["is_current"]]
    assert len(current) == 1
    # Token prefix exposed but not the full secret.
    a_cookie = a.cookies.get("music_session")
    assert a_cookie is not None
    assert current[0]["token_prefix"] == a_cookie[:12]
    assert len(current[0]["token_prefix"]) == 12


def test_revoke_session_drops_other_session(client: TestClient) -> None:
    _ensure_user()
    a = TestClient(client.app)
    a.post(
        "/api/auth/login",
        json={"username": TEST_USERNAME, "password": TEST_PASSWORD},
    )
    b = TestClient(client.app)
    b.post(
        "/api/auth/login",
        json={"username": TEST_USERNAME, "password": TEST_PASSWORD},
    )
    rows = a.get("/api/auth/sessions").json()
    other = next(row for row in rows if not row["is_current"])
    r = a.delete(f"/api/auth/sessions/{other['token_prefix']}")
    assert r.status_code == 204
    # Revoked session can no longer authenticate; the surviving one still does.
    assert b.get("/api/auth/me").status_code == 401
    assert a.get("/api/auth/me").status_code == 200


def test_revoke_session_rejects_short_prefix(client: TestClient) -> None:
    _ensure_user()
    a = TestClient(client.app)
    a.post(
        "/api/auth/login",
        json={"username": TEST_USERNAME, "password": TEST_PASSWORD},
    )
    r = a.delete("/api/auth/sessions/abc")
    assert r.status_code == 400


def test_revoke_session_rejects_unknown_prefix(client: TestClient) -> None:
    _ensure_user()
    a = TestClient(client.app)
    a.post(
        "/api/auth/login",
        json={"username": TEST_USERNAME, "password": TEST_PASSWORD},
    )
    r = a.delete("/api/auth/sessions/00000000abcd")
    assert r.status_code == 404
