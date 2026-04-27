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
#   ffmpeg                       — beets/mutagen audio probing
#   ca-certificates              — TLS for any outbound HTTPS calls
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        libffi-dev \
        ca-certificates \
        ffmpeg \
        && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Backend source + install. The pyproject.toml's setuptools.packages.find
# picks up the `app` package directly from cwd, so we install with `pip
# install .` after copying both the manifest and the source.
COPY backend/pyproject.toml ./
COPY backend/app ./app
COPY backend/migrations ./migrations
COPY backend/alembic.ini ./
RUN pip install --no-cache-dir --upgrade pip && \
    pip install --no-cache-dir .

# Modes + presets are read by the backend at startup. Bake them into the
# image at /modes and /presets (the .env points MODES_DIR / PRESETS_DIR here).
COPY modes /modes
COPY presets /presets

# Built frontend SPA from stage 1.
COPY --from=frontend-builder /frontend/dist /app/static

# Non-root runtime user. UID 1000 lines up with the typical first-user UID
# on the host so bind-mounted dirs (/opt/stacks/music/{data,incoming,library})
# can be chown'd to match.
RUN groupadd -r music && \
    useradd -r -g music -u 1000 -d /app -s /sbin/nologin music && \
    mkdir -p /app/data /app/incoming /app/library && \
    chown -R music:music /app

USER music

EXPOSE 8000

# Run alembic migrations on startup, then exec uvicorn so signals propagate
# correctly (graceful shutdown on SIGTERM from `docker stop`).
CMD ["sh", "-c", "alembic upgrade head && exec uvicorn app.main:app --host 0.0.0.0 --port 8000 --log-level info"]
