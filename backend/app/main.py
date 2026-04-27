import logging
import os
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from fastapi.staticfiles import StaticFiles
from starlette.exceptions import HTTPException

from app.api import auth, health, library, modes, nicknames, playlists, presets
from app.core.config import get_settings
from app.core.db import SessionLocal
from app.modes import loader as modes_loader
from app.presets import loader as presets_loader
from app.sync import router as sync_router
from app.sync import state as sync_state


class SpaStaticFiles(StaticFiles):
    """Serve a built React/Vite SPA — fall back to index.html on 404 so the
    client-side router can handle the route. Mounted at "/" after all API
    routers, so API paths still win."""

    async def get_response(self, path: str, scope):  # type: ignore[override]
        try:
            return await super().get_response(path, scope)
        except HTTPException as exc:
            if exc.status_code == 404:
                return await super().get_response("index.html", scope)
            raise


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

    # Mount the built frontend SPA last so API routes win. Falls back to
    # index.html on 404 so React Router can take over client-side routes.
    # In dev (no built static dir present), the mount is skipped so the
    # API still works via uvicorn alone.
    static_dir = os.environ.get("STATIC_DIR", "/app/static")
    if os.path.isdir(static_dir):
        app.mount("/", SpaStaticFiles(directory=static_dir, html=True), name="spa")

    return app


app = create_app()
