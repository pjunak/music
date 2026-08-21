"""Synthetic Assistant credentials shared by provider-facing tests."""


def _credential(*parts: str) -> str:
    """Build realistic test input without committing secret-shaped literals."""

    return "-".join(("test", *parts))


TEST_PROVIDER_API_KEY = _credential("provider", "credential", "1234")
TEST_REPLACEMENT_API_KEY = _credential("replacement", "credential", "9999")
TEST_CLEANUP_API_KEY = _credential("cleanup", "credential", "1234")
TEST_EXISTING_API_KEY = _credential("existing", "credential", "5678")
TEST_SHORT_API_KEY = _credential("provider", "value")
