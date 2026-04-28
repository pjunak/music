import os
from pathlib import Path
from unittest.mock import patch

import pytest
from fastapi.testclient import TestClient


def test_search_requires_auth(client: TestClient) -> None:
    response = client.get("/api/library/search")
    assert response.status_code == 401


def test_search_returns_seeded_track(auth_client: TestClient) -> None:
    response = auth_client.get("/api/library/search")
    assert response.status_code == 200
    body = response.json()
    assert body["limit"] == 100
    assert body["offset"] == 0
    assert body["total"] >= 1
    assert body["sort"] == "artist"
    assert body["order"] == "asc"
    assert len(body["tracks"]) >= 1

    titles = {t["title"] for t in body["tracks"]}
    assert "Test Song" in titles


def test_search_query_filter_excludes_non_matches(auth_client: TestClient) -> None:
    response = auth_client.get("/api/library/search", params={"q": "artist:nonexistent"})
    assert response.status_code == 200
    body = response.json()
    assert body["tracks"] == []
    assert body["total"] == 0


def test_search_pagination(
    auth_client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    """With multiple tracks seeded, limit+offset should slice correctly."""
    response = auth_client.get("/api/library/search", params={"limit": 1, "offset": 0})
    assert response.status_code == 200
    body = response.json()
    assert len(body["tracks"]) == 1
    assert body["total"] >= 4  # 1 primary + 3 extras

    # Page through and confirm total is consistent.
    page2 = auth_client.get(
        "/api/library/search", params={"limit": 2, "offset": 2}
    ).json()
    assert page2["total"] == body["total"]
    assert len(page2["tracks"]) == 2


def test_search_sort_by_title_asc_and_desc(
    auth_client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    asc = auth_client.get(
        "/api/library/search", params={"sort": "title", "order": "asc"}
    ).json()
    desc = auth_client.get(
        "/api/library/search", params={"sort": "title", "order": "desc"}
    ).json()
    asc_titles = [t["title"] for t in asc["tracks"]]
    desc_titles = [t["title"] for t in desc["tracks"]]
    assert asc_titles == sorted(asc_titles, key=str.lower)
    assert desc_titles == list(reversed(asc_titles))


def test_search_rejects_unknown_sort_key(auth_client: TestClient) -> None:
    response = auth_client.get("/api/library/search", params={"sort": "nope"})
    assert response.status_code == 422


def test_get_track_by_id(auth_client: TestClient, seeded_track_id: int) -> None:
    response = auth_client.get(f"/api/library/tracks/{seeded_track_id}")
    assert response.status_code == 200
    assert response.json()["title"] == "Test Song"


def test_get_track_missing_returns_404(auth_client: TestClient) -> None:
    response = auth_client.get("/api/library/tracks/999999")
    assert response.status_code == 404


def test_stream_returns_audio_bytes_inline(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    response = auth_client.get(f"/api/library/tracks/{seeded_track_id}/stream")
    assert response.status_code == 200
    assert len(response.content) == 1100
    # `inline` (or absent attachment) lets <audio> play instead of forcing download.
    assert "attachment" not in response.headers.get("content-disposition", "")


def test_stream_supports_range_requests(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    response = auth_client.get(
        f"/api/library/tracks/{seeded_track_id}/stream",
        headers={"Range": "bytes=0-9"},
    )
    assert response.status_code == 206
    assert len(response.content) == 10
    assert "content-range" in {k.lower() for k in response.headers}


def test_stream_missing_track_returns_404(auth_client: TestClient) -> None:
    response = auth_client.get("/api/library/tracks/999999/stream")
    assert response.status_code == 404


# --- search tolerates a missing Beets DB ---------------------------------


def test_search_returns_empty_when_beets_db_missing(
    auth_client: TestClient, tmp_path: Path
) -> None:
    """Fresh deploys have no Beets DB until the first import. Search should
    return an empty result set rather than 500."""
    from app.core import config
    from app.library import beets_adapter

    original = os.environ["BEETS_LIBRARY_DB"]
    missing = tmp_path / "does-not-exist.db"
    os.environ["BEETS_LIBRARY_DB"] = str(missing)
    config.get_settings.cache_clear()
    beets_adapter.invalidate_cache()
    try:
        response = auth_client.get("/api/library/search")
        assert response.status_code == 200
        assert response.json()["tracks"] == []
    finally:
        os.environ["BEETS_LIBRARY_DB"] = original
        config.get_settings.cache_clear()
        beets_adapter.invalidate_cache()


# --- upload manager: incoming, upload, delete, ingest --------------------


@pytest.fixture
def isolated_incoming(tmp_path: Path):
    """Point INCOMING_DIR at a per-test tmp dir so uploads don't pollute
    each other's listings."""
    from app.core import config

    incoming = tmp_path / "incoming"
    incoming.mkdir()
    original = os.environ.get("INCOMING_DIR")
    os.environ["INCOMING_DIR"] = str(incoming)
    config.get_settings.cache_clear()
    try:
        yield incoming
    finally:
        if original is not None:
            os.environ["INCOMING_DIR"] = original
        else:
            os.environ.pop("INCOMING_DIR", None)
        config.get_settings.cache_clear()


def test_incoming_requires_auth(client: TestClient) -> None:
    assert client.get("/api/library/incoming").status_code == 401


def test_incoming_starts_empty(auth_client: TestClient, isolated_incoming: Path) -> None:
    r = auth_client.get("/api/library/incoming")
    assert r.status_code == 200
    assert r.json() == {"files": []}


def test_upload_writes_files_and_listing_reflects(
    auth_client: TestClient, isolated_incoming: Path
) -> None:
    files = [
        ("files", ("song1.mp3", b"FAKE-MP3-A" * 10, "audio/mpeg")),
        ("files", ("song2.mp3", b"FAKE-MP3-B" * 5, "audio/mpeg")),
    ]
    r = auth_client.post("/api/library/upload", files=files)
    assert r.status_code == 201
    saved = r.json()["saved"]
    assert {f["name"] for f in saved} == {"song1.mp3", "song2.mp3"}

    # Bytes actually hit disk.
    assert (isolated_incoming / "song1.mp3").read_bytes() == b"FAKE-MP3-A" * 10
    assert (isolated_incoming / "song2.mp3").read_bytes() == b"FAKE-MP3-B" * 5

    # And the listing matches.
    listed = auth_client.get("/api/library/incoming").json()["files"]
    assert {f["name"] for f in listed} == {"song1.mp3", "song2.mp3"}


def test_upload_dedupes_filename_collisions(
    auth_client: TestClient, isolated_incoming: Path
) -> None:
    auth_client.post(
        "/api/library/upload",
        files=[("files", ("dup.mp3", b"first", "audio/mpeg"))],
    )
    r = auth_client.post(
        "/api/library/upload",
        files=[("files", ("dup.mp3", b"second", "audio/mpeg"))],
    )
    assert r.status_code == 201
    assert r.json()["saved"][0]["name"] == "dup-1.mp3"
    # Both files exist independently.
    assert (isolated_incoming / "dup.mp3").read_bytes() == b"first"
    assert (isolated_incoming / "dup-1.mp3").read_bytes() == b"second"


def test_upload_strips_directory_components_from_filename(
    auth_client: TestClient, isolated_incoming: Path
) -> None:
    """A crafted filename like '../escape.mp3' must land in incoming/escape.mp3,
    not outside the incoming dir."""
    r = auth_client.post(
        "/api/library/upload",
        files=[("files", ("../../escape.mp3", b"x", "audio/mpeg"))],
    )
    assert r.status_code == 201
    assert (isolated_incoming / "escape.mp3").is_file()
    assert not (isolated_incoming.parent / "escape.mp3").exists()


def test_upload_rejects_empty_request(
    auth_client: TestClient, isolated_incoming: Path
) -> None:
    r = auth_client.post("/api/library/upload")
    # FastAPI rejects missing required form field with 422 before our handler
    # runs; either 400 or 422 is acceptable — both signal "no files".
    assert r.status_code in (400, 422)


def test_delete_incoming_removes_file(
    auth_client: TestClient, isolated_incoming: Path
) -> None:
    auth_client.post(
        "/api/library/upload",
        files=[("files", ("doomed.mp3", b"bytes", "audio/mpeg"))],
    )
    r = auth_client.delete("/api/library/incoming/doomed.mp3")
    assert r.status_code == 204
    assert not (isolated_incoming / "doomed.mp3").exists()


def test_delete_incoming_404_for_missing_file(
    auth_client: TestClient, isolated_incoming: Path
) -> None:
    assert auth_client.delete("/api/library/incoming/never-here.mp3").status_code == 404


def test_delete_incoming_rejects_path_traversal(
    auth_client: TestClient, isolated_incoming: Path
) -> None:
    """A '..' filename must not escape the incoming dir, even via DELETE."""
    sentinel = isolated_incoming.parent / "sentinel.txt"
    sentinel.write_text("must-not-be-deleted")
    # Request "../sentinel.txt" — URL-encoded so it reaches the handler.
    r = auth_client.delete("/api/library/incoming/%2E%2E%2Fsentinel.txt")
    # Either 400 (rejected) or 404 (basename "sentinel.txt" doesn't exist in
    # incoming/) is acceptable; what matters is that the sentinel survives.
    assert r.status_code in (400, 404)
    assert sentinel.exists()


def test_ingest_invokes_run_autoimport_and_returns_result(
    auth_client: TestClient, isolated_incoming: Path
) -> None:
    from app.library import ingest

    captured: dict = {}

    async def _fake_autoimport(path: Path, *, autotag: bool = False):
        captured["autotag"] = autotag
        return ingest.IngestResult(
            returncode=0,
            stdout="/in/song.mp3 -> /lib/Artist/Album/song.mp3\n",
            stderr="",
            imported=1,
            skipped=0,
        )

    with patch.object(ingest, "run_autoimport", side_effect=_fake_autoimport):
        r = auth_client.post("/api/library/ingest")

    assert r.status_code == 200
    body = r.json()
    assert body["ok"] is True
    assert body["returncode"] == 0
    assert body["imported"] == 1
    assert body["skipped"] == 0
    # Default flow is no-autotag — Beets imports as-is using existing tags.
    assert captured["autotag"] is False


def test_ingest_passes_autotag_flag_when_requested(
    auth_client: TestClient, isolated_incoming: Path
) -> None:
    from app.library import ingest

    captured: dict = {}

    async def _fake_autoimport(path: Path, *, autotag: bool = False):
        captured["autotag"] = autotag
        return ingest.IngestResult(
            returncode=0, stdout="", stderr="", imported=0, skipped=0
        )

    with patch.object(ingest, "run_autoimport", side_effect=_fake_autoimport):
        r = auth_client.post("/api/library/ingest", json={"autotag": True})

    assert r.status_code == 200
    assert captured["autotag"] is True


def test_ingest_surfaces_failure_returncode_and_stderr(
    auth_client: TestClient, isolated_incoming: Path
) -> None:
    from app.library import ingest

    async def _fake_autoimport(path: Path, *, autotag: bool = False):
        return ingest.IngestResult(
            returncode=2,
            stdout="",
            stderr="beet: bad config\n",
            imported=0,
            skipped=0,
        )

    with patch.object(ingest, "run_autoimport", side_effect=_fake_autoimport):
        r = auth_client.post("/api/library/ingest")

    assert r.status_code == 200
    body = r.json()
    assert body["ok"] is False
    assert body["returncode"] == 2
    assert "bad config" in body["stderr"]


def test_ingest_503_when_beet_binary_missing(
    auth_client: TestClient, isolated_incoming: Path
) -> None:
    """If `beet` isn't on PATH, asyncio.create_subprocess_exec raises
    FileNotFoundError — surface as 503 so the operator knows to install it."""
    from app.library import ingest

    async def _fake_autoimport(path: Path, *, autotag: bool = False):
        raise FileNotFoundError("[Errno 2] No such file or directory: 'beet'")

    with patch.object(ingest, "run_autoimport", side_effect=_fake_autoimport):
        r = auth_client.post("/api/library/ingest")

    assert r.status_code == 503
    assert "beet" in r.json()["detail"]


def test_ingest_summary_parser_counts_imports_and_skips() -> None:
    """The summary parser is what powers the 'X imported, Y skipped' display.
    Spot-check it against representative beet `-A -q` output."""
    from app.library.ingest import _summarize

    sample = (
        "/in/track1.mp3 -> /lib/A/01.mp3\n"
        "/in/track2.flac -> /lib/B/02.flac\n"
        "Skipping.\n"
        "/in/track3.mp3 -> /lib/C/03.mp3\n"
        "Skipping.\n"
    )
    imported, skipped = _summarize(sample)
    assert imported == 3
    assert skipped == 2
