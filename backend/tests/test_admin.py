"""Admin/backup endpoint."""
from __future__ import annotations

import io
import tarfile

from fastapi.testclient import TestClient


def test_backup_requires_auth(client: TestClient) -> None:
    r = client.get("/api/admin/backup")
    assert r.status_code == 401


def test_backup_includes_db_modes_presets(auth_client: TestClient) -> None:
    r = auth_client.get("/api/admin/backup")
    assert r.status_code == 200
    assert r.headers["content-type"].startswith("application/gzip")
    disposition = r.headers.get("content-disposition", "")
    assert "music-backup-" in disposition and ".tar.gz" in disposition

    with tarfile.open(fileobj=io.BytesIO(r.content), mode="r:gz") as tar:
        names = tar.getnames()
    # SQLite snapshot at the root.
    assert "app.db" in names
    # Modes and presets directories included.
    assert any(n == "modes" or n.startswith("modes/") for n in names)
    assert any(n == "presets" or n.startswith("presets/") for n in names)
