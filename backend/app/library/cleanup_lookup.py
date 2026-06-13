"""MusicBrainz name lookups for the cleanup analyzer.

Answers one narrow question per name: *is this string a known artist, a
known album (release-group), both, or neither?* The analyzer uses the
verdict as one more clue — upgrading a structural guess it agrees with,
flipping an artist↔album candidate it contradicts. This is deliberately
NOT recording identification (no fingerprinting, no per-file lookups):
names are deduped by the caller and each is queried at most once ever
(verdicts are cached in the `cleanup_name_lookups` table).

MusicBrainz etiquette, per https://musicbrainz.org/doc/MusicBrainz_API:
anonymous clients get 1 request/second and must send an identifiable
User-Agent. `_pace()` enforces the rate process-wide so concurrent calls
can't gang up on the limit; the caller keeps batches small enough that a
synchronous request handler stays comfortably under proxy timeouts.
"""
from __future__ import annotations

import json
import logging
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

logger = logging.getLogger(__name__)

USER_AGENT = "music-dnd-orchestrator/0.1 ( https://github.com/pjunak/music )"
_MB_ROOT = "https://musicbrainz.org/ws/2"
_REQUEST_TIMEOUT_S = 10.0
_MIN_INTERVAL_S = 1.1  # a hair over the 1 req/s rule

_pace_lock = threading.Lock()
_last_request_at = 0.0


def _pace() -> None:
    global _last_request_at
    with _pace_lock:
        wait = _MIN_INTERVAL_S - (time.monotonic() - _last_request_at)
        if wait > 0:
            time.sleep(wait)
        _last_request_at = time.monotonic()


def _lucene_quote(name: str) -> str:
    escaped = name.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


def _top_score(url: str, list_key: str) -> int:
    _pace()
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=_REQUEST_TIMEOUT_S) as response:
        payload = json.loads(response.read().decode("utf-8"))
    entries = payload.get(list_key) or []
    if not entries:
        return 0
    return int(entries[0].get("score") or 0)


def fetch_name_scores(name: str) -> tuple[int, int]:
    """(artist_score, album_score) for `name` — the relevance score (0-100)
    of the best MusicBrainz match when the string is searched as an artist
    and as a release-group. Raises on network/HTTP failure; the caller
    decides whether a failed name is retried on a later run (it is — only
    successful lookups are cached)."""
    quoted = _lucene_quote(name)
    artist_q = urllib.parse.urlencode(
        {"query": f"artist:{quoted}", "fmt": "json", "limit": 1}
    )
    rg_q = urllib.parse.urlencode(
        {"query": f"releasegroup:{quoted}", "fmt": "json", "limit": 1}
    )
    artist_score = _top_score(f"{_MB_ROOT}/artist?{artist_q}", "artists")
    album_score = _top_score(f"{_MB_ROOT}/release-group?{rg_q}", "release-groups")
    return artist_score, album_score


__all__ = ["USER_AGENT", "fetch_name_scores"]
