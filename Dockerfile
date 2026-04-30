# syntax=docker/dockerfile:1.7
#
# Multi-stage build for the music app (FastAPI backend + React/Vite SPA).
#   Stage 1 — node: build the React/Vite frontend → dist/
#   Stage 2 — python: install backend, copy dist/ as /app/static, run uvicorn
# The backend serves both /api/* (FastAPI routers) and / (the SPA via
# SpaStaticFiles) at the same origin, so VITE_API_BASE_URL is empty.

# ============================================================================
# Stage 1: frontend builder
# ============================================================================
FROM node:20-alpine AS frontend-builder

WORKDIR /frontend

# Cache the npm install layer separately so source changes don't reinstall.
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci

COPY frontend/ ./

# Same-origin in production: backend hosts the SPA at /, so the frontend
# uses relative URLs.
ENV VITE_API_BASE_URL=""

RUN npm run build
# Output: /frontend/dist

# ============================================================================
# Stage 2: python runtime
# ============================================================================
FROM python:3.11-slim AS runtime

# OS deps:
#   build-essential + libffi-dev — for any wheels that need to compile (argon2-cffi etc.)
#   ca-certificates              — TLS for any outbound HTTPS calls
# Note: ffmpeg used to be here for beets; mutagen reads tags from file
# headers directly so we don't need it. Add back if/when we ship the
# auto-format-conversion feature listed in docs/FUTURE.md.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        libffi-dev \
        ca-certificates \
        && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Backend source + install. The pyproject.toml's setuptools.packages.find
# picks up the `app` package directly from cwd, so we install with `pip
# install .` after copying both the manifest and the source.
COPY backend/pyproject.toml ./
COPY backend/app ./app
RUN pip install --no-cache-dir --upgrade pip && \
    pip install --no-cache-dir .

# Modes + presets ship as read-only seeds at /seeds/{modes,presets}. On boot
# the backend copies them into MODES_DIR / PRESETS_DIR only when those
# directories are empty, so user edits made in a bind-mounted volume survive
# image rebuilds.
COPY modes /seeds/modes
COPY presets /seeds/presets

# Built frontend SPA from stage 1.
COPY --from=frontend-builder /frontend/dist /app/static

# Non-root runtime user. UID 1000 lines up with the typical first-user UID
# on the host so bind-mounted dirs can be chown'd to match.
#
# `/data` is the recommended mount point: bind-mount it from the host to
# persist music/, sfx/, modes/, presets/, and app.db across image rebuilds.
# Subdirs are created by the app on first boot if missing — see lifespan.
RUN groupadd -r music && \
    useradd -r -g music -u 1000 -d /app -s /sbin/nologin music && \
    mkdir -p /app/data /app/incoming /app/library /data && \
    chown -R music:music /app /data /seeds

USER music

# Default storage paths under /data so a single bind-mount persists every
# stateful directory the app uses. Operators can override per-dir if they
# need separate volumes (e.g. NAS-backed music, fast-disk DB).
ENV MUSIC_DIR=/data/music \
    SFX_LIBRARY_DIR=/data/sfx \
    MODES_DIR=/data/modes \
    PRESETS_DIR=/data/presets \
    MODES_SEED_DIR=/seeds/modes \
    PRESETS_SEED_DIR=/seeds/presets \
    DATABASE_URL=sqlite:////data/app.db

EXPOSE 8000

# Schema is created (idempotently) by the FastAPI lifespan — no migrations
# step. exec'ing uvicorn directly keeps signal handling clean (graceful
# shutdown on SIGTERM from `docker stop`).
CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000", "--log-level", "info"]
