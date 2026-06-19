"""Library API: tree, search, metadata edit, move, delete, upload, rescan."""
from __future__ import annotations

import io
import os
import struct
from pathlib import Path

from fastapi.testclient import TestClient


def _silent_wav(seconds: float = 0.5) -> bytes:
    sr = 8000
    pcm = b"\x00\x00" * int(seconds * sr)
    h = b"RIFF" + struct.pack("<I", 36 + len(pcm)) + b"WAVE"
    h += b"fmt " + struct.pack("<I", 16)
    h += struct.pack("<H", 1) + struct.pack("<H", 1)
    h += struct.pack("<I", sr) + struct.pack("<I", sr * 2)
    h += struct.pack("<H", 2) + struct.pack("<H", 16)
    h += b"data" + struct.pack("<I", len(pcm))
    return h + pcm


# --- auth + smoke --------------------------------------------------------


def test_search_requires_auth(client: TestClient) -> None:
    assert client.get("/api/library/search").status_code == 401


def test_tree_requires_auth(client: TestClient) -> None:
    assert client.get("/api/library/tree").status_code == 401


def test_search_returns_seeded_track(auth_client: TestClient, seeded_track_id: int) -> None:
    response = auth_client.get("/api/library/search")
    assert response.status_code == 200
    body = response.json()
    assert body["total"] >= 1
    paths = {t["path"] for t in body["tracks"]}
    assert "Demo/test-song.wav" in paths


# --- search behaviour ----------------------------------------------------


def test_search_substring_query(
    auth_client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    response = auth_client.get("/api/library/search", params={"q": "extra"})
    assert response.status_code == 200
    body = response.json()
    assert body["total"] >= 3
    for t in body["tracks"]:
        assert "extra" in (t["path"] + t["title"]).lower()


def test_search_pagination_and_sort(
    auth_client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    """Sort by path asc, page through; total stays consistent."""
    page1 = auth_client.get(
        "/api/library/search", params={"sort": "path", "limit": 2, "offset": 0}
    ).json()
    page2 = auth_client.get(
        "/api/library/search", params={"sort": "path", "limit": 2, "offset": 2}
    ).json()
    assert page1["total"] == page2["total"]
    assert len(page1["tracks"]) == 2
    paths_seen = [t["path"] for t in (page1["tracks"] + page2["tracks"])]
    assert paths_seen == sorted(paths_seen)


def test_search_rejects_unknown_sort(auth_client: TestClient) -> None:
    assert auth_client.get("/api/library/search", params={"sort": "nope"}).status_code == 422


# --- tree -----------------------------------------------------------------


def test_tree_root_lists_seeded_folders(auth_client: TestClient, seeded_track_id: int) -> None:
    body = auth_client.get("/api/library/tree").json()
    folders = {f["name"] for f in body["folders"]}
    assert "Demo" in folders
    # The Demo folder contains the seeded track recursively.
    demo = next(f for f in body["folders"] if f["name"] == "Demo")
    assert demo["path"] == "Demo"
    assert demo["track_count"] >= 1


def test_tree_subfolder_lists_tracks(auth_client: TestClient, seeded_track_id: int) -> None:
    body = auth_client.get("/api/library/tree", params={"path": "Demo"}).json()
    assert body["path"] == "Demo"
    paths = {t["path"] for t in body["tracks"]}
    assert "Demo/test-song.wav" in paths


def test_tree_unknown_path_is_empty(auth_client: TestClient) -> None:
    body = auth_client.get("/api/library/tree", params={"path": "NowhereLand"}).json()
    assert body["folders"] == []
    assert body["tracks"] == []


# --- all folders (client-side tree) ---------------------------------------


def test_folders_requires_auth(client: TestClient) -> None:
    assert client.get("/api/library/folders").status_code == 401


def test_folders_lists_whole_hierarchy_with_counts(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    # Empty nested folders must appear too — they're upload destinations.
    auth_client.post("/api/library/folders", json={"path": "AllF/Skyrim/Combat"})
    body = auth_client.get("/api/library/folders").json()
    by_path = {f["path"]: f for f in body["folders"]}
    assert by_path["AllF"]["has_children"] is True
    assert by_path["AllF/Skyrim"]["has_children"] is True
    assert by_path["AllF/Skyrim/Combat"]["has_children"] is False
    assert by_path["AllF/Skyrim/Combat"]["track_count"] == 0
    # The seeded track counts recursively on its own folder.
    assert by_path["Demo"]["track_count"] >= 1


# --- single track + stream + cover ---------------------------------------


def test_get_track_by_id(auth_client: TestClient, seeded_track_id: int) -> None:
    r = auth_client.get(f"/api/library/tracks/{seeded_track_id}")
    assert r.status_code == 200
    body = r.json()
    assert body["id"] == seeded_track_id
    assert body["path"] == "Demo/test-song.wav"


def test_get_track_missing_returns_404(auth_client: TestClient) -> None:
    assert auth_client.get("/api/library/tracks/999999").status_code == 404


def test_stream_returns_audio_bytes(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    r = auth_client.get(f"/api/library/tracks/{seeded_track_id}/stream")
    assert r.status_code == 200
    assert len(r.content) > 0
    assert "attachment" not in r.headers.get("content-disposition", "")


def test_stream_supports_range(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    r = auth_client.get(
        f"/api/library/tracks/{seeded_track_id}/stream",
        headers={"Range": "bytes=0-9"},
    )
    assert r.status_code == 206
    assert len(r.content) == 10


def test_cover_404_when_no_artwork(
    auth_client: TestClient, seeded_track_id: int
) -> None:
    """The seeded WAV has no embedded art and no cover.jpg next to it, so
    the cover endpoint returns 404 cleanly rather than a 500."""
    r = auth_client.get(f"/api/library/tracks/{seeded_track_id}/cover")
    assert r.status_code == 404


# --- upload + rescan -----------------------------------------------------


def test_upload_lands_in_destination_and_indexes(auth_client: TestClient) -> None:
    files = [("files", ("uploaded-1.wav", _silent_wav(), "audio/wav"))]
    r = auth_client.post(
        "/api/library/upload", files=files, params={"dest": "Uploads"}
    )
    assert r.status_code == 201
    body = r.json()
    assert body["destination"] == "Uploads"
    assert len(body["saved"]) == 1
    saved = body["saved"][0]
    assert saved["path"] == "Uploads/uploaded-1.wav"

    # Indexed and visible via search.
    paths = {
        t["path"]
        for t in auth_client.get(
            "/api/library/search", params={"q": "uploaded-1"}
        ).json()["tracks"]
    }
    assert "Uploads/uploaded-1.wav" in paths


def test_upload_dedupe_collisions(auth_client: TestClient) -> None:
    files1 = [("files", ("dup.wav", _silent_wav(), "audio/wav"))]
    files2 = [("files", ("dup.wav", _silent_wav(), "audio/wav"))]
    auth_client.post("/api/library/upload", files=files1, params={"dest": "DupTest"})
    r = auth_client.post(
        "/api/library/upload", files=files2, params={"dest": "DupTest"}
    )
    assert r.status_code == 201
    assert r.json()["saved"][0]["path"] == "DupTest/dup-1.wav"


def test_upload_conflict_overwrite_replaces_in_place(auth_client: TestClient) -> None:
    files1 = [("files", ("dup.wav", _silent_wav(), "audio/wav"))]
    auth_client.post("/api/library/upload", files=files1, params={"dest": "OverTest"})
    files2 = [("files", ("dup.wav", _silent_wav(), "audio/wav"))]
    r = auth_client.post(
        "/api/library/upload",
        files=files2,
        params={"dest": "OverTest", "conflict": "overwrite"},
    )
    assert r.status_code == 201
    body = r.json()
    assert body["saved"][0]["path"] == "OverTest/dup.wav"
    assert body["skipped"] == []
    # No -1 copy was minted.
    music_dir = Path(os.environ["MUSIC_DIR"])
    assert not (music_dir / "OverTest" / "dup-1.wav").exists()


def test_upload_conflict_skip_keeps_existing(auth_client: TestClient) -> None:
    files1 = [("files", ("dup.wav", _silent_wav(), "audio/wav"))]
    auth_client.post("/api/library/upload", files=files1, params={"dest": "SkipTest"})
    files2 = [("files", ("dup.wav", _silent_wav(), "audio/wav"))]
    r = auth_client.post(
        "/api/library/upload",
        files=files2,
        params={"dest": "SkipTest", "conflict": "skip"},
    )
    assert r.status_code == 201
    body = r.json()
    assert body["saved"] == []
    assert body["skipped"] == ["dup.wav"]
    music_dir = Path(os.environ["MUSIC_DIR"])
    assert (music_dir / "SkipTest" / "dup.wav").exists()
    assert not (music_dir / "SkipTest" / "dup-1.wav").exists()


def test_upload_check_reports_only_existing(auth_client: TestClient) -> None:
    auth_client.post(
        "/api/library/upload",
        files=[("files", ("a.wav", _silent_wav(), "audio/wav"))],
        params={"dest": "CheckTest"},
    )
    r = auth_client.post(
        "/api/library/upload/check",
        json={
            "items": [
                {"dest": "CheckTest", "name": "a.wav"},
                {"dest": "CheckTest", "name": "b.wav"},
            ]
        },
    )
    assert r.status_code == 200
    assert r.json()["collisions"] == [{"dest": "CheckTest", "name": "a.wav"}]


def test_upload_rejects_path_traversal(auth_client: TestClient) -> None:
    files = [("files", ("../escape.wav", _silent_wav(), "audio/wav"))]
    r = auth_client.post(
        "/api/library/upload", files=files, params={"dest": "Uploads"}
    )
    # Either rejected outright or basename-only stored; either way nothing
    # escapes the music dir.
    music_dir = Path(os.environ["MUSIC_DIR"])
    assert not (music_dir.parent / "escape.wav").exists()
    if r.status_code == 201:
        for saved in r.json()["saved"]:
            assert ".." not in saved["path"]


def test_upload_rejects_traversal_in_dest(auth_client: TestClient) -> None:
    files = [("files", ("a.wav", _silent_wav(), "audio/wav"))]
    r = auth_client.post(
        "/api/library/upload", files=files, params={"dest": "../escape"}
    )
    assert r.status_code == 400


def test_rescan_finds_files_dropped_via_filesystem(auth_client: TestClient) -> None:
    """SFTP / scp use case — operator drops a file directly under MUSIC_DIR
    and hits Rescan. The new file appears."""
    music_dir = Path(os.environ["MUSIC_DIR"])
    sftp_dir = music_dir / "Direct"
    sftp_dir.mkdir(exist_ok=True)
    target = sftp_dir / "dropped.wav"
    target.write_bytes(_silent_wav())

    r = auth_client.post("/api/library/rescan")
    assert r.status_code == 200
    body = r.json()
    # +1 added at least (could be more if other tests left files behind).
    assert body["added"] >= 1

    paths = {
        t["path"]
        for t in auth_client.get(
            "/api/library/search", params={"q": "dropped"}
        ).json()["tracks"]
    }
    assert "Direct/dropped.wav" in paths


# --- metadata edit + delete ----------------------------------------------


def test_metadata_edit_writes_tags_and_reindexes(auth_client: TestClient) -> None:
    files = [("files", ("editme.wav", _silent_wav(), "audio/wav"))]
    upload = auth_client.post(
        "/api/library/upload", files=files, params={"dest": "Edits"}
    ).json()
    track_id = upload["saved"][0]["id"]

    r = auth_client.patch(
        f"/api/library/tracks/{track_id}/metadata",
        json={"title": "My Custom Title", "artist": "Some Artist"},
    )
    assert r.status_code == 200
    body = r.json()
    assert body["title"] == "My Custom Title"
    assert body["artist"] == "Some Artist"

    # Persisted across re-fetch.
    fresh = auth_client.get(f"/api/library/tracks/{track_id}").json()
    assert fresh["title"] == "My Custom Title"


def test_metadata_edit_404_for_unknown_track(auth_client: TestClient) -> None:
    r = auth_client.patch(
        "/api/library/tracks/999999/metadata", json={"title": "x"}
    )
    assert r.status_code == 404


def test_display_title_persists_independent_of_filename(auth_client: TestClient) -> None:
    """`display_title` is a DB-only override — set it once, then move the
    file or re-tag it; the value sticks because it never round-trips through
    ID3 or the filename."""
    files = [("files", ("first-name.wav", _silent_wav(), "audio/wav"))]
    upload = auth_client.post(
        "/api/library/upload", files=files, params={"dest": "Display"}
    ).json()
    track_id = upload["saved"][0]["id"]

    r = auth_client.patch(
        f"/api/library/tracks/{track_id}/metadata",
        json={"display_title": "Battle Theme A"},
    )
    assert r.status_code == 200
    assert r.json()["display_title"] == "Battle Theme A"
    # Tag-derived title untouched.
    assert r.json()["title"] == "first-name"

    # Move + rename the file. The display_title must survive — the row id
    # is stable because index.update_path rewires it on rename.
    moved = auth_client.post(
        f"/api/library/tracks/{track_id}/move",
        json={"destination": "DisplayMoved", "new_filename": "renamed.wav"},
    ).json()
    assert moved["display_title"] == "Battle Theme A"


def test_origin_field_round_trips(auth_client: TestClient) -> None:
    files = [("files", ("origin-test.wav", _silent_wav(), "audio/wav"))]
    upload = auth_client.post(
        "/api/library/upload", files=files, params={"dest": "Origins"}
    ).json()
    track_id = upload["saved"][0]["id"]

    r = auth_client.patch(
        f"/api/library/tracks/{track_id}/metadata",
        json={"origin": "Skyrim"},
    )
    assert r.status_code == 200
    assert r.json()["origin"] == "Skyrim"

    fresh = auth_client.get(f"/api/library/tracks/{track_id}").json()
    assert fresh["origin"] == "Skyrim"


def test_search_matches_origin(auth_client: TestClient) -> None:
    files = [("files", ("orig-search.wav", _silent_wav(), "audio/wav"))]
    upload = auth_client.post(
        "/api/library/upload", files=files, params={"dest": "OriginSearch"}
    ).json()
    track_id = upload["saved"][0]["id"]
    auth_client.patch(
        f"/api/library/tracks/{track_id}/metadata",
        json={"origin": "Hollow Knight OST"},
    )

    r = auth_client.get("/api/library/search", params={"q": "hollow knight"})
    assert r.status_code == 200
    ids = {t["id"] for t in r.json()["tracks"]}
    assert track_id in ids


def test_bulk_metadata_sets_artist_across_selection(auth_client: TestClient) -> None:
    ids: list[int] = []
    for n in range(3):
        files = [("files", (f"bulk-{n}.wav", _silent_wav(), "audio/wav"))]
        upload = auth_client.post(
            "/api/library/upload", files=files, params={"dest": "BulkArtist"}
        ).json()
        ids.append(upload["saved"][0]["id"])

    r = auth_client.patch(
        "/api/library/tracks/bulk-metadata",
        json={
            "track_ids": ids,
            "updates": {"artist": "John Williams", "origin": "Star Wars"},
        },
    )
    assert r.status_code == 200
    body = r.json()
    assert body["skipped"] == []
    assert len(body["updated"]) == 3
    for row in body["updated"]:
        assert row["artist"] == "John Williams"
        assert row["origin"] == "Star Wars"

    # Verify each survives a re-fetch (origin is DB-only; artist is ID3 +
    # mirrored). This proves the write-and-rescan path didn't blow either
    # of them away.
    for tid in ids:
        fresh = auth_client.get(f"/api/library/tracks/{tid}").json()
        assert fresh["artist"] == "John Williams"
        assert fresh["origin"] == "Star Wars"


def test_bulk_metadata_rejects_empty_track_ids(auth_client: TestClient) -> None:
    r = auth_client.patch(
        "/api/library/tracks/bulk-metadata",
        json={"track_ids": [], "updates": {"artist": "x"}},
    )
    assert r.status_code == 422


def test_bulk_metadata_rejects_empty_updates(auth_client: TestClient) -> None:
    upload = auth_client.post(
        "/api/library/upload",
        files=[("files", ("bm-empty.wav", _silent_wav(), "audio/wav"))],
        params={"dest": "BulkEmpty"},
    ).json()
    track_id = upload["saved"][0]["id"]
    r = auth_client.patch(
        "/api/library/tracks/bulk-metadata",
        json={"track_ids": [track_id], "updates": {}},
    )
    assert r.status_code == 400


def test_bulk_metadata_404_when_no_ids_match(auth_client: TestClient) -> None:
    r = auth_client.patch(
        "/api/library/tracks/bulk-metadata",
        json={"track_ids": [9999991, 9999992], "updates": {"artist": "x"}},
    )
    assert r.status_code == 404


def test_bulk_metadata_partial_failure_when_file_deleted(
    auth_client: TestClient,
) -> None:
    """If a file vanishes between selection and apply, the response should
    surface that track in `skipped` with a reason — not silently drop it.
    Exercises the per-track failure path added when bulk-metadata moved
    from `list[TrackOut]` to `{updated, skipped}`."""
    ids: list[int] = []
    paths: list[str] = []
    for n in range(3):
        upload = auth_client.post(
            "/api/library/upload",
            files=[("files", (f"partial-{n}.wav", _silent_wav(), "audio/wav"))],
            params={"dest": "BulkPartial"},
        ).json()
        saved = upload["saved"][0]
        ids.append(saved["id"])
        paths.append(saved["path"])

    # Yank the second file from disk before the bulk update runs. The row
    # stays in the DB so it's still in the selection.
    music_dir = Path(os.environ["MUSIC_DIR"])
    (music_dir / paths[1]).unlink()

    r = auth_client.patch(
        "/api/library/tracks/bulk-metadata",
        json={
            "track_ids": ids,
            "updates": {"artist": "Hans Zimmer", "origin": "Inception"},
        },
    )
    assert r.status_code == 200
    body = r.json()

    # The two surviving files take the change; the deleted one is skipped
    # with a useful reason. DB-only fields (origin) still apply across the
    # whole selection — that's the documented "best-effort" semantic.
    updated_ids = {row["id"] for row in body["updated"]}
    assert updated_ids == set(ids)  # origin applied to all 3
    skipped = body["skipped"]
    assert len(skipped) == 1
    assert skipped[0]["track_id"] == ids[1]
    assert "missing" in skipped[0]["reason"].lower()

    # The two surviving files have BOTH artist (tag-backed) and origin
    # (DB-only). The deleted-file row only got origin (tag write skipped).
    for tid, p in zip(ids, paths, strict=True):
        fresh = auth_client.get(f"/api/library/tracks/{tid}").json()
        assert fresh["origin"] == "Inception"
        if (music_dir / p).is_file():
            assert fresh["artist"] == "Hans Zimmer"


def test_track_move_renames_and_relocates(auth_client: TestClient) -> None:
    files = [("files", ("movable.wav", _silent_wav(), "audio/wav"))]
    upload = auth_client.post(
        "/api/library/upload", files=files, params={"dest": "MoveSrc"}
    ).json()
    track_id = upload["saved"][0]["id"]

    r = auth_client.post(
        f"/api/library/tracks/{track_id}/move",
        json={"destination": "MoveDest", "new_filename": "renamed.wav"},
    )
    assert r.status_code == 200
    body = r.json()
    assert body["path"] == "MoveDest/renamed.wav"

    music_dir = Path(os.environ["MUSIC_DIR"])
    assert (music_dir / "MoveDest" / "renamed.wav").is_file()
    assert not (music_dir / "MoveSrc" / "movable.wav").exists()


def test_track_rename_in_place(auth_client: TestClient) -> None:
    """Renaming without relocating: destination == the file's current folder,
    only the filename changes. This is the path the TagInspector's rename
    control drives."""
    upload = auth_client.post(
        "/api/library/upload",
        files=[("files", ("oldname.wav", _silent_wav(), "audio/wav"))],
        params={"dest": "RenameHere"},
    ).json()
    track_id = upload["saved"][0]["id"]

    r = auth_client.post(
        f"/api/library/tracks/{track_id}/move",
        json={"destination": "RenameHere", "new_filename": "newname.wav"},
    )
    assert r.status_code == 200
    assert r.json()["path"] == "RenameHere/newname.wav"

    music_dir = Path(os.environ["MUSIC_DIR"])
    assert (music_dir / "RenameHere" / "newname.wav").is_file()
    assert not (music_dir / "RenameHere" / "oldname.wav").exists()


def test_track_move_409_when_target_exists(auth_client: TestClient) -> None:
    auth_client.post(
        "/api/library/upload",
        files=[("files", ("a.wav", _silent_wav(), "audio/wav"))],
        params={"dest": "Conflict"},
    )
    upload = auth_client.post(
        "/api/library/upload",
        files=[("files", ("b.wav", _silent_wav(), "audio/wav"))],
        params={"dest": "Conflict"},
    ).json()
    track_id = upload["saved"][0]["id"]
    r = auth_client.post(
        f"/api/library/tracks/{track_id}/move",
        json={"destination": "Conflict", "new_filename": "a.wav"},
    )
    assert r.status_code == 409


def test_delete_removes_file_and_row(auth_client: TestClient) -> None:
    files = [("files", ("doomed.wav", _silent_wav(), "audio/wav"))]
    upload = auth_client.post(
        "/api/library/upload", files=files, params={"dest": "Doomed"}
    ).json()
    track_id = upload["saved"][0]["id"]

    music_dir = Path(os.environ["MUSIC_DIR"])
    assert (music_dir / "Doomed" / "doomed.wav").is_file()

    r = auth_client.delete(f"/api/library/tracks/{track_id}")
    assert r.status_code == 204
    assert not (music_dir / "Doomed" / "doomed.wav").exists()
    assert auth_client.get(f"/api/library/tracks/{track_id}").status_code == 404


def test_bulk_move_relocates_tracks(auth_client: TestClient) -> None:
    ids: list[int] = []
    for n in range(3):
        upload = auth_client.post(
            "/api/library/upload",
            files=[("files", (f"bm-{n}.wav", _silent_wav(), "audio/wav"))],
            params={"dest": "BulkMoveSrc"},
        ).json()
        ids.append(upload["saved"][0]["id"])

    r = auth_client.post(
        "/api/library/tracks/bulk-move",
        json={"track_ids": ids, "destination": "BulkMoveDst"},
    )
    assert r.status_code == 200
    body = r.json()
    assert body["skipped"] == []
    assert len(body["moved"]) == 3
    music_dir = Path(os.environ["MUSIC_DIR"])
    for n in range(3):
        assert (music_dir / "BulkMoveDst" / f"bm-{n}.wav").is_file()
        assert not (music_dir / "BulkMoveSrc" / f"bm-{n}.wav").exists()


def test_bulk_move_skips_collisions(auth_client: TestClient) -> None:
    """When a destination file already exists, that single track is skipped
    with a reason while the rest still move."""
    src_id = auth_client.post(
        "/api/library/upload",
        files=[("files", ("collide.wav", _silent_wav(), "audio/wav"))],
        params={"dest": "MoveSrcA"},
    ).json()["saved"][0]["id"]
    # Pre-place a same-named file at the destination.
    auth_client.post(
        "/api/library/upload",
        files=[("files", ("collide.wav", _silent_wav(), "audio/wav"))],
        params={"dest": "MoveDstA"},
    )
    other_id = auth_client.post(
        "/api/library/upload",
        files=[("files", ("solo.wav", _silent_wav(), "audio/wav"))],
        params={"dest": "MoveSrcA"},
    ).json()["saved"][0]["id"]

    r = auth_client.post(
        "/api/library/tracks/bulk-move",
        json={"track_ids": [src_id, other_id], "destination": "MoveDstA"},
    )
    assert r.status_code == 200
    body = r.json()
    assert len(body["moved"]) == 1
    assert body["moved"][0]["id"] == other_id
    assert len(body["skipped"]) == 1
    assert body["skipped"][0]["track_id"] == src_id
    assert "exists" in body["skipped"][0]["reason"]


def test_bulk_delete_removes_files_and_rows(auth_client: TestClient) -> None:
    ids: list[int] = []
    for n in range(3):
        upload = auth_client.post(
            "/api/library/upload",
            files=[("files", (f"bd-{n}.wav", _silent_wav(), "audio/wav"))],
            params={"dest": "BulkDelete"},
        ).json()
        ids.append(upload["saved"][0]["id"])

    music_dir = Path(os.environ["MUSIC_DIR"])
    r = auth_client.post(
        "/api/library/tracks/bulk-delete",
        json={"track_ids": ids},
    )
    assert r.status_code == 200
    body = r.json()
    assert body["skipped"] == []
    assert sorted(body["deleted_ids"]) == sorted(ids)
    for n in range(3):
        assert not (music_dir / "BulkDelete" / f"bd-{n}.wav").exists()
    for tid in ids:
        assert auth_client.get(f"/api/library/tracks/{tid}").status_code == 404


def test_bulk_delete_handles_missing_file(auth_client: TestClient) -> None:
    """A missing file isn't fatal — the row still goes so the operator can
    sweep dangling rows after deleting files outside the app."""
    upload = auth_client.post(
        "/api/library/upload",
        files=[("files", ("ghost.wav", _silent_wav(), "audio/wav"))],
        params={"dest": "Ghosts"},
    ).json()
    track_id = upload["saved"][0]["id"]
    music_dir = Path(os.environ["MUSIC_DIR"])
    (music_dir / "Ghosts" / "ghost.wav").unlink()

    r = auth_client.post(
        "/api/library/tracks/bulk-delete",
        json={"track_ids": [track_id]},
    )
    assert r.status_code == 200
    body = r.json()
    assert track_id in body["deleted_ids"]
    assert auth_client.get(f"/api/library/tracks/{track_id}").status_code == 404


# --- folder ops ----------------------------------------------------------


def test_create_folder(auth_client: TestClient) -> None:
    r = auth_client.post("/api/library/folders", json={"path": "NewFolder"})
    assert r.status_code == 201
    body = r.json()
    assert body["path"] == "NewFolder"
    assert body["track_count"] == 0
    music_dir = Path(os.environ["MUSIC_DIR"])
    assert (music_dir / "NewFolder").is_dir()


def test_create_nested_folder(auth_client: TestClient) -> None:
    r = auth_client.post(
        "/api/library/folders", json={"path": "Parent/Child/Grandchild"}
    )
    assert r.status_code == 201
    music_dir = Path(os.environ["MUSIC_DIR"])
    assert (music_dir / "Parent" / "Child" / "Grandchild").is_dir()


def test_create_folder_rejects_traversal(auth_client: TestClient) -> None:
    r = auth_client.post("/api/library/folders", json={"path": "../escape"})
    assert r.status_code in (400, 422)


def test_delete_folder_empty(auth_client: TestClient) -> None:
    auth_client.post("/api/library/folders", json={"path": "ToDelete"})
    r = auth_client.delete("/api/library/folders", params={"path": "ToDelete"})
    assert r.status_code == 200
    assert r.json()["removed_tracks"] == 0
    music_dir = Path(os.environ["MUSIC_DIR"])
    assert not (music_dir / "ToDelete").exists()


def test_delete_folder_non_empty_refuses_without_recursive(auth_client: TestClient) -> None:
    auth_client.post(
        "/api/library/upload",
        files=[("files", ("a.wav", _silent_wav(), "audio/wav"))],
        params={"dest": "FullFolder"},
    )
    r = auth_client.delete("/api/library/folders", params={"path": "FullFolder"})
    assert r.status_code == 400
    music_dir = Path(os.environ["MUSIC_DIR"])
    assert (music_dir / "FullFolder").is_dir()


def test_delete_folder_recursive_removes_tracks(auth_client: TestClient) -> None:
    auth_client.post(
        "/api/library/upload",
        files=[("files", ("a.wav", _silent_wav(), "audio/wav"))],
        params={"dest": "WipeMe"},
    )
    r = auth_client.delete(
        "/api/library/folders",
        params={"path": "WipeMe", "recursive": "true"},
    )
    assert r.status_code == 200
    assert r.json()["removed_tracks"] >= 1
    music_dir = Path(os.environ["MUSIC_DIR"])
    assert not (music_dir / "WipeMe").exists()


def test_rename_folder_updates_index(auth_client: TestClient) -> None:
    upload = auth_client.post(
        "/api/library/upload",
        files=[("files", ("song.wav", _silent_wav(), "audio/wav"))],
        params={"dest": "OldName"},
    ).json()
    track_id = upload["saved"][0]["id"]

    r = auth_client.post(
        "/api/library/folders/rename", json={"src": "OldName", "dst": "NewName"}
    )
    assert r.status_code == 200
    assert r.json()["path"] == "NewName"

    music_dir = Path(os.environ["MUSIC_DIR"])
    assert not (music_dir / "OldName").exists()
    assert (music_dir / "NewName" / "song.wav").is_file()

    track = auth_client.get(f"/api/library/tracks/{track_id}").json()
    assert track["path"] == "NewName/song.wav"


def test_rename_folder_conflict(auth_client: TestClient) -> None:
    auth_client.post("/api/library/folders", json={"path": "ExistsA"})
    auth_client.post("/api/library/folders", json={"path": "ExistsB"})
    r = auth_client.post(
        "/api/library/folders/rename", json={"src": "ExistsA", "dst": "ExistsB"}
    )
    assert r.status_code == 409


# --- regression: album-fallback preserves dots ---------------------------


def test_metadata_album_fallback_preserves_dots() -> None:
    """A WAV with no album tag falls back to its parent-folder name VERBATIM —
    dots included. `metadata_for` derives the album from `Path(parent_rel).name`,
    not a slugified form, so a folder literally named "Vol.1" must yield album
    "Vol.1", never "Vol1". A root-level file (parent == ".") yields an empty
    album, not "." and not the mangled folder name."""
    from app.library import index as library_index

    music_dir = Path(os.environ["MUSIC_DIR"])

    dotted = music_dir / "Vol.1"
    dotted.mkdir(exist_ok=True)
    song = dotted / "song.wav"
    song.write_bytes(_silent_wav())

    meta = library_index.metadata_for(song, root=music_dir)
    assert meta["album"] == "Vol.1"

    # Root-level file: parent resolves to "." → album must be empty, not "."
    # and not a mangled folder name.
    root_song = music_dir / "rootlevel.wav"
    root_song.write_bytes(_silent_wav())
    root_meta = library_index.metadata_for(root_song, root=music_dir)
    assert root_meta["album"] == ""


# --- regression: delete_track OSError -> 500, row preserved ---------------


def test_delete_track_unlink_failure_returns_500_and_keeps_row(
    auth_client: TestClient, monkeypatch,
) -> None:
    """When the on-disk unlink raises OSError, the endpoint must surface a 500
    (with a "unlink failed" detail) AND leave the DB row intact — the row is
    only dropped *after* a successful unlink, so a failed delete can't orphan
    the index.

    The endpoint guards the unlink behind `abs_path.is_file()`, so the file has
    to genuinely exist (a dir/symlink swap would make is_file() False and the
    delete would short-circuit to 204). To force the OSError branch
    deterministically on every OS, we make `Path.unlink` raise for this one
    real, existing file — no privileged file-locking tricks, no platform skew."""
    upload = auth_client.post(
        "/api/library/upload",
        files=[("files", ("undeletable.wav", _silent_wav(), "audio/wav"))],
        params={"dest": "Undeletable"},
    ).json()
    track_id = upload["saved"][0]["id"]
    rel = upload["saved"][0]["path"]

    music_dir = Path(os.environ["MUSIC_DIR"])
    target = (music_dir / rel).resolve()
    assert target.is_file()  # is_file() stays True so the unlink branch runs

    real_unlink = Path.unlink

    def fake_unlink(self: Path, *args, **kwargs):
        if self.resolve() == target:
            raise OSError("simulated unlink failure")
        return real_unlink(self, *args, **kwargs)

    monkeypatch.setattr(Path, "unlink", fake_unlink)

    r = auth_client.delete(f"/api/library/tracks/{track_id}")
    assert r.status_code == 500
    assert "unlink failed" in r.json()["detail"].lower()

    # The row survived — a failed unlink must not orphan the index. The
    # monkeypatch is auto-reverted by the fixture, so this re-fetch hits disk
    # normally and the row is still there.
    assert auth_client.get(f"/api/library/tracks/{track_id}").status_code == 200


# --- follow / play-folder library helpers --------------------------------
#
# The Extras/ fixture files (extra-2/3/4.wav) are contiguous in path order —
# nothing can sort between "Extras/extra-2.wav" and "Extras/extra-3.wav" — so
# these assertions hold regardless of what other tests leave in the shared
# session library.


def test_track_ids_under_folder_in_path_order(
    client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    from app.core.db import SessionLocal
    from app.library import index as library_index

    with SessionLocal() as db:
        ids = library_index.track_ids_under(db, "Extras")
    assert ids == extra_seeded_track_ids


def test_track_ids_under_empty_path_returns_whole_library(
    client: TestClient, seeded_track_id: int, extra_seeded_track_ids: list[int]
) -> None:
    from app.core.db import SessionLocal
    from app.library import index as library_index

    with SessionLocal() as db:
        ids = library_index.track_ids_under(db, "")
    # Whole-library load is a superset of every known track.
    assert seeded_track_id in ids
    for tid in extra_seeded_track_ids:
        assert tid in ids


def test_next_track_id_after_is_adjacent(
    client: TestClient, extra_seeded_track_ids: list[int]
) -> None:
    from app.core.db import SessionLocal
    from app.library import index as library_index

    with SessionLocal() as db:
        # Extras/extra-2 < extra-3 < extra-4, contiguous in path order.
        assert (
            library_index.next_track_id_after(db, "Extras/extra-2.wav")
            == extra_seeded_track_ids[1]
        )
        assert (
            library_index.next_track_id_after(db, "Extras/extra-3.wav")
            == extra_seeded_track_ids[2]
        )


def test_next_track_id_after_wraps_or_stops_at_end(
    client: TestClient, seeded_track_id: int
) -> None:
    from app.core.db import SessionLocal
    from app.library import index as library_index

    # A path that sorts after any real track exercises the end-of-library
    # branch deterministically without needing to know the last track.
    sentinel = "￿"
    with SessionLocal() as db:
        assert library_index.next_track_id_after(db, sentinel, wrap=False) is None
        # With wrap, follow loops back to the library's first track.
        assert library_index.next_track_id_after(db, sentinel, wrap=True) is not None


# Quiet pyflakes about io being imported for potential future test helpers.
_ = io
