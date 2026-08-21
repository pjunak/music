"""Process-local throttling for endpoints that verify account passwords."""

from __future__ import annotations

import time


class PasswordAttemptThrottle:
    """Bound online guessing and expensive password-hash work per process."""

    def __init__(
        self,
        *,
        window_seconds: float,
        max_failures: int,
        global_max_failures: int,
        max_keys: int,
    ) -> None:
        self._window_seconds = window_seconds
        self._max_failures = max_failures
        self._global_max_failures = global_max_failures
        self._max_keys = max_keys
        self._failures: dict[str, list[float]] = {}

    def _recent(self, key: str, now: float) -> tuple[int, int]:
        recent = [
            attempt
            for attempt in self._failures.get(key, [])
            if now - attempt < self._window_seconds
        ]
        if recent:
            self._failures[key] = recent
        else:
            self._failures.pop(key, None)
        total = sum(
            sum(
                1
                for attempt in attempts
                if now - attempt < self._window_seconds
            )
            for attempts in self._failures.values()
        )
        return len(recent), total

    def blocked(self, key: str, *, now: float | None = None) -> bool:
        checked_at = time.monotonic() if now is None else now
        key_failures, total_failures = self._recent(key, checked_at)
        return (
            key_failures >= self._max_failures
            or total_failures >= self._global_max_failures
        )

    def record_failure(self, key: str, *, now: float | None = None) -> None:
        attempted_at = time.monotonic() if now is None else now
        self._failures.setdefault(key, []).append(attempted_at)
        if len(self._failures) <= self._max_keys:
            return
        for candidate in [
            candidate
            for candidate, attempts in self._failures.items()
            if not attempts
            or attempted_at - attempts[-1] >= self._window_seconds
        ]:
            self._failures.pop(candidate, None)

    def record_success(self, key: str) -> None:
        self._failures.pop(key, None)

    def reset(self) -> None:
        self._failures.clear()


password_attempt_throttle = PasswordAttemptThrottle(
    window_seconds=60.0,
    max_failures=10,
    global_max_failures=50,
    max_keys=1024,
)
