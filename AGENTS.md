# Music

Self-hosted FastAPI + React music player and tabletop-session orchestrator.
The server owns canonical playback state; browser and headless clients register
as controllers and optional audio outputs.

## Read first

- [`README.md`](README.md) for setup, product behavior, and deployment.
- [`clients/README.md`](clients/README.md) before changing the external output
  protocol; the reference appliance is under `clients/headless/`.
- [`backend/.env.example`](backend/.env.example) for storage/config defaults.
- Protocol schemas and state transitions in `backend/app/sync/` before changing
  WebSocket messages or playback semantics.

## Commands

Backend, from `backend/`:

```powershell
uv sync --locked --extra dev
uv run ruff check app tests
uv run mypy app tests
uv run pytest
```

An existing Windows virtual environment may be used instead:

```powershell
.\.venv\Scripts\ruff.exe check app tests
.\.venv\Scripts\mypy.exe app tests
.\.venv\Scripts\pytest.exe
```

Frontend, from `frontend/` (Node 26+):

```powershell
npm ci
npm run lint
npm run typecheck
npm run test
npm run build
```

The frontend uses the native TypeScript 7 compiler and Oxlint. The local
`local/stable-store-selector` rule is an Oxlint JS plugin under
`frontend/lint-rules/`; keep its real-binary fixture test when changing it.

Run locally:

```powershell
# backend/
uv run uvicorn app.main:app --reload

# frontend/
npm run dev
```

Keep `backend/uv.lock` synchronized with `pyproject.toml`; CI and the container
build use the locked graph.

## Architecture

```text
backend/app/
  assistant/     provider-independent suggestion contracts and local engines
  api/           HTTP routes and dependencies
  core/          settings, database, security
  devices/       file-backed remembered-device registry
  domain/        playlist and persisted playback helpers
  jobs/          durable background-job registry, runner, and lifecycle
  library/       filesystem index, metadata, cleanup
  models/        SQLAlchemy models
  modes/         mode bundle loader
  presets/       effect validation and crossfade resolution
  sync/          protocol, state machine, WS router, broadcasts, loops
frontend/src/
  core/          API/WS clients, stores, audio engine, shared pure helpers
  shell/         routing, auth gates, app shell, footer
  views/         Console, Library, Authoring, Assistant, Settings, Diagnostics
  components/    reusable controls, dialogs, editors, primitives
clients/         documented guest output protocol and headless reference client
modes/           seed mode bundles and per-mode authored resources
```

The production image builds the frontend and serves it from the FastAPI app.
Runtime data lives outside the image.

## State and synchronization

- `backend/app/sync/` is authoritative. Every state mutation goes through
  `commit_and_broadcast` and the state machine; HTTP mutations use the same
  funnel as WebSocket actions.
- `protocol.py` is the wire contract. Update backend schemas, frontend types and
  guards, compatibility behavior, external-client docs, and tests together.
- Clients reconcile server snapshots. Do not create an independent frontend
  truth or optimistically invent lasting playback state.
- Registration uses a stable `client_id`. Reconnects register again and receive
  current state; never replay stale mutations automatically.
- The frontend validates every WS frame before stores/listeners consume it.
  Preserve machine-readable error codes for session loss and protocol errors.
- Boot pruning removes dangling tracks/modes/presets, clears live output IDs,
  and stops persisted loops whose timers cannot survive restart.

## Authentication

- Authoring APIs use `CurrentUser`; player/output read surfaces use
  `OptionalUser` where documented.
- Guest sockets may register and act as read-only outputs. Mutating actions
  require a valid session; active-output position reports follow the documented
  exception.
- Re-check long-lived WS sessions so expiry or revocation downgrades an open
  connection. Keep API 401s and WS session-loss errors on the same re-login
  path.
- The shell renders immediately. Protected route content owns its loading and
  login state; do not reintroduce redirect loops or a full-shell auth block.
- Sessions are opaque, random, database-backed tokens. Do not add a signing
  secret or hard-code the cookie name.

## Library and filesystem safety

- `MUSIC_DIR` is the library. The `tracks` table is a materialized filesystem
  index keyed by normalized relative path.
- Every index write serializes on `library_index.write_lock`. A disk move and
  its index update must hold the same lock across both operations.
- All paths under music or SFX roots pass through the existing normalization
  and containment helpers. Never use string-prefix or `..` substring checks.
- A missing or empty media directory is valid and must return coherent empty
  states.
- Moves, renames, deletes, uploads, and metadata edits happen only through
  explicit user actions. Conflict handling remains ask-first with `rename`,
  `overwrite`, and `skip` race-safe on the server.
- Tag-backed metadata round-trips through the declarative `TAG_REGISTRY`.
  Database-only fields stay independent. Preserve per-track partial-failure
  results for bulk operations.
- Library cleanup is propose -> review -> journal -> execute. Detection must
  remain pure and must never mutate files while merely scanning.
- SFX paths are rooted under `SFX_LIBRARY_DIR`; serving remains gated by loaded
  soundboard references.

## Devices and volume

- Activation, output-by-default designation, and volume are separate:
  - `active_output_device_ids` is live session state.
  - `devices.json` stores operator-curated remembered devices and default-on.
  - `device_volumes` stores canonical absolute software levels by `client_id`.
- Any connected device may be activated. Designation only auto-activates a
  device when it connects and must not gate manual activation.
- Per-device volume is the current protocol. The deprecated master `volume`
  remains a compatibility projection for legacy clients; do not restore a
  master-volume UI or let presets override device volume.
- Output clients apply their own level to all audio, including SFX. The server
  validates position reports against active membership.
- Disconnect cleanup must not silence another tab using the same stable client
  ID. Stale IDs are tolerated only as documented by the state machine.

## Modes, authoring, and effects

- Playlists, soundboards, cues, and EQ presets belong to exactly one mode.
- Assistant suggestions are read-only drafts until the operator explicitly previews and commits
  them through Authoring import. Keep local heuristics and future model providers behind the same
  suggestion contracts; never let a ranking engine write playlists or mutate the library directly.
- Track analysis profiles are keyed by `(track_id, analyzer_id)`. Preserve source signatures,
  evidence, confidence, and analyzer versioning so metadata, signal, and optional model outputs can
  coexist. Suggestion engines may consume only current profiles and must fall back safely when a
  profile is absent, stale, or malformed.
- Authoring import is source adapter -> preview -> explicit selection -> atomic commit. Mode and
  versioned JSON sources share the same planner and transaction. It is create-only: conflicts are
  skipped, playlist tracks are re-resolved by canonical library-relative path, and a selected cue
  or interrupt cannot commit unless its source-side dependencies are also selected (or already
  exist in the target). Keep the v1 contract in `clients/authoring-import-v1.md` backward compatible.
- Authored IDs are derived with `uniqueSlug`; do not add manual ID fields.
- Preset effect types must stay aligned across backend validation, editor UI,
  frontend types, and the playback-engine switch.
- Effect-aware outputs cache manifests by active mode/id and must invalidate
  them when `PlayerState.preset_revision` changes. The guest-readable preset
  list is an output surface; keep mutations authenticated.
- Graphic EQ band definitions and response math live in `frontend/src/core/eq.ts`
  and are shared by the engine and editor visualization.
- Presets may override crossfade; they do not override output volume.
- Server-side loops own and cancel their timers. Cleanup must be idempotent on
  stop, mode changes, disconnect, and shutdown.

## Frontend rules

- Zustand selectors must not create fresh arrays/objects inside the selector.
  Return the raw reference and default outside, or use `usePlayerArray`.
  `local/stable-store-selector` enforces this.
- Use `toast`, `confirmDialog`, and `inputDialog`; do not use browser
  `alert`, `confirm`, or `prompt`.
- Use existing components, SVG icons, design tokens, and semantic accent rules.
  Do not introduce decorative danger/warning/success colors.
- Keep global keyboard shortcuts out of interactive controls and synchronized
  with the shortcut sheet. Mutating shortcuts remain unavailable to guests.
- The old-TV compatibility client is a supported guest output. Preserve the
  bundle-execution watchdog, `nomodule` path, idempotent takeover guard,
  stable client ID, and polling fallback. Do not let it clobber a booting SPA.
- User-visible asynchronous work needs loading, empty, failure, retry, and
  partial-success feedback as applicable.

## Persistence and deployment

- Long-running server work uses the durable background-job runner. Enqueue the
  database row before waking the worker, report cooperative progress/cancellation,
  and declare restartability explicitly. Job handlers run outside the event loop
  and must be idempotent or checkpointed before they may be restartable.
- SQLite schema creation is idempotent. `_apply_additive_columns()` handles
  only additive compatible columns; renames, drops, and type changes require a
  deliberate migration or documented reset.
- Runtime persistence is `app.db`, `devices.json`, media directories, and mode
  data under `/data`. Seed modes copy only when the target is empty.
- The GitHub workflow runs backend lint/type/tests and frontend
  lint/type/tests before building, publishing to GHCR, and dispatching the
  sibling infrastructure repository.
- This repository does not SSH to production. Deployment rollout, reverse
  proxy, bind mounts, and production `.env` live in `junak.eu`.

## Completion

- Run the affected narrow tests while iterating.
- Before handoff, run all relevant backend and frontend gates listed above.
- Add regression tests for protocol, persistence, path, auth, synchronization,
  or audio-state bugs.
- Update `README.md`, `clients/README.md`, `.env.example`, and this file when
  their contracts change.

Do not commit media, databases, device registries, secrets, generated builds,
or local tool configuration. The global Codex instructions govern task
commits. Never push, deploy, or release unless explicitly requested.
