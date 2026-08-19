# Music

A self-hosted **music player and tabletop-session orchestrator**. One person (the GM/DM)
drives playback from any device; audio comes out of one or more output devices — a laptop,
a TV in the room, a headless box wired to a speaker. The server is the single source of
truth for *what should be playing right now*; every client reconciles to it.

Built as a single FastAPI process that serves both the JSON API and the React SPA from one
origin. SQLite for state, the filesystem for the music library, YAML for campaign content.

> Single-operator app, designed to run on a home server behind a reverse proxy. It is **not**
> multi-tenant — there is one account, the operator's.

## Features

- **Filesystem-driven library** — your folder tree under `MUSIC_DIR` *is* the library. Drag in
  a file or a whole album folder; tags are read via [mutagen], with a filename/parent-folder
  fallback. The index is a materialised view of the tree, rebuilt on boot and on upload.
- **Server-as-reducer playback** — the server holds the canonical `PlayerState`, owns the
  playback clock (`position_ms` is live in every push), and advances the queue itself at end of
  track; clients follow state and seek only when `position_epoch` changes. Repeat, shuffle,
  crossfade, and a graphic-EQ effect chain are all there.
- **Multi-device output** — any connected browser tab (or headless client) can be switched on as
  a live speaker, each with its own canonical software volume. A device can be saved as "output by default" so
  it auto-activates when it reconnects.
- **TV / room display** — a read-only now-playing view at `/` for a screen in the room, usable
  **without logging in** (guest access), with cover art, up-next, and recently-played.
- **Compatibility mode** — a dependency-free ES5 fallback player (`compat-mode.js`) for browsers
  that can't run the SPA (old smart TVs). Loads via `<script nomodule>` or a boot watchdog when
  the bundle fails to run; previewable on any browser with `?compat`. Same output protocol,
  plain `<audio>` + XHR/WebSocket.
- **Modes** — top-level campaign bundles (theme + soundboards + cues + EQ presets), authored as
  on-disk YAML and hot-reloadable.
- **Authoring import** — preview and selectively bring in playlists, soundboards, interrupts, EQ
  presets, and cues from another mode, a JSON file, or pasted JSON. The versioned JSON contract is
  suitable for assistant-generated drafts. Imports are create-only: invalid references and existing
  names or IDs are reported for review rather than overwritten. See
  [`clients/authoring-import-v1.md`](clients/authoring-import-v1.md).
- **Review-first playlist Assistant** — describe a mood or scene and get song suggestions from your
  manual tags, current metadata profiles, and any available measured audio signals. The local,
  explainable planner remains the default and makes no external calls. An explicitly selected,
  quality-certified provider model can optionally rerank a locally filtered pool after a versioned
  disclosure and confirmation. Both methods produce the same read-only draft: you choose the final
  tracks, then use the existing preview and create-only Authoring import transaction. Signal
  measurements remain numeric evidence and never become semantic mood tags automatically.
- **Playlist quality evaluation** — run versioned, synthetic D&D playlist scenarios through the
  provider-neutral suggestion contract. The harness measures relevance, required selection,
  ordering, explanations, determinism, and invented or excluded tracks, with explicit thresholds
  that fail regressions. A configured playlist model can be evaluated either through an explicit
  CLI disclosure flag or an explicit durable job in AI Setup. The server stores progress and the
  exact model-configuration fingerprint, so refreshes can restore the run and changed settings
  invalidate its result. Local filtering reduces each case to at most 100 candidates, paths are
  removed, and the model may return only known track IDs. A current pass is required before that
  exact model configuration can be selected for a live-library suggestion.
  See [`backend/evaluation/README.md`](backend/evaluation/README.md).
- **Optional AI connections** — save user-chosen OpenAI-compatible provider access in the
  dedicated Assistant tab, verify it from the server, and assign a model independently to each
  declared role. Every assignment must pass a fixed synthetic structured-output test before it can
  be enabled; changing its connection, model, timeout, or response limit invalidates that test.
  API keys are encrypted at rest and never returned to the browser. The shared execution harness
  is bounded and provider-neutral, but it is not exposed as a general prompt API. The playlist
  planner and metadata music tagger are the first optional live-library integrations.
  Each requires its own current synthetic quality pass and versioned disclosure consent. Playlist
  planning sends at most 100 path-free candidates and returns a draft. Music tagging sends metadata
  in batches of at most 20, may choose only from the fixed D&D vocabulary, and stores suggestions
  under `model-metadata-tagger/v1` for explicit per-tag review. Neither path can write a playlist or
  manual tag directly. Quality, playlist, and tagging jobs retain their attempted request count,
  provider-reported model IDs, and reported input/output token totals; the UI identifies calls where
  the provider omitted usage rather than treating missing counts as exact zero. Usage is checkpointed
  after each provider attempt, so a failed or cancelled job still shows what was already reported.
  It does not estimate charges because provider pricing is not part of the portable model contract.
  The Library Analysis screen restores tagging progress after refresh or reopen and shows model output
  beside local suggestions without merging their ownership. Cleanup, EQ, audio, and other workflows
  remain local until they receive their own reviewed contracts.
- **Durable library analysis** — build versioned per-track mood profiles in a server-side job that
  stores progress, survives page refreshes, resumes safely after restart, skips unchanged tracks,
  and keeps outputs from different analyzers side by side. `local-metadata/v1` produces reviewable
  suggestions from existing metadata. `local-audio/v1` separately decodes files on the server and
  stores measured level, dynamics, high-frequency, transient, and stable-tempo evidence. Signal
  failures are checkpointed per track and retried later; signal measurements never become manual
  tags or semantic mood claims automatically. The production image includes FFmpeg for the indexed
  MP3, FLAC, OGG/Opus, M4A/AAC, WAV, and WMA formats; development without FFmpeg has a PCM-WAV
  fallback.
- **Manual playlist tags** — attach operator-owned context such as `medieval`, `tavern`, `dancing`,
  or any custom tag without modifying the audio file. Manual and generated tags remain visibly and
  structurally separate, while local playlist ranking gives explicit manual matches priority.
  Multi-select actions apply tags across a batch, and usage-aware rename can merge overlapping tags
  without leaving duplicates. Generated tags expose their analyzer, confidence, and evidence for
  per-tag review; accepting copies one into manual tags, while rejection remains a separate durable
  decision, removes that label from current playlist evidence, and never mutates authored data.
  Review-state filters and explicitly selected bulk decisions make larger libraries manageable;
  stale or invalid suggestions are reported individually instead of blocking valid selections.
  An optional quality-certified metadata tagging model can populate the same review surface through
  a durable server job. It never receives paths, audio, existing tags, or review decisions, skips
  unchanged model profiles, and cannot promote its output without an explicit acceptance.
- **Live EQ tuning** — enable Live tuning in an existing preset to auto-activate it and
  hear throttled, auto-saved rack/EQ changes on every active browser output while music plays.
- **Soundboards** — fire-and-forget SFX, with keyboard hotkeys, broadcast to every active output.
- **Cues** — one-click saved setups: apply an EQ preset, start a playlist from a chosen
  song + timestamp, fire one-shot SFX, and start looping SFX, all from one button.
- **Interrupts** — briefly take over playback for an alert/stinger, either pausing the music or
  *ducking* it under the interrupt with configurable fade in/out.
- **External outputs** — anything that can play an HTTP stream can be an audio output, with no
  server changes and no login. See [`clients/`](clients/README.md) for the protocol and a
  ready-to-run headless appliance.

## Quick start (Docker)

The repo ships a multi-stage `Dockerfile` (Node build of the SPA → Python runtime serving both
the API and the static bundle on port 8000). All state lives under `/data`, so a single bind
mount persists everything.

```bash
# Build the image
docker build -t music .

# Run it — one bind mount for music/, sfx/, modes/, and app.db
docker run -d --name music \
  -p 8000:8000 \
  -v /srv/music-data:/data \
  music

# Create the operator account (password prompts interactively)
docker exec -it music music-cli create-user admin
```

Then open `http://localhost:8000` and sign in. A fresh install boots fine with **zero** audio
files — drop music in through the Library tab (or straight into `/srv/music-data/music`).

The image is also built and pushed to GHCR by CI on every push to `main`; the production
rollout itself is handled by a separate infra repository.

## Configuration

Set via environment variables (see [`backend/.env.example`](backend/.env.example)). The
Dockerfile pre-sets the storage paths under `/data`, so a containerised run typically needs
no environment variables at all. (There is no `SECRET_KEY`: sessions are opaque random
DB-backed tokens, nothing is signed.)

| Variable | Required | Default (in image) | Purpose |
|---|---|---|---|
| `MUSIC_DIR` | | `/data/music` | Root of the scanned music library |
| `SFX_LIBRARY_DIR` | | `/data/sfx` | Root for soundboard SFX files |
| `MODES_DIR` | | `/data/modes` | On-disk mode bundles (seeded on first boot) |
| `DEVICES_FILE` | | `/data/devices.json` | Remembered output-device registry |
| `DATABASE_URL` | | `sqlite:////data/app.db` | App DB (auth, playlists, indexed tracks) |
| `STATIC_DIR` | | `/app/static` | Built SPA served at `/` |
| `ALLOWED_ORIGINS` | | `http://localhost:5173` | Comma-separated CORS origins (only needed for split dev) |
| `SESSION_COOKIE_SECURE` | | `true` | Send the session cookie over HTTPS only. Set `false` only for a plain-HTTP (no-TLS) deployment |
| `SESSION_COOKIE_DOMAIN` | | — | Cookie domain override for multi-host deploys |
| `ASSISTANT_CREDENTIAL_KEY` | Only for optional AI setup | — | URL-safe base64 32-byte key used to encrypt provider API keys in `app.db` |
| `MAX_UPLOAD_FILES` / `MAX_UPLOAD_FILE_BYTES` | | `500` / `1 GiB` | Per-request upload guard rails |
| `LOG_LEVEL` | | `info` | Log verbosity |

### Optional AI connection storage

The local Assistant does not need a model provider or a credential key. To enable encrypted
credential storage in the separate **Assistant → AI Setup** screen, generate one deployment key:

```powershell
python -c "import base64,secrets; print(base64.urlsafe_b64encode(secrets.token_bytes(32)).decode())"
```

Set the printed value as `ASSISTANT_CREDENTIAL_KEY` in the server environment and restart the
server. Keep it in the deployment's secret store, not in source control. A database backup and
this key must be restored together; without the original key, saved provider credentials cannot
be decrypted and must be entered again.

The first adapter verifies OpenAI-compatible providers by requesting their model list. Public
addresses require HTTPS. Private-network providers are opt-in per connection. Verification uses
strict time and response-size limits and does not send songs, tags, prompts, or audio.

There is no general migration framework: the schema is created idempotently on boot, and
compatible additive columns are applied automatically. Renames, drops, and type changes require
a deliberate migration or a documented database reset. The track index remains regenerable from
the filesystem.

## Development

Backend (Python 3.11+) and frontend (Node 26+) run as two processes in development.

```bash
# Backend — uv-managed (uv.lock is the pinned resolution)
cd backend
uv sync --locked --extra dev                         # creates .venv from uv.lock
cp .env.example .env                                 # dev defaults work as-is
uv run music-cli create-user admin
uv run uvicorn app.main:app --reload                 # http://localhost:8000

# Frontend (separate terminal)
cd frontend
npm ci
npm run dev                                          # http://localhost:5173, proxies to the API
```

No `uv`? `python -m venv .venv && source .venv/bin/activate && pip install -e ".[dev]"` works
too — you just won't get the locked versions.

Checks:

```bash
# Backend
cd backend
uv run pytest                                        # tests
uv run ruff check app tests                          # lint
uv run mypy app tests                                # types

# Frontend
cd frontend
npm run typecheck
npm run lint
npm run test                                         # vitest
npm run build
```

## Architecture

```
backend/   FastAPI app. The sync package (app/sync/) is the authority: state-mutating
           actions funnel through commit_and_broadcast → a state machine → DB persistence +
           a WebSocket broadcast. HTTP handlers that mutate state route through the same funnel.
           Two auth tiers: most endpoints require a session; the player/stream/cover endpoints
           accept guests so a logged-out TV tab can act as an output.
frontend/  React + TypeScript (Vite). A Web Audio engine (ambient crossfade + interrupt lane +
           a preset effect chain) reconciles to PlayerState pushed over the WebSocket.
modes/     On-disk campaign bundles, baked into the image as a seed and copied to MODES_DIR on
           first boot. Everything authored is per-mode — playlists, soundboards, cues, EQ presets.
clients/   The documented guest output protocol + a reference headless appliance.
```

- **The library never moves files implicitly.** Moves, renames, and deletes happen only as
  explicit API actions, and the index follows.
- **Single process, one origin.** `/api/*` and the SPA share a host; the SPA falls back to
  client-side routing.
- **Unhandled exceptions return JSON** with the error class + message — a single-user debug aid.

## Security model

- Authoring and every filesystem/database mutation require the operator session.
- Playback state, track streams, cover art, metadata, and registered output clients are
  intentionally guest-readable so room displays and speaker appliances work without credentials.
- Guest WebSockets may register and follow state. Their only mutation exception is an optional
  position report while that same client is an active output.
- Sessions are opaque, random, revocable database tokens stored in an HTTP-only cookie. There is
  no signing secret because no client-side session data is trusted.

## Tech stack

FastAPI · SQLAlchemy 2.0 · Pydantic · argon2 · mutagen — React · TypeScript 7 · Vite · Zustand · Oxlint ·
Web Audio API. Packaged as a multi-stage Docker image (`node:26-alpine` build → `python:3.12-slim`
runtime).

[mutagen]: https://mutagen.readthedocs.io/
