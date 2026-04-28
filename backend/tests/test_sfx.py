"""SFX endpoint: serves files from SFX_LIBRARY_DIR with traversal +
soundboard-reference guards."""
from fastapi.testclient import TestClient


def test_requires_auth(client: TestClient) -> None:
    assert client.get("/api/sfx/file", params={"path": "dnd/door.ogg"}).status_code == 401


def test_serves_referenced_file(auth_client: TestClient) -> None:
    r = auth_client.get("/api/sfx/file", params={"path": "dnd/door.ogg"})
    assert r.status_code == 200
    assert len(r.content) > 0


def test_404_for_unreferenced_path(auth_client: TestClient) -> None:
    """Even if the file existed on disk, it must be referenced by a soundboard
    to be servable. dnd/totally_random.ogg is referenced by nothing."""
    r = auth_client.get("/api/sfx/file", params={"path": "dnd/totally_random.ogg"})
    assert r.status_code == 404


def test_410_when_referenced_but_missing(auth_client: TestClient) -> None:
    """dnd/sword.ogg is referenced by the dungeon soundboard but no bytes were
    seeded into the SFX library — exercises the 410 path."""
    r = auth_client.get("/api/sfx/file", params={"path": "dnd/sword.ogg"})
    assert r.status_code == 410


def test_400_for_traversal(auth_client: TestClient) -> None:
    r = auth_client.get("/api/sfx/file", params={"path": "../escape.ogg"})
    assert r.status_code == 400
