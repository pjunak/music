"""Test harness.

Points the app at an in-memory SQLite app DB and a tmp Beets DB stub so
tests don't need any external state. One throwaway track is seeded into
Beets so library/streaming tests have something to query.
"""
import os
import sqlite3
import tempfile
from collections.abc import Iterator
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

TEST_USERNAME = "tester"
TEST_PASSWORD = "test-password"


@pytest.fixture(autouse=True, scope="session")
def _test_env() -> Iterator[None]:
    tmp = Path(tempfile.mkdtemp(prefix="music-test-"))

    beets_db = tmp / "beets.db"
    sqlite3.connect(beets_db).close()

    os.environ["SECRET_KEY"] = "x" * 64
    os.environ["DATABASE_URL"] = f"sqlite:///{tmp / 'app.db'}"
    os.environ["BEETS_LIBRARY_DB"] = str(beets_db)
    os.environ["MODES_DIR"] = str(tmp / "modes")
    os.environ["INCOMING_DIR"] = str(tmp / "incoming")
    os.environ["LIBRARY_DIR"] = str(tmp / "library")
    os.environ["PRESETS_DIR"] = str(tmp / "presets")
    (tmp / "modes").mkdir()
    (tmp / "incoming").mkdir()
    (tmp / "library").mkdir()
    (tmp / "presets").mkdir()

    # Seed a test mode with theme, two soundboards, and a scene so the
    # corresponding endpoints have content to return.
    dnd_dir = tmp / "modes" / "dnd"
    dnd_dir.mkdir()
    (dnd_dir / "manifest.yaml").write_text(
        "id: dnd\n"
        "name: Test DnD Mode\n"
        "theme: theme.css\n"
        "panels: [now-playing]\n"
        "default_crossfade_ms: 1500\n"
        "default_soundboard: tavern\n",
        encoding="utf-8",
    )
    (dnd_dir / "theme.css").write_text(
        ":root[data-mode='dnd'] { --bg: #000; }\n",
        encoding="utf-8",
    )
    (dnd_dir / "soundboards").mkdir()
    (dnd_dir / "soundboards" / "tavern.yaml").write_text(
        "name: Tavern\n"
        "categories:\n"
        "  - id: doors\n"
        "    name: Doors\n"
        "    items:\n"
        "      - file: door.ogg\n"
        "        name: Door slam\n"
        "        hotkey: d\n",
        encoding="utf-8",
    )
    (dnd_dir / "soundboards" / "dungeon.yaml").write_text(
        "name: Dungeon\n"
        "categories:\n"
        "  - id: combat\n"
        "    name: Combat\n"
        "    items:\n"
        "      - file: sword.ogg\n"
        "        name: Swords clash\n",
        encoding="utf-8",
    )
    # Real bytes for door.ogg so the SFX stream endpoint has something to
    # serve. sword.ogg deliberately not written — exercises the 410 path.
    (dnd_dir / "soundboards" / "files").mkdir()
    (dnd_dir / "soundboards" / "files" / "door.ogg").write_bytes(b"FAKEOGGDATA" * 50)
    (dnd_dir / "scenes").mkdir()
    (dnd_dir / "scenes" / "tavern.yaml").write_text(
        "name: Stonehill Inn\n"
        "ambient: { playlist: tavern-music, crossfade_ms: 2500 }\n"
        "presets: [radio-vintage]\n",
        encoding="utf-8",
    )

    # Seed two preset YAMLs for test_presets.
    (tmp / "presets" / "cave.yaml").write_text(
        "id: cave\nname: Cave\neffects:\n  - type: reverb\n    wet: 0.4\n",
        encoding="utf-8",
    )
    (tmp / "presets" / "radio-vintage.yaml").write_text(
        "id: radio-vintage\nname: Vintage Radio\neffects:\n  - type: highpass\n    frequency: 400\n",
        encoding="utf-8",
    )

    # Reset cached settings so the values above take effect.
    from app.core import config

    config.get_settings.cache_clear()

    # Create our app's schema. Tests don't run alembic — that's covered by a
    # separate manual smoke. Here we trust the model declarations match the
    # migration.
    from app.core.db import engine
    from app.models import Base

    Base.metadata.create_all(bind=engine)

    # Seed a single Beets item pointing at a real (tiny) audio file so the
    # streaming endpoint has bytes to serve.
    audio_dir = tmp / "audio"
    audio_dir.mkdir()
    audio_path = audio_dir / "test.mp3"
    audio_path.write_bytes(b"FAKEMP3DATA" * 100)  # 1100 bytes

    from beets.library import Item, Library

    lib = Library(str(beets_db))
    with lib.transaction():
        item = Item(
            title="Test Song",
            artist="Test Artist",
            albumartist="Test Artist",
            album="Test Album",
            track=1,
            length=180.0,
        )
        item.path = str(audio_path).encode()
        lib.add(item)
    lib._close()

    yield


@pytest.fixture
def client() -> Iterator[TestClient]:
    """TestClient as a context manager — without `with`, FastAPI's lifespan
    (modes loader, future startup hooks) never runs."""
    from app.main import app

    with TestClient(app) as c:
        yield c


@pytest.fixture
def db_session():
    from app.core.db import SessionLocal

    with SessionLocal() as session:
        yield session


@pytest.fixture
def seeded_track_id() -> int:
    """The id of the primary seeded track ('Test Song'). Stable across runs
    regardless of how many extras have been seeded."""
    from app.library import beets_adapter

    matches = [t for t in beets_adapter.search(limit=20) if t.title == "Test Song"]
    assert matches, "primary seeded track 'Test Song' missing"
    return matches[0].beets_id


@pytest.fixture(scope="session")
def extra_seeded_track_ids() -> list[int]:
    """Seed 3 additional tracks once per session and return their ids.

    Used by tests that need multiple tracks (playlist add/move/remove).
    Independent of the primary seeded track so existing single-track
    assertions stay valid.
    """
    from pathlib import Path

    from beets.library import Item, Library

    beets_db = os.environ["BEETS_LIBRARY_DB"]
    audio_dir = Path(beets_db).parent / "audio"

    ids: list[int] = []
    lib = Library(beets_db)
    with lib.transaction():
        for n in range(2, 5):  # tracks 2, 3, 4
            audio_path = audio_dir / f"test{n}.mp3"
            audio_path.write_bytes(b"FAKEMP3DATA" * 100)
            item = Item(
                title=f"Extra Song {n}",
                artist="Extra Artist",
                albumartist="Extra Artist",
                album="Extra Album",
                track=n,
                length=180.0,
            )
            item.path = str(audio_path).encode()
            lib.add(item)
            ids.append(item.id)
    lib._close()
    return ids


@pytest.fixture
def auth_client(client: TestClient) -> TestClient:
    from sqlalchemy import select

    from app.core.db import SessionLocal
    from app.core.security import hash_password
    from app.models.user import User

    with SessionLocal() as db:
        existing = db.scalar(select(User).where(User.username == TEST_USERNAME))
        if existing is None:
            db.add(User(username=TEST_USERNAME, password_hash=hash_password(TEST_PASSWORD)))
            db.commit()

    response = client.post(
        "/api/auth/login",
        json={"username": TEST_USERNAME, "password": TEST_PASSWORD},
    )
    assert response.status_code == 200, response.text
    return client
