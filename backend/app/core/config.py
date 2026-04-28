from functools import lru_cache
from pathlib import Path

from pydantic import Field
from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(
        env_file=".env",
        env_file_encoding="utf-8",
        extra="ignore",
    )

    secret_key: str = Field(min_length=32)

    database_url: str = "sqlite:///./app.db"

    # The music library is the directory the app indexes and serves from.
    # Audio files placed under it (any depth) appear in the Library; uploads
    # land in `<music_dir>/<destination>/` and get indexed.
    music_dir: Path = Path("./music")

    # SFX assets used by mode soundboards live under their own root, separate
    # from music. Mode manifests refer to files relative to this directory.
    sfx_library_dir: Path = Path("./sfx")

    modes_dir: Path = Path("../modes")
    presets_dir: Path = Path("../presets")

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
    return Settings()  # type: ignore[call-arg]
