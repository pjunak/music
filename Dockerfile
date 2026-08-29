# syntax=docker/dockerfile:1.7
#
# Canonical release image: React SPA + Rust server/CLI in one non-root
# container. Runtime state is kept under /data; the image itself is immutable.

# ============================================================================
# Stage 1: browser application
# ============================================================================
FROM node:26.7.0-alpine AS frontend-builder

WORKDIR /frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci

COPY frontend/ ./
ENV VITE_API_BASE_URL=""
RUN npm run build

# ============================================================================
# Stage 2: Rust application
# ============================================================================
FROM rust:1.97.1-trixie AS rust-builder

WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/build/target,sharing=locked \
    cargo build --locked --release -p music-server --bins && \
    mkdir -p /out && \
    cp target/release/music-server target/release/music-cli /out/

# ============================================================================
# Stage 3: minimal non-root runtime
# ============================================================================
FROM debian:trixie-slim AS runtime

# FFmpeg's libstdc++ dependency includes unused GDB Python pretty-printers.
# GDB is absent from the runtime image, so discard that debugger-only payload.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        ffmpeg \
        && rm -rf /var/lib/apt/lists/* /usr/share/gcc/python /usr/share/gdb && \
    groupadd --system music && \
    useradd --system --gid music --uid 1000 --home-dir /app --shell /usr/sbin/nologin music

WORKDIR /app

COPY --from=rust-builder /out/music-server /usr/local/bin/music-server
COPY --from=rust-builder /out/music-cli /usr/local/bin/music-cli
COPY --from=frontend-builder /frontend/dist /app/static
COPY modes /seeds/modes
COPY docs/THIRD_PARTY_NOTICES.md /usr/share/doc/music/THIRD_PARTY_NOTICES.md

RUN mkdir -p /data && \
    chown music:music /data && \
    chmod 0750 /data

USER music

ENV MUSIC_DIR=/data/music \
    SFX_LIBRARY_DIR=/data/sfx \
    MODES_DIR=/data/modes \
    MODES_SEED_DIR=/seeds/modes \
    DEVICES_FILE=/data/devices.json \
    DATABASE_URL=sqlite:////data/app.db \
    STATIC_DIR=/app/static \
    ASSISTANT_CREDENTIAL_KEY_FILE=/run/music-secrets/assistant-credential.key

EXPOSE 8000

HEALTHCHECK --interval=30s --timeout=3s --start-period=15s --retries=3 \
  CMD ["music-cli", "healthcheck", "--address", "127.0.0.1:8000", "--timeout-ms", "2000"]

CMD ["music-server"]
