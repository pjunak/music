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
    beets_library_db: Path

    incoming_dir: Path = Path("./incoming")
    library_dir: Path = Path("./library")
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
