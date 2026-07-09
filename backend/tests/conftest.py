"""Test harness.

Per-session: stand up an isolated tmp dir, point env vars at it (app DB,
music dir, sfx dir, modes dir, presets dir), seed some YAML for modes/
presets, drop a real silent WAV into the music dir, and create the schema.

Per-test: a `client` and an authenticated `auth_client`.

We use real WAV bytes so mutagen actually parses something; tests can rely
on length/title/etc behaviour rather than mocks.
"""
from __future__ import annotations

import os
import struct
import tempfile
from collections.abc import Iterator
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

TEST_USERNAME = "tester"
TEST_PASSWORD = "test-password"

# The env must point at the throwaway tmp dir BEFORE any app module can be
# imported. Pytest imports test modules at collection time, and app imports
# can happen there — directly (test_cleanup, test_tag_roundtrip) or by
# accident (collection hooks getattr() module-level objects, which can fire
# lazy importers). `app.core.db` binds its engine to settings AT IMPORT, so a
# half-configured env at that moment silently points every test at the repo's
# ./app.db. Module-level conftest code is the one thing guaranteed to run
# before collection; the session fixture below does the seeding/schema work.
_TMP = Path(tempfile.mkdtemp(prefix="music-test-"))
os.environ["DATABASE_URL"] = f"sqlite:///{_TMP / 'app.db'}"
os.environ["MUSIC_DIR"] = str(_TMP / "music")
os.environ["SFX_LIBRARY_DIR"] = str(_TMP / "sfx")
os.environ["MODES_DIR"] = str(_TMP / "modes")
os.environ["DEVICES_FILE"] = str(_TMP / "devices.json")
# Keep the end-of-track advancer out of unrelated tests: the seeded WAVs
# are 0.5 s long, so a playing lane would advance ~1.25 s into any test
# and inject unexpected broadcasts. test_advancer re-enables it locally.
os.environ["ADVANCER_ENABLED"] = "0"


def _silent_wav_bytes(seconds: float = 0.5, sample_rate: int = 8000) -> bytes:
    """Build a minimal valid WAV file in pure Python so we don't ship
    binary fixtures. mutagen can read these; reliably reports `length`."""
    n_samples = int(seconds * sample_rate)
    pcm = b"\x00\x00" * n_samples  # 16-bit silence, mono
    # WAV header (RIFF / fmt / data chunks).
    header = b"RIFF"
    header += struct.pack("<I", 36 + len(pcm))
    header += b"WAVE"
    header += b"fmt "
    header += struct.pack("<I", 16)            # subchunk1 size
    header += struct.pack("<H", 1)             # PCM
    header += struct.pack("<H", 1)             # mono
    header += struct.pack("<I", sample_rate)
    header += struct.pack("<I", sample_rate * 2)  # byte rate
    header += struct.pack("<H", 2)             # block align
    header += struct.pack("<H", 16)            # bits per sample
    header += b"data"
    header += struct.pack("<I", len(pcm))
    return header + pcm


@pytest.fixture(autouse=True, scope="session")
def _test_env() -> Iterator[None]:
    music_dir = _TMP / "music"
    sfx_dir = _TMP / "sfx"
    modes_dir = _TMP / "modes"
    music_dir.mkdir()
    sfx_dir.mkdir()
    modes_dir.mkdir()

    # Seed modes/dnd with theme + soundboards + EQ presets.
    dnd_dir = modes_dir / "dnd"
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
    # Soundboard items reference SFX paths relative to SFX_LIBRARY_DIR.
    (dnd_dir / "soundboards" / "tavern.yaml").write_text(
        "name: Tavern\n"
        "categories:\n"
        "  - id: doors\n"
        "    name: Doors\n"
        "    items:\n"
        "      - file: dnd/door.ogg\n"
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
        "      - file: dnd/sword.ogg\n"
        "        name: Swords clash\n",
        encoding="utf-8",
    )

    # SFX library: dnd/door.ogg is referenced and present; dnd/sword.ogg is
    # referenced but missing (exercises the 410 path). Use WAV bytes via the
    # ".ogg" extension here — the endpoint just streams whatever's at the
    # path; the soundboard ref check is what gates it.
    (sfx_dir / "dnd").mkdir()
    (sfx_dir / "dnd" / "door.ogg").write_bytes(b"FAKEOGGDATA" * 50)

    # Seed two EQ presets — now per-mode, under modes/dnd/presets/.
    (dnd_dir / "presets").mkdir()
    (dnd_dir / "presets" / "cave.yaml").write_text(
        "id: cave\nname: Cave\neffects:\n  - type: reverb\n    wet: 0.4\n",
        encoding="utf-8",
    )
    (dnd_dir / "presets" / "radio-vintage.yaml").write_text(
        "id: radio-vintage\nname: Vintage Radio\neffects:\n  - type: highpass\n    frequency: 400\n",
        encoding="utf-8",
    )

    # Seed one music file under MUSIC_DIR/Demo/.
    demo_dir = music_dir / "Demo"
    demo_dir.mkdir()
    (demo_dir / "test-song.wav").write_bytes(_silent_wav_bytes())

    # Reset cached settings + Library handle so the values above take effect.
    from app.core import config

    config.get_settings.cache_clear()

    # Create our app's schema.
    from app.core.db import engine
    from app.models import Base

    Base.metadata.create_all(bind=engine)

    yield


@pytest.fixture
def client() -> Iterator[TestClient]:
    """TestClient as a context manager — without `with`, FastAPI's lifespan
    (modes loader, library scan, etc.) never runs."""
    from app.main import app

    with TestClient(app) as c:
        yield c


@pytest.fixture
def db_session():
    from app.core.db import SessionLocal

    with SessionLocal() as session:
        yield session


@pytest.fixture
def seeded_track_id(client: TestClient) -> int:
    """The id of the primary seeded track. The lifespan startup scan
    indexes it; we just look it up by path."""
    from sqlalchemy import select

    from app.core.db import SessionLocal
    from app.models.track import Track

    with SessionLocal() as db:
        track = db.scalar(select(Track).where(Track.path == "Demo/test-song.wav"))
        assert track is not None, "primary seeded track not indexed"
        return track.id


@pytest.fixture
def extra_seeded_track_ids() -> list[int]:
    """Drop three extra silent WAVs into the library and index them.

    Returns the ids in walk order. A separate fixture so tests that don't
    need the extras don't pay the cost of indexing them."""
    from sqlalchemy import select

    from app.core.db import SessionLocal
    from app.library import index as library_index
    from app.models.track import Track

    music_dir = Path(os.environ["MUSIC_DIR"])
    extras = music_dir / "Extras"
    extras.mkdir(exist_ok=True)
    paths: list[Path] = []
    for n in range(2, 5):
        p = extras / f"extra-{n}.wav"
        if not p.exists():
            p.write_bytes(_silent_wav_bytes())
        paths.append(p)

    with SessionLocal() as db:
        library_index.scan_paths(db, paths)

    ids: list[int] = []
    with SessionLocal() as db:
        for p in paths:
            rel = library_index.to_relative(p)
            track = db.scalar(select(Track).where(Track.path == rel))
            assert track is not None
            ids.append(track.id)
    return ids


def reset_sync_singletons() -> None:
    """Reset the process-wide sync singletons (state machine, live device
    registry, connection manager), the persisted playback row, and the
    file-backed device store, so devices/designations from a prior test don't
    leak. Shared by the autouse fixtures in test_sync and test_devices."""
    from app.core.db import SessionLocal
    from app.devices.store import device_store
    from app.models.playback_state import PlaybackState
    from app.sync import loops as loops_manager
    from app.sync.connection import manager
    from app.sync.devices import registry
    from app.sync.state import machine

    machine.reset_for_tests()
    registry.reset_for_tests()
    manager.reset_for_tests()
    device_store.reset_for_tests()
    loops_manager.stop_all()
    with SessionLocal() as db:
        row = db.get(PlaybackState, 1)
        if row is not None:
            row.state_json = {}
            db.commit()


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
