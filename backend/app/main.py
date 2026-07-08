import logging
import os
import shutil
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, cast

from fastapi import FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse
from fastapi.staticfiles import StaticFiles
from sqlalchemy import CursorResult, delete
from starlette.exceptions import HTTPException

from app.api import (
    admin,
    auth,
    cleanup,
    devices,
    diagnostics,
    health,
    library,
    modes,
    playlists,
    sfx,
    sync,
)
from app.core.config import get_settings
from app.core.db import SessionLocal, engine
from app.devices.store import device_store
from app.library import index as library_index
from app.models import Base
from app.models.auth_session import AuthSession
from app.modes import loader as modes_loader
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


# Columns that were added to existing tables after the table's first
# release. `Base.metadata.create_all()` only creates *missing* tables — it
# never touches an existing table's columns. Listing additive changes here
# lets the lifespan ALTER them in idempotently, sparing the operator a full
# `app.db` wipe (which would erase users / playlists). Type changes, drops,
# and renames still require a wipe.
_ADDITIVE_COLUMNS: list[tuple[str, str, str]] = [
    ("tracks", "display_title", "VARCHAR(512) NOT NULL DEFAULT ''"),
    ("tracks", "origin", "VARCHAR(512) NOT NULL DEFAULT ''"),
]


def _apply_additive_columns() -> None:
    """ALTER TABLE … ADD COLUMN for any entry in `_ADDITIVE_COLUMNS` whose
    column doesn't yet exist. SQLite-only; on other dialects this is a noop
    (we don't deploy to anything else, but the guard keeps tests / dev runs
    against alternate DBs from blowing up)."""
    if engine.dialect.name != "sqlite":
        return
    with engine.begin() as conn:
        for table_name, col_name, col_def in _ADDITIVE_COLUMNS:
            existing = {
                row[1]
                for row in conn.exec_driver_sql(
                    f"PRAGMA table_info({table_name})"
                ).fetchall()
            }
            if not existing:
                # Table doesn't exist yet — `create_all` will build it
                # with the new column already present.
                continue
            if col_name in existing:
                continue
            conn.exec_driver_sql(
                f"ALTER TABLE {table_name} ADD COLUMN {col_name} {col_def}"
            )
            logger.info("schema: added %s.%s", table_name, col_name)


def _seed_if_empty(target: Path, seed: Path | None, label: str) -> None:
    """Copy `seed` into `target` only when `target` is missing or empty.

    Idempotent: a populated `target` is left strictly alone. This is the
    contract that lets operators bind-mount a host volume at `target`,
    keep their edits across image rebuilds, and still get the bundled
    defaults on a fresh first boot."""
    target.mkdir(parents=True, exist_ok=True)
    if seed is None:
        return
    if not seed.is_dir():
        logger.warning("%s seed dir %s does not exist; skipping seed", label, seed)
        return
    if any(target.iterdir()):
        return  # already populated — never wipe operator edits
    for entry in seed.iterdir():
        dest = target / entry.name
        try:
            if entry.is_dir():
                shutil.copytree(entry, dest)
            else:
                shutil.copy2(entry, dest)
        except OSError:
            logger.exception(
                "failed to seed %s from %s — continuing with what we copied",
                dest,
                entry,
            )
    logger.info("seeded empty %s from %s", label, seed)


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
    modes_dir = settings.modes_dir.resolve()
    logger.info("MUSIC_DIR=%s", music_dir)
    logger.info("SFX_LIBRARY_DIR=%s", sfx_dir)
    logger.info("MODES_DIR=%s", modes_dir)
    logger.info("DEVICES_FILE=%s", settings.devices_file.resolve())

    # Idempotent base-structure init. The lifespan only ever creates
    # missing dirs and (optionally) seeds initially-empty modes/presets;
    # once anything's in there, we don't touch the contents on subsequent
    # boots. Operator changes survive image rebuilds as long as these
    # paths are bind-mounted.
    music_dir.mkdir(parents=True, exist_ok=True)
    sfx_dir.mkdir(parents=True, exist_ok=True)
    _seed_if_empty(modes_dir, settings.modes_seed_dir, "modes")

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
    _apply_additive_columns()
    device_store.load()
    modes_loader.load_all()  # also loads each mode's per-mode EQ presets
    # Walk the music dir on boot so search/tree work immediately. A noop on
    # first deploy when MUSIC_DIR is empty.
    db = SessionLocal()
    try:
        library_index.scan_full(db)
    except Exception:
        logger.exception("startup library scan failed; continuing")
    try:
        # Sweep expired auth sessions on boot. Resolve-time cleanup handles
        # the steady state, but a long downtime can leave stale rows nobody
        # ever revisits.
        # Session.execute() is typed as Result, but DML always yields a
        # CursorResult (the only Result with a rowcount).
        result = cast(
            "CursorResult[Any]",
            db.execute(
                delete(AuthSession).where(AuthSession.expires_at <= datetime.now(UTC))
            ),
        )
        db.commit()
        if result.rowcount:
            logger.info("pruned %d expired auth session(s) on boot", result.rowcount)
    except Exception:
        logger.exception("startup auth session sweep failed; continuing")
        db.rollback()
    finally:
        db.close()
    await sync_state.machine.load(SessionLocal)
    # Server-side end-of-track advancement — the watchdog that keeps the
    # queue moving when no client can send (or successfully deliver) a skip.
    from app.sync.advancer import advancer

    if settings.advancer_enabled:
        advancer.start()
    yield
    # Shutdown: stop the advancer and cancel any running looping-SFX timers.
    await advancer.stop()
    from app.sync import loops as loops_manager

    loops_manager.stop_all()


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
    app.include_router(cleanup.router)
    app.include_router(modes.router)
    app.include_router(playlists.router)
    app.include_router(sfx.router)
    app.include_router(devices.router)
    app.include_router(diagnostics.router)
    app.include_router(admin.router)
    app.include_router(sync.router)
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
