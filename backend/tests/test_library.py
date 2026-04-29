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


# Quiet pyflakes about io being imported for potential future test helpers.
_ = io
