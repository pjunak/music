from functools import lru_cache
from pathlib import Path

from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(
        env_file=".env",
        env_file_encoding="utf-8",
        extra="ignore",
    )

    # NOTE: there is deliberately no SECRET_KEY. Sessions are opaque random
    # DB-backed tokens (app/core/security.py) — nothing is signed, so a
    # signing key would be theatre. If signed cookies/links ever land, add
    # the setting together with the feature.

    database_url: str = "sqlite:///./app.db"

    # The music library is the directory the app indexes and serves from.
    # Audio files placed under it (any depth) appear in the Library; uploads
    # land in `<music_dir>/<destination>/` and get indexed.
    music_dir: Path = Path("./music")

    # SFX assets used by mode soundboards live under their own root, separate
    # from music. Mode manifests refer to files relative to this directory.
    sfx_library_dir: Path = Path("./sfx")

    modes_dir: Path = Path("../modes")

    # Operator-curated registry of remembered devices + their audio-output
    # designations. A standalone JSON file (not in app.db) so it survives a
    # reinstall AND an app.db wipe — bind-mount it alongside the data volume.
    devices_file: Path = Path("./devices.json")

    # Read-only seed directory. When `modes_dir` is missing/empty on startup,
    # the seed dir's contents are copied across so a fresh deploy with a blank
    # bind-mount picks up the bundled defaults. Subsequent boots (where the dir
    # already has content) leave the operator's edits alone. EQ presets live
    # under each mode now, so they ride along inside the modes seed.
    modes_seed_dir: Path | None = None

    # Server-side end-of-track advancement (app/sync/advancer.py). On in
    # production; the test suite switches it off globally so timing-based
    # advances can't race unrelated assertions (a dedicated test re-enables it).
    advancer_enabled: bool = True

    allowed_origins: str = "http://localhost:5173"
    session_cookie_secure: bool = False
    session_cookie_domain: str | None = None
    session_cookie_name: str = "music_session"
    session_ttl_days: int = 30

    log_level: str = "info"

    @property
    def allowed_origins_list(self) -> list[str]:
        return [o.strip() for o in self.allowed_origins.split(",") if o.strip()]


@lru_cache
def get_settings() -> Settings:
    return Settings()
