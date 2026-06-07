# syntax=docker/dockerfile:1.7
#
# Multi-stage build for the music app (FastAPI backend + React/Vite SPA).
#   Stage 1 — frontend-builder: build the React/Vite frontend → dist/
#   Stage 2 — backend-builder:  compile any cffi/native wheels into /wheels
#   Stage 3 — runtime:          slim image, no compilers, just installs
#                               from the wheelhouse + copies static assets
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
# Stage 2: backend wheel builder
# ============================================================================
# Has the C toolchain + libffi-dev needed to compile argon2-cffi (and any
# future cffi-backed wheel that doesn't ship a manylinux build for slim).
# Output is just the wheels; the runtime stage stays compiler-free.
FROM python:3.11-slim AS backend-builder

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        libffi-dev \
        && rm -rf /var/lib/apt/lists/*

WORKDIR /build

COPY backend/pyproject.toml ./
COPY backend/app ./app

# Build wheels for everything declared in pyproject.toml (transitively)
# plus the project itself, into /wheels. The runtime stage installs from
# this directory only — no network, no compiler.
RUN pip install --no-cache-dir --upgrade pip wheel && \
    pip wheel --no-cache-dir --wheel-dir /wheels .

# ============================================================================
# Stage 3: python runtime
# ============================================================================
FROM python:3.11-slim AS runtime

# Only runtime deps. ca-certificates for outbound TLS; that's it.
# (build-essential + libffi-dev are in the builder stage above; they
# don't ship in this image.)
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Install from the pre-built wheelhouse — no compiler, no network access
# needed. The wheelhouse contains every transitive dep + the music-backend
# wheel itself.
COPY --from=backend-builder /wheels /wheels
RUN pip install --no-cache-dir --upgrade pip && \
    pip install --no-cache-dir --no-index --find-links=/wheels music-backend && \
    rm -rf /wheels

# Modes ship as a read-only seed at /seeds/modes (EQ presets ride along inside
# each mode). On boot the backend copies it into MODES_DIR only when that
# directory is empty, so user edits made in a bind-mounted volume survive
# image rebuilds.
COPY modes /seeds/modes

# Built frontend SPA from stage 1.
COPY --from=frontend-builder /frontend/dist /app/static

# Non-root runtime user. UID 1000 lines up with the typical first-user UID
# on the host so bind-mounted dirs can be chown'd to match.
#
# `/data` is the recommended mount point: bind-mount it from the host to
# persist music/, sfx/, modes/, and app.db across image rebuilds.
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
    MODES_SEED_DIR=/seeds/modes \
    DEVICES_FILE=/data/devices.json \
    DATABASE_URL=sqlite:////data/app.db

EXPOSE 8000

# Schema is created (idempotently) by the FastAPI lifespan — no migrations
# step. exec'ing uvicorn directly keeps signal handling clean (graceful
# shutdown on SIGTERM from `docker stop`).
CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000", "--log-level", "info"]
