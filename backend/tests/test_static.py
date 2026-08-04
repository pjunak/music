from pathlib import Path

import pytest
from fastapi.testclient import TestClient


def _production_client(
    static_dir: Path, monkeypatch: pytest.MonkeyPatch
) -> TestClient:
    static_dir.mkdir()
    (static_dir / "index.html").write_text(
        "<!doctype html><title>Music test shell</title><div id='root'></div>",
        encoding="utf-8",
    )
    assets = static_dir / "assets"
    assets.mkdir()
    (assets / "app.js").write_text("window.musicLoaded = true;", encoding="utf-8")
    monkeypatch.setenv("STATIC_DIR", str(static_dir))

    from app.main import create_app

    return TestClient(create_app())


def test_production_spa_mount_serves_assets_and_client_routes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    client = _production_client(tmp_path / "static", monkeypatch)

    root = client.get("/")
    assert root.status_code == 200
    assert "Music test shell" in root.text

    client_route = client.get("/settings/playback")
    assert client_route.status_code == 200
    assert client_route.text == root.text

    asset = client.get("/assets/app.js")
    assert asset.status_code == 200
    assert asset.text == "window.musicLoaded = true;"


def test_production_spa_mount_does_not_shadow_api_routes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    client = _production_client(tmp_path / "static", monkeypatch)

    response = client.get("/api/health")

    assert response.status_code == 200
    assert response.json() == {"status": "ok"}
    assert "Music test shell" not in response.text
