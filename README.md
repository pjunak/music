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
  tracks, can audition each suggested song through the normal canonical playback controls without
  changing the selection, then use the existing preview and create-only Authoring import
  transaction. Signal measurements remain numeric evidence and never become semantic mood tags
  automatically.
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
- **Review-first EQ Assistant** — connect a separately chosen structured-text model and describe
  the sound you want. The task can return only ten fixed graphic-EQ bands with gains from -12 to
  +12 dB in 0.5 dB steps; frequencies and the final preset document are constructed locally. A
  current synthetic EQ quality pass and an explicit per-request disclosure are required. Jobs are
  durable across browser refreshes, and results remain inert until the operator previews and
  explicitly commits the preset through the normal create-only Authoring import.
- **Optional AI connections** — save user-chosen OpenAI-compatible provider access in the
  dedicated Assistant tab, verify it from the server, and assign a model independently to each
  declared role. A connection owns one key: several roles can deliberately reuse it, or specialized
  roles can choose separate connections and keys even when they use the same provider. Every
  assignment must pass a fixed synthetic structured-output test before it can be enabled; changing
  its connection, model, timeout, or response limit invalidates that test.
  API keys are encrypted at rest and never returned to the browser. AI Setup shows only whether a
  key is saved plus a masked hint. A saved key can be deleted without deleting its connection or
  role drafts; deletion and replacement both invalidate verification and model-quality gates until
  the operator explicitly completes them again. The shared execution harness is bounded and
  provider-neutral, but it is not exposed as a general prompt API. Provider adapters declare
  supported transports such as structured text or future bounded audio input; verification records
  the capabilities actually confirmed, and tasks accept only compatible verified connections.
  Reserved future tasks remain visible as planned work but cannot be configured before their
  feature-specific input, quality, consent, and review contracts exist. The playlist planner,
  metadata music tagger, manual-tag cleanup reviewer, and EQ draft assistant are the implemented
  optional model tasks.
  Each requires its own current synthetic quality pass and versioned disclosure consent. Playlist
  planning sends at most 100 path-free candidates and returns a draft. Music tagging sends metadata
  in batches of at most 20, may choose only from the fixed D&D vocabulary, and stores suggestions
  under `model-evidence-tagger/v2` for explicit per-tag review. When current local signal analysis
  exists, tagging may also send its bounded energy, brightness, tension, tempo, and confidence
  values; audio files, waveforms, paths, and detailed measurements remain on the server. Neither
  path can write a playlist or
  manual tag directly. Tag cleanup sends only normalized manual tag names, their usage counts, and
  the fixed D&D starter vocabulary in one bounded request; its stored proposal remains inert until
  the user selects specific renames, and stale proposals are rejected. Quality, playlist, tagging,
  and cleanup jobs retain their attempted request count,
  provider-reported model IDs, and reported input/output token totals; the UI identifies calls where
  the provider omitted usage rather than treating missing counts as exact zero. Usage is checkpointed
  after each provider attempt, so a failed or cancelled job still shows what was already reported.
  It does not estimate charges because provider pricing is not part of the portable model contract.
  The Library Analysis screen restores tagging and cleanup progress after refresh or reopen and
  shows model output beside local tools without merging their ownership. The EQ workflow sends only
  the operator's goal and fixed band limits; specialized audio-model workflows remain locked until
  a concrete bounded audio transport receives its own reviewed contract.
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
  without leaving duplicates. A conservative local cleanup preview finds only unambiguous spelling
  or plural matches to the D&amp;D starter vocabulary; it changes nothing until individual suggestions
  are selected, rejects stale previews, and applies the chosen renames in one transaction. Generated
  tags expose their analyzer, confidence, and evidence for per-tag review; accepting copies one into
  manual tags, while rejection remains a separate durable decision, removes that label from current
  playlist evidence, and never mutates authored data.
  Review-state filters and explicitly selected bulk decisions make larger libraries manageable;
  stale or invalid suggestions are reported individually instead of blocking valid selections.
  An optional quality-certified metadata tagging model can populate the same review surface through
  a durable server job. It never receives paths, audio, existing tags, or review decisions, skips
  unchanged model profiles, and cannot promote its output without an explicit acceptance.
  A separately assigned, quality-certified cleanup model can review only the manual tag catalog and
  usage counts. It proposes renames in a durable server job, selects nothing by default, and can
  apply only the individually checked, still-current proposal items in one atomic transaction.
- **Automatic playlists** — switch a normal playlist to a versioned local tag/BPM rule after
  previewing its exact resolved songs. Rules can match any or every included tag, exclude tags,
  choose accepted/manual tags alone or add current local metadata analysis, bound BPM and list
  length, and use deterministic ordering. The result materializes into the same ordered playlist
  rows used by playback and refreshes when opened or played after relevant library evidence changes.
  Individual item edits stay locked while automatic mode is active; switching back to manual keeps
  the current resolved songs. Provider/model suggestions are never automatic rule evidence.
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
strict time and response-size limits and does not send songs, tags, prompts, or audio. The UI reports
saved-key presence separately from verification. Removing a saved key keeps the connection and its
role choices, but prevents use until a new key is saved, the connection is verified, and the exact
model configuration passes its tests again.

Before relying on a backup, verify that its database and deployment key still match. This command
is read-only: it prints a short non-secret key ID and counts, but never prints a provider key.

```powershell
music-cli assistant-credentials check
```

To validate a restored copy without touching production, point `DATABASE_URL` and
`ASSISTANT_CREDENTIAL_KEY` at the isolated restore before running the same check. Treat a non-zero
`unreadable credentials` count as an incomplete or mismatched backup.

Master-key rotation is an offline, all-or-nothing operation. Generate a second key, expose it only
to the rotation process as `ASSISTANT_CREDENTIAL_KEY_NEW`, and run the default dry run first:

```powershell
$env:ASSISTANT_CREDENTIAL_KEY_NEW = "<new URL-safe base64 32-byte key>"
music-cli assistant-credentials rotate
```

After stopping every Music server process that uses the database, apply the rotation and then
replace `ASSISTANT_CREDENTIAL_KEY` in the deployment secret store before restarting:

```powershell
music-cli assistant-credentials rotate --apply --server-stopped
```

Rotation decrypts every saved credential before changing any row, re-encrypts them in one database
transaction, and resets provider verification, conformance, and model-quality gates. Connection and
role choices remain, but must be verified and checked again. Never keep the old and new keys in the
same long-lived environment file.

Connection types and model tasks are linked through versioned capabilities rather than provider or
model-name guesses. The initial OpenAI-compatible adapter verifies `structured-text/v1`. Future
audio-capable adapters must implement and verify their own bounded `audio-input/v1` transport before
the specialized audio-analysis role can be configured.

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
