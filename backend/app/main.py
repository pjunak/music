import logging
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware

from app.api import auth, health, library, modes, nicknames, playlists, presets
from app.core.config import get_settings
from app.core.db import SessionLocal
from app.modes import loader as modes_loader
from app.presets import loader as presets_loader
from app.sync import router as sync_router
from app.sync import state as sync_state


def _configure_logging(level: str) -> None:
    logging.basicConfig(
        level=level.upper(),
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )


@asynccontextmanager
async def lifespan(app: FastAPI) -> AsyncIterator[None]:
    settings = get_settings()
    _configure_logging(settings.log_level)
    modes_loader.load_all()
    presets_loader.load_all()
    await sync_state.machine.load(SessionLocal)
    yield


def create_app() -> FastAPI:
    settings = get_settings()
    app = FastAPI(
        title="Music",
        description="Self-hosted music player and DnD session orchestrator",
        version="0.1.0",
        lifespan=lifespan,
    )
    app.add_middleware(
        CORSMiddleware,
        allow_origins=settings.allowed_origins_list,
        allow_credentials=True,
        allow_methods=["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"],
        allow_headers=["*"],
    )
    app.include_router(health.router)
    app.include_router(auth.router)
    app.include_router(library.router)
    app.include_router(modes.router)
    app.include_router(nicknames.router)
    app.include_router(playlists.router)
    app.include_router(presets.router)
    app.include_router(sync_router.router)
    return app


app = create_app()
