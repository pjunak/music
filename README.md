# Music

A self-hosted **music player and tabletop-session orchestrator**. One person (the GM/DM)
drives playback from any device; audio comes out of one or more output devices — a laptop,
a TV in the room, a headless box wired to a speaker. The server is the single source of
truth for *what should be playing right now*; every client reconciles to it.

Built as one safe-Rust modular monolith that serves both the JSON API and the React SPA from one
origin. SQLite owns durable state, the filesystem owns media, and YAML owns campaign content.

> Single-operator app, designed to run on a home server behind a reverse proxy. It is **not**
> multi-tenant — there is one account, the operator's.

## Features

- **Filesystem-driven library** — your folder tree under `MUSIC_DIR` *is* the library. Drag in
  a file or a whole album folder; native Rust metadata adapters read its tags, with a
  filename/parent-folder fallback. SQLite keeps a durable materialized index, and a visible,
  retryable reconciliation job refreshes it without blocking startup.
- **Review-first library cleanup** — open **Assistant → Library cleanup** (or use the scoped
  Library toolbar shortcut) to repair filenames, folder names, and embedded metadata through one
  proposal and per-change review. Enabled Sources can identify a recording through a conservative
  MusicBrainz metadata match, optionally fall back to a local Chromaprint/AcoustID fingerprint, and
  retrieve Last.fm community tags only after identity is established. Canonical metadata repairs and
  controlled-vocabulary mood-tag suggestions start unticked; ambiguous matches propose nothing.
  Every applied filename, folder, and embedded-tag run keeps a server journal in **History &
  rollback**, where it can be downloaded, conflict-safely reverted, or restored later from the JSON
  file. Accepted community tags remain database-only operator tags and use the Mood Library's review
  boundary rather than the file rollback journal.
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
  database mood tags, indexed metadata, and any available current local analysis. The local,
  explainable planner remains the default and makes no external calls. An explicitly selected,
  quality-certified provider model can optionally rerank a locally filtered pool after a versioned
  disclosure and confirmation. Both methods produce the same read-only draft: you choose the final
  tracks, can audition each suggested song through the normal canonical playback controls without
  changing the selection, then use the existing preview and create-only Authoring import
  transaction. The same builder is available as an optional sidecar in Authoring, where the
  created playlist opens immediately for ordinary manual reordering and editing. Signal
  measurements remain numeric evidence and never become semantic mood tags automatically.
- **Playlist quality evaluation** — run versioned, synthetic D&D playlist scenarios through the
  provider-neutral suggestion contract. The harness measures relevance, required selection,
  ordering, explanations, determinism, and invented or excluded tracks, with explicit thresholds
  that fail regressions. A configured playlist model can be evaluated either through an explicit
  CLI disclosure flag or an explicit durable job in model settings. The server stores progress and the
  exact model-configuration fingerprint, so refreshes can restore the run and changed settings
  invalidate its result. Local filtering reduces each case to at most 100 candidates, paths are
  removed, and the model may return only known track IDs. A current pass is required before that
  exact model configuration can be selected for a live-library suggestion.
  Bundled suites live under
  [`crates/music-application/src/assistant/evaluation_suites/`](crates/music-application/src/assistant/evaluation_suites/);
  the practical evaluation sequence is in [`ASSISTANT.md`](ASSISTANT.md).
- **Review-first EQ Assistant** — connect a separately chosen structured-text model and describe
  the sound you want. A deterministic intent map first creates a conservative ten-band baseline
  and per-band safety envelope; the model may refine only inside that envelope in 0.5 dB steps.
  Frequencies and the final preset document are constructed locally. A
  current synthetic EQ quality pass and an explicit per-request disclosure are required. Jobs are
  durable across browser refreshes, and results remain inert until the operator previews and
  explicitly commits the preset through the normal create-only Authoring import. Authoring can
  open this workflow as a sidecar and then switches directly to the normal effect-rack editor for
  listening and manual band adjustment.
- **Optional AI connections** — save user-chosen OpenAI-compatible provider access in the
  dedicated Assistant tab, verify it from the server, and assign a model independently to each
  declared role. A connection owns one key: several roles can deliberately reuse it, or specialized
  roles can choose separate connections and keys even when they use the same provider. Every
  assignment must pass a fixed synthetic structured-output test before it can be enabled; changing
  its connection, model, timeout, or response limit invalidates that test.
  API keys are encrypted at rest and never returned to the browser. Model settings show only whether a
  key is saved plus a masked hint. A saved key can be deleted without deleting its connection or
  role drafts; deletion and replacement both invalidate verification and model-quality gates until
  the operator explicitly completes them again. The shared execution harness is bounded and
  provider-neutral. Every task generates its provider schema, prompt contract, example, and local
  validator from one strict output model. The standard adapter requests broadly compatible
  JSON-object output; the separate strict JSON Schema adapter uses native schema constraints when
  the chosen provider supports them. A harness or task contract upgrade makes existing
  model tests and quality reports stale so they must be rerun explicitly. The harness is not
  exposed as a general prompt API. Provider adapters declare
  supported transports such as structured text or future bounded audio input; verification records
  the capabilities actually confirmed, and tasks accept only compatible verified connections.
  Reserved future tasks remain visible as planned work but cannot be configured before their
  feature-specific input, quality, consent, and review contracts exist. The playlist planner,
  context-aware mood tagger, mood-tag cleanup reviewer, and EQ draft assistant are the implemented
  optional model tasks.
  Each requires its own current synthetic quality pass and versioned disclosure consent. Playlist
  planning sends at most 100 path-free candidates, constrains the response to those exact track
  IDs, and returns a draft.
  Playlist refinement keeps local defaults and supplements its bounded candidate pool through
  current vocabulary aliases/cues matched to your tags. Use `music-cli evaluate-playlists SUITE
  --engine candidates --json` to inspect synthetic candidate recall without a provider call;
  this diagnostic does not certify model quality.
  Mood tagging sends bounded artist, album, origin, and genre metadata in
  batches of at most 20, may choose only stable IDs from the revisioned operator vocabulary, and
  stores suggestions under `model-context-tagger/v6` for explicit per-tag review.
  The pre-run plan shows the normal batch estimate and a maximum that includes up to two
  fresh contract-recovery requests; invalid output is never silently repaired.
  **Assistant → Mood library → Mood vocabulary** exposes every canonical name, definition, group,
  exact alias, and bounded semantic context cue for manual editing. The provider receives those
  global cues to interpret soundtrack phrases, but no per-track local tag hypothesis. When current
  comprehensive local context exists, tagging may also send bounded
  whole-track trajectories, tempo development, major acoustic sections and transitions,
  repetition, analyzer confidence, and optional local voice/instrumental classifier evidence (or an
  explicit unknown/unavailable status). It does not send locally
  generated tag hypotheses or ask the provider to recreate energy/brightness/tension axes. Audio
  files, waveforms, full timelines, spectrograms, track titles, display titles, file and folder
  names, library paths, database mood tags, and detailed measurements remain on the server. Neither
  workflow can write a playlist or
  manual tag directly. Tag cleanup resolves declared aliases and unambiguous spelling/plural cases
  locally first, then sends only unresolved source IDs/names and usage counts plus canonical ID
  definitions in batches of at most 20. The model returns exactly one canonical-ID-or-null decision
  per source. Local-only cleanup makes no provider call, and every stored
  suggestion identifies whether it came from a local rule or the model. The built-in groups separate
  physical setting, period feel, scene, and emotional mood; period feel describes the era the track
  evokes rather than its release date. The proposal remains inert
  until
  the user selects specific renames, and stale proposals are rejected. Quality, playlist, tagging,
  and cleanup jobs retain their attempted request count,
  provider-reported model IDs, and reported input/output token totals; the UI identifies calls where
  the provider omitted usage rather than treating missing counts as exact zero. Usage is checkpointed
  after each provider attempt, so a failed or cancelled job still shows what was already reported.
  It does not estimate charges because provider pricing is not part of the portable model contract.
  The Library Analysis screen restores tagging progress after refresh or reopen and shows model
  output beside local tools without merging their ownership. Vocabulary and cleanup share a separate
  workspace, while model settings group their routing rows but preserve independent model choices.
  The EQ workflow sends only the operator's goal and fixed band limits; specialized audio-model
  workflows remain locked until a concrete bounded audio transport receives its own reviewed contract.
  See [`ASSISTANT.md`](ASSISTANT.md) for practical deployment and acceptance,
  [`docs/ASSISTANT_ARCHITECTURE.md`](docs/ASSISTANT_ARCHITECTURE.md) for the current source,
  contract, privacy, test, and change map, and
  [`docs/assistant-ux-philosophy.md`](docs/assistant-ux-philosophy.md) for the shared drafting,
  review, creation, and manual-tuning interaction contract.
- **Durable library context analysis** — one restartable `local-context/v2` job runs two sequential
  library passes. The first decodes every new or changed track into factual whole-track context;
  only after those rows are checkpointed does a capacity-one, job-scoped model-owning thread perform
  optional voice detection. The thread and compiled graph are dropped when the voice pass completes or
  is cancelled, so the several-gigabyte inference working set is not retained while the service is idle.
  The factual context includes signal-level and intensity development, rhythmic
  drive, perceptual brightness, spectral fullness, spectral change, local tempo behavior, major acoustic sections,
  repetition, technical details, explicit analysis-stage status, and bounded performance timings.
  Native RustFFT frame/spectrum math uses gain-invariant Mel spectral profiles, logarithmic brightness,
  dB-based onset strength, octave-aware tempo confidence, and multiscale change detection. It stores condensed timelines
  in a separate context table, checkpoints each completed/failed track and each voice result, and
  never proposes semantic tags. A failed voice pass leaves the first-pass rows usable and resumable
  instead of repeating their audio analysis. The Track Context tab mirrors the library folders,
  supports playback, and shows the full
  stored context for a selected song. The production image includes FFmpeg for the indexed MP3,
  FLAC, OGG/Opus, M4A/AAC, WAV, and WMA formats plus Chromaprint's `fpcalc` for optional AcoustID
  fallback; development without FFmpeg has a PCM-WAV analysis fallback.
  An opt-in, checksum-pinned MusiCNN model running through tract can add a normalized voice score
  and vocal-window coverage. It is never downloaded or enabled automatically because the model has
  separate non-commercial/share-alike license obligations; see
  [`ASSISTANT.md`](ASSISTANT.md#optional-local-voice-analysis) and
  [`ADR-009`](docs/ADR-009-opt-in-local-voice-analysis.md).
- **Database mood tags** — attach operator-owned setting, period, scene, and mood context such as
  `medieval`, `tavern`, `dancing`, `combat`, or any custom term without modifying the audio file or
  its embedded album, artist, year, genre, and similar metadata. Database mood tags and generated
  suggestions remain visibly and
  structurally separate, while local playlist ranking gives explicit manual matches priority.
  Multi-select actions apply tags across a batch, and usage-aware rename can merge overlapping tags
  without leaving duplicates. A conservative local cleanup preview applies operator-declared aliases
  and finds only unambiguous spelling or plural matches to the controlled vocabulary; it changes
  nothing until individual suggestions are selected, rejects stale previews, and applies the chosen
  renames in one transaction. Generated
  tags expose their analyzer, confidence, and evidence for per-tag review; accepting copies one into
  manual tags, while rejection remains a separate durable decision, removes that label from current
  playlist evidence, and never mutates authored data.
  Review-state filters and explicitly selected bulk decisions make larger libraries manageable;
  stale or invalid suggestions are reported individually instead of blocking valid selections.
  The Mood Library's expandable review summary counts pending, accepted, and rejected current
  suggestions by source across all matching tracks and pages, regardless of the review-state
  filter. These describe operator decisions, not model accuracy or lifetime history.
  An optional quality-certified context-aware tagging model can populate the same review surface through
  a durable server job for the whole library, the current folder (recursive or direct children), or
  explicitly selected tracks. Its Library dialog previews counts, provider calls, and full/partial/
  missing context coverage; the operator may run with metadata-only fallback or skip tracks without
  full current context. It restores durable progress, lets the operator audition each song, and
  preselects only high/medium-confidence suggestions for explicit acceptance. It never receives
  track titles, display titles, library paths, audio, existing database mood tags, or review decisions; it skips unchanged
  model profiles and cannot promote its output without acceptance.
  A separately assigned, quality-certified cleanup model can review only the database mood-tag catalog and
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

The canonical `Dockerfile` builds the React SPA and Rust server/CLI, then copies only those
binaries, the SPA, CA certificates, FFmpeg, Chromaprint tools, and mode seeds into a non-root Debian runtime. It
contains no Python runtime. Application data lives
under `/data`; the optional Assistant credential master key uses a separate secrets mount so it is
not mixed into the database/media backup. The runtime user can write only the data mount; application
binaries, SPA assets, and seed modes remain root-owned and read-only to that user.

```bash
# Build the release image.
docker build -t music-rust .

# Prepare persistent data for the image's non-root UID.
sudo install -d -m 0700 -o 1000 -g 1000 /srv/music-data

# Prepare the optional external-service secrets directory for the same UID. Skip
# this and its mount if no model-provider or catalog API keys will be saved in Music.
sudo install -d -m 0700 -o 1000 -g 1000 /srv/music-secrets

# Run it — application data and optional secrets stay separate.
docker run -d --name music \
  -p 8000:8000 \
  -v /srv/music-data:/data \
  -v /srv/music-secrets:/run/music-secrets \
  music-rust

# Create the operator account (password prompts interactively)
docker exec -it music music-cli create-user admin
```

Then open `http://localhost:8000` and sign in. A fresh install boots fine with **zero** audio
files — drop music in through the Library tab (or straight into `/srv/music-data/music`).

Pull requests run the Rust, frontend, architecture, security, and non-root container gates without
publishing. A successful `main` workflow reuses those checks, compiles and smoke-tests the release
image once, publishes the warm cached result to GHCR, and dispatches its immutable digest to the
separate infrastructure repository for rollout.

## Configuration

Set via environment variables (see [`.env.example`](.env.example)). The Rust image pre-sets the
storage paths under `/data`, so a containerized run typically needs
no environment variables at all. (There is no `SECRET_KEY`: sessions are opaque random
DB-backed tokens, nothing is signed.)

| Variable | Required | Default (in image) | Purpose |
|---|---|---|---|
| `MUSIC_DIR` | | `/data/music` | Root of the scanned music library |
| `SFX_LIBRARY_DIR` | | `/data/sfx` | Root for soundboard SFX files |
| `MODES_DIR` | | `/data/modes` | On-disk mode bundles (seeded on first boot) |
| `DEVICES_FILE` | | `/data/devices.json` | Legacy one-time device-import source; SQLite is authoritative afterward |
| `DATABASE_URL` | | `sqlite:////data/app.db` | App DB (auth, playlists, indexed tracks) |
| `STATIC_DIR` | | `/app/static` | Built SPA served at `/` |
| `ALLOWED_ORIGINS` | | `http://localhost:5173` | Comma-separated CORS origins (only needed for split dev) |
| `SESSION_COOKIE_SECURE` | | `true` | Send the session cookie over HTTPS only. Set `false` only for a plain-HTTP (no-TLS) deployment |
| `SESSION_COOKIE_DOMAIN` | | — | Cookie domain override for multi-host deploys |
| `ASSISTANT_CREDENTIAL_KEY` | Only for optional external-service setup | — | URL-safe base64 32-byte key used to encrypt model-provider and catalog API keys in `app.db` |
| `ASSISTANT_CREDENTIAL_KEY_FILE` | Only for optional external-service setup | `/run/music-secrets/assistant-credential.key` | Fixed master-key file; authenticated settings may create it once when its private parent mount exists |
| `ASSISTANT_CREDENTIAL_HOST_DIRECTORY_HINT` | | — | Optional non-secret host path shown in model settings' copyable mount/setup guide |
| `ASSISTANT_VOICE_MODEL_PATH` | Only for opt-in local voice analysis | — | Read-only path to the exact checksum-pinned Essentia voice/instrumental model |
| `ASSISTANT_LIBRARY_CONTEXT_WORKERS` | | `1` | Bounded first-pass analysis workers (`1`–`4`); voice inference remains capacity one |
| `CLEANUP_ACOUSTID_API_KEY` | Optional deployment fallback | — | AcoustID application client key; a key saved under Library cleanup → Sources takes precedence without a restart |
| `CLEANUP_LASTFM_API_KEY` | Optional deployment fallback | — | Last.fm API key; a key saved under Library cleanup → Sources takes precedence without a restart |
| `CLEANUP_FPCALC_PATH` | | `fpcalc` | Chromaprint executable used locally by the AcoustID fallback |
| `MAX_UPLOAD_FILES` / `MAX_UPLOAD_FILE_BYTES` | | `500` / `1 GiB` | Per-request upload guard rails |
| `LOG_LEVEL` | | `info` | Log verbosity |

### Optional encrypted API-key storage

The local Assistant does not need a model provider or a credential key. For the standard Docker
image, mount the private directory shown in Quick start, sign in, open either
**Assistant → AI setup** or **Assistant → Library cleanup → Sources**, and
select **Initialize secure storage**. Music creates the fixed
`/run/music-secrets/assistant-credential.key` file with a new random key. The key value is never
sent to the browser, stored in `app.db`, or mixed into `/data`.

The API cannot choose a path, overwrite an existing file, replace a key, or initialize a new key
when saved encrypted credentials already exist. A password-confirmed complete reset is the only
browser operation that may remove the fixed file: it first erases every encrypted model-provider
and catalog credential, resets model gates, and clears catalog evidence caches in one database
transaction, then removes that exact file.
`ASSISTANT_CREDENTIAL_KEY_FILE` is a non-secret deployment setting; its parent directory must
already exist, be private, and be writable by the container's UID 1000.

Managed deployments may instead generate a key externally:

```powershell
[Convert]::ToBase64String(
  [Security.Cryptography.RandomNumberGenerator]::GetBytes(32)
).Replace('+', '-').Replace('/', '_')
```

Set the printed value as `ASSISTANT_CREDENTIAL_KEY` in the server environment and restart the
server. This environment value takes precedence over the configured file. Keep either form in the
deployment's secret store, not in source control. A database backup and this key must be restored
together; without the original key, saved external-service credentials cannot be decrypted and must be
entered again.

For file-backed storage, model settings can delete every saved API key and the master key through
**Reset encrypted key storage** after a destructive-action warning and current-password confirmation.
Connection and role drafts remain, active model or catalog-enrichment jobs block the reset, and a failed final file
removal is reported separately after the credentials are already safely erased. Environment-backed
keys still belong to the service configuration and cannot be removed by a running process. To
preserve saved credentials, use the offline rotation workflow below instead of resetting or editing
the file.

Provider adapters verify compatible services by requesting their model list. The generic adapters
cover OpenAI-shaped services; explicit transport-free handlers own documented provider differences
without bypassing the shared HTTP safety boundary. The Google Gemini handler pins the Google AI
Studio address, canonicalizes model resource IDs, and translates thinking controls. Public
addresses require HTTPS. Private-network providers are opt-in per connection. Verification uses
strict time and response-size limits and does not send songs, tags, prompts, or audio. The UI reports
saved-key presence separately from verification. A saved provider key cannot be replaced in place:
delete it explicitly before entering another one. Removing it keeps the connection and its role
choices, but prevents use until a new key is saved, the connection is verified, and the exact model
configuration passes its tests again.

Before relying on a backup, verify that its database and deployment key still match. This command
is read-only: it prints a short non-secret key ID and counts, but never prints an API key.

```powershell
music-cli assistant-credentials check
```

To validate a restored copy without touching production, point `DATABASE_URL` at the isolated
database and provide its matching key through `ASSISTANT_CREDENTIAL_KEY` or an isolated
`ASSISTANT_CREDENTIAL_KEY_FILE` before running the same check. Treat a non-zero `unreadable
credentials` count as an incomplete or mismatched backup.

Master-key rotation is an offline, all-or-nothing operation. Generate a second key, expose it only
to the rotation process as `ASSISTANT_CREDENTIAL_KEY_NEW`, and run the default dry run first:

```powershell
$env:ASSISTANT_CREDENTIAL_KEY_NEW = "<new URL-safe base64 32-byte key>"
music-cli assistant-credentials rotate
```

After stopping every Music server process that uses the database, apply the rotation and then
replace the configured environment key or key-file contents with the new key before restarting:

```powershell
music-cli assistant-credentials rotate --apply --server-stopped
```

Rotation decrypts every saved model-provider and catalog credential before changing any row,
re-encrypts them in one database
transaction, and resets provider verification, conformance, and model-quality gates. Connection and
role choices remain, but must be verified and checked again. Never keep the old and new keys in the
same long-lived environment file.

Connection types and model tasks are linked through versioned capabilities rather than provider or
model-name guesses. OpenAI-compatible and Google Gemini adapters verify `structured-text/v1`; the
explicit strict-schema variants additionally verify `strict-json-schema/v1`. Future
audio-capable adapters must implement and verify their own bounded `audio-input/v1` transport before
the specialized audio-analysis role can be configured.

The complete first-run sequence—local baseline, connection verification, per-role conformance and
quality checks, live-data acceptance, and isolated backup restore—is in
[`ASSISTANT.md`](ASSISTANT.md).

Rust owns an ordered SQLx migration ledger and refuses unknown or incompatible database shapes
before opening them for writes. `music-cli db doctor` is read-only; `music-cli db migrate` creates
and verifies a pre-migration backup before applying compatible migrations. The track index remains
regenerable from the filesystem, while authored and human-owned state is preserved. A legacy
Python database's standard Alembic bookkeeping table and obsolete `known_devices` table are accepted
only in their exact historical shapes and removed by explicit SQLx migrations after that backup
succeeds. The separately preserved `devices.json` remains the authoritative one-time device import.
Equivalent historical uniqueness constraints are preserved when they duplicate the same canonical
unique index; no uniqueness invariant is relaxed during migration.

## Development

The Rust server and Node 26+ frontend run as two processes in development.

```bash
# Server — Rust 1.97.1 is pinned by rust-toolchain.toml
cp .env.example .env                                 # dev defaults work as-is
cargo run --locked -p music-server --bin music-cli -- create-user admin
cargo run --locked -p music-server --bin music-server # http://localhost:8000

# Frontend (separate terminal)
cd frontend
npm ci
npm run dev                                          # http://localhost:5173, proxies to the API
```

Checks:

```bash
# Rust
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo nextest run --workspace --all-features
cargo test --workspace --all-features --doc
cargo run --locked -p music-server --bin music-cli -- contracts check --root .
cargo deny check
cargo audit --deny warnings

# Frontend
cd frontend
npm run typecheck
npm run lint
npm run test                                         # vitest
npm run build
```

## Architecture

The maintained documentation index is [`docs/README.md`](docs/README.md). In particular, use the
[Assistant architecture and contract map](docs/ASSISTANT_ARCHITECTURE.md) before changing model
inputs, outputs, disclosures, fingerprints, provider transport, or review behavior.

### Repository lifecycle

- `main` is the only active development and production branch. It contains the complete Rust
  server, CLI, analysis pipeline, and headless output appliance together with the retained React
  frontend and compatibility client.
- `legacy/python` is a frozen copy of the final Python implementation at `b93f91d`; the matching
  `legacy-python-2026-08-29` tag is the durable rollback source. Do not backport features or publish
  normal releases from it.
- `archive/*` tags retain superseded, unmerged snapshots discovered during branch cleanup. They are
  historical recovery points, not supported release lines.

```
crates/music-domain/       pure playback, paths, playlists, and cleanup rules
crates/music-application/  use-case contracts, actors/coordinators, jobs, Assistant logic
crates/music-storage/      SQLite repositories, migrations, backup, crypto, recovery journals
crates/music-media/        rooted filesystem capabilities, metadata, YAML, streaming adapters
crates/music-analysis/     bounded RustFFT/EBU-R128 context and optional tract voice inference
crates/music-protocol/     typed HTTP/WebSocket edge contracts and generated TypeScript DTOs
crates/music-server/       Axum routes, composition root, supervision, CLI, provider transport
crates/music-output/       native headless output appliance with supervised mpv subprocesses
frontend/                  React + TypeScript; Web Audio reconciles canonical PlayerState
modes/                     on-disk campaign bundles copied as an empty-target seed
clients/                   guest output protocol and appliance installation material
```

- **The library never moves files implicitly.** Moves, renames, and deletes happen only as
  explicit API actions, and the index follows.
- **Single process, one origin.** `/api/*` and the SPA share a host; the SPA falls back to
  client-side routing. Mutable subsystems have explicit owners rather than process-global state.
- **Unexpected failures are correlation-safe.** Clients receive stable codes and a correlation ID;
  internal error details remain in structured server logs.

## Security model

- Authoring and every filesystem/database mutation require the operator session.
- Playback state, track streams, cover art, metadata, and registered output clients are
  intentionally guest-readable so room displays and speaker appliances work without credentials.
- Guest WebSockets may register and follow state. Their only mutation exception is an optional
  position report while that same client is an active output.
- Sessions are opaque, random, revocable database tokens stored in an HTTP-only cookie. There is
  no signing secret because no client-side session data is trusted.

## Tech stack

Rust 1.97.1 · Tokio · Axum · SQLx/SQLite · Serde · Lofty · RustFFT · tract · argon2 · AES-GCM —
React · TypeScript 7 · Vite · Zustand · Oxlint · Web Audio API. The release is packaged as a
multi-stage image (`node:26.7.0-alpine` + `rust:1.97.1-trixie` builders → non-root
`debian:trixie-slim` runtime).
