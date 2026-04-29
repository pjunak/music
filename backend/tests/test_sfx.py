"""SFX HTTP surface — playback (reference-gated) + management (free)."""
from __future__ import annotations

import os
from pathlib import Path

from fastapi.testclient import TestClient

# --- playback: reference-gated -----------------------------------------------


def test_playback_open_to_guest(client: TestClient) -> None:
    """SFX file playback is intentionally guest-accessible so a logged-out
    Player tab (TV bookmark) can render soundboard fires without the
    operator having to log in on the room display."""
    r = client.get("/api/sfx/file", params={"path": "dnd/door.ogg"})
    assert r.status_code == 200


def test_playback_serves_referenced_file(auth_client: TestClient) -> None:
    r = auth_client.get("/api/sfx/file", params={"path": "dnd/door.ogg"})
    assert r.status_code == 200
    assert len(r.content) > 0


def test_playback_404_for_unreferenced_path(auth_client: TestClient) -> None:
    r = auth_client.get("/api/sfx/file", params={"path": "dnd/totally_random.ogg"})
    assert r.status_code == 404


def test_playback_410_when_referenced_but_missing(auth_client: TestClient) -> None:
    r = auth_client.get("/api/sfx/file", params={"path": "dnd/sword.ogg"})
    assert r.status_code == 410


def test_playback_400_for_traversal(auth_client: TestClient) -> None:
    r = auth_client.get("/api/sfx/file", params={"path": "../escape.ogg"})
    assert r.status_code == 400


# --- management: tree / upload / move / delete / folders ---------------------


def test_tree_requires_auth(client: TestClient) -> None:
    assert client.get("/api/sfx/tree").status_code == 401


def test_tree_root_lists_seeded(auth_client: TestClient) -> None:
    """conftest seeds dnd/door.ogg under SFX_LIBRARY_DIR."""
    r = auth_client.get("/api/sfx/tree")
    assert r.status_code == 200
    body = r.json()
    folder_paths = {f["path"] for f in body["folders"]}
    assert "dnd" in folder_paths


def test_files_returns_flat_list_recursive(auth_client: TestClient) -> None:
    """Used by the soundboard editor to populate file pickers without
    walking the tree from the client."""
    auth_client.post(
        "/api/sfx/upload",
        files=[("files", ("nested.wav", b"x" * 8, "audio/wav"))],
        params={"dest": "deep/path"},
    )
    r = auth_client.get("/api/sfx/files")
    assert r.status_code == 200
    paths = {f["path"] for f in r.json()}
    assert "deep/path/nested.wav" in paths
    assert "dnd/door.ogg" in paths  # seeded


def test_tree_subfolder_includes_files_and_referenced_flag(auth_client: TestClient) -> None:
    r = auth_client.get("/api/sfx/tree", params={"path": "dnd"})
    body = r.json()
    files_by_path = {f["path"]: f for f in body["files"]}
    assert "dnd/door.ogg" in files_by_path
    assert files_by_path["dnd/door.ogg"]["referenced"] is True


def test_upload_writes_into_destination(auth_client: TestClient) -> None:
    files = [("files", ("ping.wav", b"FAKE-WAV-A" * 10, "audio/wav"))]
    r = auth_client.post(
        "/api/sfx/upload", files=files, params={"dest": "ambient/birds"}
    )
    assert r.status_code == 201
    saved = r.json()["saved"]
    assert saved[0]["path"] == "ambient/birds/ping.wav"

    sfx_dir = Path(os.environ["SFX_LIBRARY_DIR"])
    assert (sfx_dir / "ambient" / "birds" / "ping.wav").read_bytes() == b"FAKE-WAV-A" * 10


def test_upload_dedupes_collisions(auth_client: TestClient) -> None:
    auth_client.post(
        "/api/sfx/upload",
        files=[("files", ("dup.wav", b"first", "audio/wav"))],
        params={"dest": "dups"},
    )
    r = auth_client.post(
        "/api/sfx/upload",
        files=[("files", ("dup.wav", b"second", "audio/wav"))],
        params={"dest": "dups"},
    )
    assert r.json()["saved"][0]["path"] == "dups/dup-1.wav"


def test_upload_rejects_traversal_in_dest(auth_client: TestClient) -> None:
    r = auth_client.post(
        "/api/sfx/upload",
        files=[("files", ("a.wav", b"x", "audio/wav"))],
        params={"dest": "../escape"},
    )
    assert r.status_code == 400


def test_move_relocates(auth_client: TestClient) -> None:
    auth_client.post(
        "/api/sfx/upload",
        files=[("files", ("clip.wav", b"x" * 16, "audio/wav"))],
        params={"dest": "moveme"},
    )
    r = auth_client.post(
        "/api/sfx/move",
        json={"src": "moveme/clip.wav", "dst_folder": "stayhere", "new_filename": "renamed.wav"},
    )
    assert r.status_code == 200
    assert r.json()["path"] == "stayhere/renamed.wav"

    sfx_dir = Path(os.environ["SFX_LIBRARY_DIR"])
    assert (sfx_dir / "stayhere" / "renamed.wav").is_file()
    assert not (sfx_dir / "moveme" / "clip.wav").exists()


def test_delete_removes_file(auth_client: TestClient) -> None:
    auth_client.post(
        "/api/sfx/upload",
        files=[("files", ("doomed.wav", b"x", "audio/wav"))],
        params={"dest": "delme"},
    )
    r = auth_client.delete(
        "/api/sfx/files", params={"path": "delme/doomed.wav"}
    )
    assert r.status_code == 204
    sfx_dir = Path(os.environ["SFX_LIBRARY_DIR"])
    assert not (sfx_dir / "delme" / "doomed.wav").exists()


def test_create_folder(auth_client: TestClient) -> None:
    r = auth_client.post("/api/sfx/folders", json={"path": "Voices/Tavern"})
    assert r.status_code == 201
    sfx_dir = Path(os.environ["SFX_LIBRARY_DIR"])
    assert (sfx_dir / "Voices" / "Tavern").is_dir()


def test_delete_folder_recursive(auth_client: TestClient) -> None:
    auth_client.post(
        "/api/sfx/upload",
        files=[("files", ("inside.wav", b"x", "audio/wav"))],
        params={"dest": "rmme"},
    )
    r = auth_client.delete(
        "/api/sfx/folders", params={"path": "rmme", "recursive": "true"}
    )
    assert r.status_code == 200
    sfx_dir = Path(os.environ["SFX_LIBRARY_DIR"])
    assert not (sfx_dir / "rmme").exists()


def test_rename_folder(auth_client: TestClient) -> None:
    auth_client.post("/api/sfx/folders", json={"path": "rename_me"})
    r = auth_client.post(
        "/api/sfx/folders/rename", json={"src": "rename_me", "dst": "renamed"}
    )
    assert r.status_code == 200
    sfx_dir = Path(os.environ["SFX_LIBRARY_DIR"])
    assert not (sfx_dir / "rename_me").exists()
    assert (sfx_dir / "renamed").is_dir()
