import contextlib
import secrets
from datetime import UTC, datetime, timedelta

from argon2 import PasswordHasher
from argon2.exceptions import VerifyMismatchError

_hasher = PasswordHasher()

# Precomputed hash of a throwaway secret. Verifying against it costs the same
# as a real check, so login can spend that time even when the username doesn't
# exist — otherwise the fast "no such user" path is a timing oracle for whether
# an account exists. The password verified against it is irrelevant (argon2's
# cost depends on the hash parameters, not the input).
_DUMMY_HASH = _hasher.hash(secrets.token_urlsafe(16))


def hash_password(password: str) -> str:
    return _hasher.hash(password)


def verify_password(hashed: str, password: str) -> bool:
    try:
        return _hasher.verify(hashed, password)
    except VerifyMismatchError:
        return False


def dummy_verify() -> None:
    """Burn one argon2 verification to equalize login timing when the username
    is unknown. Suppresses the expected mismatch."""
    with contextlib.suppress(VerifyMismatchError):
        _hasher.verify(_DUMMY_HASH, "x")


def generate_session_token() -> str:
    return secrets.token_urlsafe(48)


def session_expiry(ttl_days: int) -> datetime:
    return datetime.now(UTC) + timedelta(days=ttl_days)
