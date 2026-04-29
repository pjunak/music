import logging
import os
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager

from fastapi import FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse
from fastapi.staticfiles import StaticFiles
from starlette.exceptions import HTTPException

from app.api import auth, health, library, modes, playlists, presets, sfx
from app.core.config import get_settings
from app.core.db import SessionLocal, engine
from app.library import index as library_index
from app.models import Base
from app.modes import loader as modes_loader
from app.presets import loader as presets_loader
from app.sync import router as sync_router
from app.sync import state as sync_state

logger = logging.getLogger(__name__)


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

    # Resolved storage paths up front. Surface them at INFO so a misconfigured
    # mount (e.g. MUSIC_DIR pointing at a path inside the writable layer
    # instead of a bind mount) is obvious in the container logs — we got
    # bitten once by uploads silently landing in the ephemeral layer and
    # vanishing on the next rebuild.
    music_dir = settings.music_dir.resolve()
    sfx_dir = settings.sfx_library_dir.resolve()
    logger.info("MUSIC_DIR=%s", music_dir)
    logger.info("SFX_LIBRARY_DIR=%s", sfx_dir)
    if "MUSIC_DIR" not in os.environ:
        logger.warning(
            "MUSIC_DIR is unset — using default %s. In a container this is "
            "almost certainly the writable layer; uploads will be lost on "
            "rebuild. Set MUSIC_DIR to a bind-mounted path.",
            music_dir,
        )
    if "SFX_LIBRARY_DIR" not in os.environ:
        logger.warning(
            "SFX_LIBRARY_DIR is unset — using default %s. Same caveat as "
            "MUSIC_DIR: must be bind-mounted to persist.",
            sfx_dir,
        )

    # No alembic — solo-deployed app where regenerable data dominates.
    # `create_all` is idempotent (skips existing tables), so first boot
    # creates everything and subsequent boots are a no-op. When the schema
    # changes incompatibly, the operator wipes app.db and re-creates the
    # auth user via `music-cli create-user`.
    Base.metadata.create_all(bind=engine)
    modes_loader.load_all()
    presets_loader.load_all()
    # Walk the music dir on boot so search/tree work immediately. A noop on
    # first deploy when MUSIC_DIR is empty.
    db = SessionLocal()
    try:
        library_index.scan_full(db)
    except Exception:
        logger.exception("startup library scan failed; continuing")
    finally:
        db.close()
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

    # Single-user app — surface unhandled exception details in the response
    # so the operator (you) can debug from the browser without server logs.
    # FastAPI's HTTPException handler is more specific, so this only catches
    # genuinely unhandled exceptions.
    @app.exception_handler(Exception)
    async def _unhandled(request: Request, exc: Exception) -> JSONResponse:
        logger.exception("unhandled exception in %s %s", request.method, request.url.path)
        return JSONResponse(
            status_code=500,
            content={"detail": f"{type(exc).__name__}: {exc}"},
        )

    app.include_router(health.router)
    app.include_router(auth.router)
    app.include_router(library.router)
    app.include_router(modes.router)
    app.include_router(playlists.router)
    app.include_router(presets.router)
    app.include_router(sfx.router)
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
