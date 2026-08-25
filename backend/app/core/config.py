from functools import lru_cache
from pathlib import Path

from pydantic import Field, SecretStr
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
    # Secure by default: the documented deployment is HTTPS behind a reverse
    # proxy, so the session cookie should never ride plaintext HTTP. Set to
    # false only for a plain-HTTP LAN deployment with no TLS at all (modern
    # browsers still accept Secure cookies on http://localhost, so local dev
    # via the Vite proxy is unaffected).
    session_cookie_secure: bool = True
    session_cookie_domain: str | None = None
    session_cookie_name: str = "music_session"
    session_ttl_days: int = 30

    # Separate deployment secret used only to encrypt optional Assistant
    # provider credentials at rest. It is a URL-safe base64 encoded 32-byte
    # AES key and deliberately has no default. The application remains fully
    # local when it is absent; the UI cannot store provider credentials.
    assistant_credential_key: SecretStr | None = None

    # Optional fixed file used when the operator wants the authenticated UI to
    # initialize credential storage. The path itself is not secret. Production
    # should bind-mount a dedicated host directory here; the app creates only
    # the final key file and never chooses or changes this path through the API.
    assistant_credential_key_file: Path | None = None

    # Optional non-secret display hint for deployment-specific setup guidance.
    # This is a host path and is never used for application filesystem access.
    assistant_credential_host_directory_hint: str | None = None

    # Optional local-only music voice/instrumental classifier. The application
    # accepts only the documented, checksum-pinned model and never downloads it
    # at runtime. The dependency and model carry licenses that require an
    # explicit operator choice, so the standard image leaves this unset.
    assistant_voice_model_path: Path | None = None

    # Whole-track context analysis is CPU-heavy pure Python. Independent
    # processes can use multiple cores while the parent remains the sole owner
    # of SQLite writes. Generic installs stay conservative; production may opt
    # into up to four workers after assigning matching CPU and memory capacity.
    assistant_library_context_workers: int = Field(default=1, ge=1, le=4)

    # Upload guard rails (per request). Generous enough for hi-res FLAC albums;
    # they exist to stop an authenticated client from exhausting the volume
    # backing MUSIC_DIR/SFX_LIBRARY_DIR with one unbounded request.
    max_upload_files: int = 500
    max_upload_file_bytes: int = 1024**3  # 1 GiB per file

    log_level: str = "info"

    @property
    def allowed_origins_list(self) -> list[str]:
        return [o.strip() for o in self.allowed_origins.split(",") if o.strip()]


@lru_cache
def get_settings() -> Settings:
    return Settings()
