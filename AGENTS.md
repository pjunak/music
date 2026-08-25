# Music

Self-hosted FastAPI + React music player and tabletop-session orchestrator.
The server owns canonical playback state; browser and headless clients register
as controllers and optional audio outputs.

## Read first

- [`README.md`](README.md) for setup, product behavior, and deployment.
- [`clients/README.md`](clients/README.md) before changing the external output
  protocol; the reference appliance is under `clients/headless/`.
- [`backend/.env.example`](backend/.env.example) for storage/config defaults.
- [`docs/README.md`](docs/README.md) for the maintained documentation index and
  [`docs/ASSISTANT_ARCHITECTURE.md`](docs/ASSISTANT_ARCHITECTURE.md) before changing model tasks,
  provider boundaries, disclosures, fingerprints, or review flows.
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
- Playlist recommendation changes must run the versioned synthetic suites under
  `backend/app/assistant/evaluation_suites/` through the provider-neutral evaluator. Add
  representative cases and explicit thresholds without copying private library data or freezing
  one incidental exact ranking. Future model providers must pass the same unknown-track,
  source-integrity, exclusion, selection-plan, and candidate-limit checks before UI integration.
- Optional model providers use encrypted connection records and per-task role mappings. Each
  connection owns exactly one credential; roles reference connections so tasks may deliberately
  reuse one credential or choose separate connections, including separate keys for the same
  provider. Never store a credential directly on a role.
  Credential presence is an explicit server-derived state; never infer it from a masked hint.
  Removing or replacing a connection credential keeps role drafts but resets verification,
  conformance, and quality results, so enabled roles remain ineffective until the new credential is
  saved and every gate passes again.
  Adapters declare transport capabilities, successful verification persists the capabilities
  actually confirmed, and roles declare their required capabilities. Enforce compatibility again
  during role save, testing, enablement, and execution; never infer it from provider or model names.
  Roles without a complete feature, quality, consent, and review contract stay explicitly
  unavailable for configuration even if their future role ID is already reserved.
  Never return or log a provider key, never infer provider capabilities from a saved URL, and never
  enable a role until the operator explicitly verifies its connection and its exact runtime
  configuration passes the fixed synthetic conformance challenge. Provider I/O must stay off the
  event loop, bounded by request size, time, and response size, and protected against redirects and
  unsafe destinations. Keep provider-specific model-ID normalization and inference parameters in
  explicit versioned adapter handlers; handlers shape requests but must not bypass the shared
  pinned-DNS transport or infer behavior from connection names, URLs, or model names.
  OpenAI-compatible structured requests must carry the generated task JSON
  Schema. The standard adapter uses JSON-object response mode; the explicit strict adapter may use
  `json_schema` only when selected and proven by conformance. Each fixed feature prompt includes a
  locally validated example of its strict output shape. Include the versioned harness, conformance,
  and per-role feature contracts in the runtime fingerprint so a transport or task-contract change
  makes existing model tests and quality results stale instead of silently reusing them. Feature
  code resolves usable roles through `prepare_role_execution()` and
  owns a fixed prompt plus strict result schema; do not expose a browser-facing general prompt
  endpoint. Saving, verifying, or testing a role does not authorize sending library data or
  replacing a local engine. Preserve the Assistant credential master key separately from database
  backups. `ASSISTANT_CREDENTIAL_KEY` takes precedence over the fixed
  `ASSISTANT_CREDENTIAL_KEY_FILE`; the authenticated API may exclusively create only that configured
  file and must never accept a path, return the key, overwrite an existing file, or generate a new
  key while saved provider credentials exist. Saved provider credentials are write-once and must be
  explicitly deleted before another key can be added. The password-confirmed browser reset may
  remove only the configured file-backed key after atomically erasing all saved credentials and
  resetting every provider/model gate; preserve connection and role drafts, refuse active provider
  jobs, and report post-commit file-removal failure as a partial result. Environment-key removal and
  credential-preserving rotation remain explicit console/offline maintenance workflows.
- Credential recovery checks and master-key rotation are offline operator workflows. Keep audit
  output secret-free and identify keys only by a short one-way fingerprint. Rotation must decrypt
  every saved credential before mutating any row, re-encrypt all credentials in one transaction,
  and reset provider verification, role conformance, and model-quality gates. Require an explicit
  server-stopped acknowledgement before applying it; a dry run is the default.
- The optional model playlist planner may run only through the dedicated consent-bound durable job.
  Keep `local-planner/v2` as the default, require the exact current `playlist-quality-v1` pass and
  disclosure version before enqueueing, and make model jobs non-restartable to avoid silently
  repeating provider cost. Locally enforce eligibility and exclusions, send a privacy-reduced pool
  of at most 100 candidates, and preserve the original local rank while unioning additional recall
  candidates found through controlled-vocabulary aliases and context cues. Treat a non-empty
  display title as canonical, and do not infer mood axes from artist names or filesystem paths.
  Choose the review default with bounded duration-error improvement. Inject the exact candidate
  IDs into the output schema, and accept only
  ranked/selected IDs. Never send library-relative paths
  or trust model-supplied source fields, tags, scores, reasons, or evidence; reconstruct the public
  response from the local candidate snapshot. Model results remain drafts and must use the existing
  Authoring import preview/select/commit path. Configured-model CLI evaluation separately requires
  the explicit `--send-suite-to-provider` disclosure flag.
- The optional EQ assistant may run only through `assistant.model-eq-draft`. Build a deterministic
  intent baseline and per-band refinement envelope before the provider call. Require the exact
  current `eq-quality-v1` pass and disclosure consent, make jobs non-restartable, and send only the
  operator's sound goal plus the fixed ten-band frequencies, local guidance, and gain limits.
  Accept exactly ten gains in the local envelope and in 0.5 dB steps; construct every frequency
  and Authoring field locally. Deterministically bound overlong rationale and caution text because
  it is incidental review prose; never repair or coerce gains, frequency order, schema identity,
  missing fields, or unexpected fields.
  The result is a review-only draft and may create a preset only through the existing Authoring
  import preview/select/commit transaction. Never send songs, audio, library metadata, paths,
  playlists, existing presets, or credentials to the EQ role.
- Optional mood tagging may run only through `assistant.model-music-tagging`. Require the
  exact current `music-tagging-quality-v1` pass and disclosure consent, batch at most 20 tracks
  per provider request, and keep jobs non-restartable. Resolve whole-library, folder
  (recursive/direct), or explicit-track scope locally. Provider input is limited to indexed
  descriptive metadata, canonical library-relative paths treated as untrusted data, duration,
  BPM, numeric track IDs, the full revisioned operator vocabulary's IDs/names/groups/definitions/
  exact aliases and bounded semantic context cues, and an optional bounded projection of current
  `local-context/v1` evidence:
  loudness, intensity, rhythmic-drive, brightness, density and spectral-change trajectories;
  tempo development; major acoustic sections/transitions; repetition; confidence; and optional
  local voice/instrumental classifier score and coverage (or explicit unknown/unavailable status).
  Never send the absolute media root, paths outside the indexed library,
  audio, waveforms, spectrograms, full-resolution timelines, database mood tags, stored
  suggestions, playlists, review history, or credentials. Local context analysis must remain
  factual and may never propose setting, period, scene, mood, genre, or instrument tags.
  Context cues are global operator-managed vocabulary guidance, not per-track local tag
  hypotheses; the model must confirm them against the complete untrusted metadata phrase.
  Keep each tag's ID, name, definition, aliases, and cues together in the provider input so the
  model never has to join a compact index to a second definition table. A run may spend at most
  two disclosed correction requests on malformed JSON, schema-invalid output, track-set mismatch,
  or unsupported tag IDs. Each correction is a fresh strict classification; never edit, coerce,
  or locally repair the rejected output, and never retry provider, network, timeout, or truncation
  failures through this budget.
  Period feel is separate from physical setting and describes the era evoked by the complete
  evidence, not release date or recording technology. It is a zero-or-one categorical group;
  `cross era` replaces rather than accompanies its component period tags. The model must choose
  zero through eight exact IDs from the full controlled vocabulary and
  return confidence plus at most four bounded evidence strings. Do not ask it for signal axes and
  do not generate a local tag-ID hypothesis before the call. Reject unknown/duplicate IDs,
  missing track IDs, malformed confidence, extra fields, and truncated output; only incidental
  evidence text may be bounded. Store output under `model-context-tagger/v6` in
  `track_analyses` and bind its source signature to metadata, current context signature (or its
  absence), vocabulary fingerprint, contract version, and role fingerprint.
  Before a live run, report full, partial, missing/stale, and failed context coverage. Let the
  operator either include incomplete tracks using metadata/path alone or skip every track without
  full current context. The model may never write `track_user_tags`; accepted suggestions become
  database mood tags only through explicit single or bulk review.
- Optional model-assisted manual-tag cleanup may run only through
  `assistant.model-tag-cleanup`. Run declared-alias and deterministic spelling/plural cleanup first and make no
  provider call when it resolves every candidate. Require the exact current
  `tag-cleanup-quality-v1` pass and versioned disclosure consent, allow at most 500 catalog tags,
  make the provider job non-restartable, batch at most 50 unresolved names per call, and send only
  source IDs/names and usage counts plus canonical vocabulary IDs, names, groups, and definitions.
  Require one ordered canonical-ID-or-null decision per source. Bound overlong reason text locally,
  but never repair source order, source or target IDs, confidence, missing decisions, or unexpected
  fields. Never send song metadata,
  paths, audio, playlists, generated tags, review
  history, or credentials. Store only a review-only proposal bound to the exact role fingerprint
  catalog signature, and vocabulary fingerprint. Apply only explicitly selected source/target pairs from that stored job,
  reject stale or invented selections, and commit all selected manual-tag renames atomically.
- Task-specific model quality checks run as durable, non-restartable jobs and persist their current
  certification separately from job history. Bind every result to the exact model-role runtime
  fingerprint, clear it after connection reverification or runtime changes, and keep historical
  reports synthetic and secret-free. A quality pass does not authorize live-library access.
  Connection changes, credential deletion, and reverification must refuse to reset assigned roles
  while their model jobs are queued or running; the UI must warn that deliberate reverification
  clears their model tests and quality results.
  The mood-tagging suite batches 20 synthetic tracks per provider request, matching live work,
  while preserving
  per-scenario progress and diagnostics, then repeats every safety scenario once to catch unstable
  forbidden output. Provider/contract failures and forbidden false positives block certification;
  a scenario's safety label alone does not turn a required semantic-tag miss into a blocking error.
  All scenarios contribute to the suite's explicit minimum scored pass rate. A
  failed-scenario recheck may call the provider only for failures from the exact current complete
  result and may merge those results only for diagnosis. Only another complete suite may update
  certification.
- Durable quality, playlist, tagging, and tag-cleanup model jobs record the shared bounded provider-usage
  summary: attempted calls, provider-reported model IDs, and reported input/output token totals.
  Checkpoint it after every provider attempt so failures, cancellation, and graceful shutdown keep
  the usage already incurred. Preserve missing-usage counts explicitly; never infer unreported
  tokens or portable cost from provider-specific pricing.
- Generated tag profiles remain keyed by `(track_id, analyzer_id)` in `track_analyses`.
  Comprehensive factual audio context is keyed the same way in `track_contexts` and stores its
  summary, condensed timeline, major sections, technical facts, and stage status separately from
  semantic tag suggestions. Preserve source signatures, confidence, and analyzer versioning.
  Consumers may use only current, well-formed context/profiles and must fall back safely when data
  is absent, partial, stale, failed, or malformed.
- Optional local voice analysis may use only the checksum-pinned Essentia MusiCNN model through the
  explicit deployment setting. Keep it off by default, local-only, and non-fatal; include its
  model/runtime identity in context staleness, preserve unknown/unavailable states, and never label
  spectral heuristics as human-voice detection.
- Database mood tags are operator-owned rows in `track_user_tags`, independent from embedded file
  tags such as album, artist, year, and genre and independent from generated analysis. Never write
  these rows into media files. Update them with additive/removal deltas, display their source explicitly,
  and pass them separately to suggestion engines. An analyzer or provider must never overwrite or
  silently promote its output into manual tags. Bulk updates commit valid tracks together and
  report missing/limited tracks; library-wide rename merges duplicate target rows atomically.
  Generated-tag decisions live in `track_analysis_tag_reviews` and bind to the reviewed source
  signature. Acceptance atomically adds the manual tag; rejection and reopening never remove or
  rewrite manual tags, and a changed analysis signature returns the suggestion to pending review.
  Current-profile consumers omit rejected tag labels without deleting the analyzer's stored profile.
  Bulk review applies only explicitly selected suggestions, commits valid decisions together, and
  reports stale, missing, or tag-limited items individually. Never add a select-all implicit write.
  Tag cleanup detection is pure and conservative. Bind its preview to the current manual-tag
  catalog, require explicit per-suggestion selection, reject stale or invented selections, and apply
  all selected renames in one transaction without changing unselected tags.
- Automatic playlists are a mode on the normal `Playlist` model, not a second playlist type. Keep
  `automatic-playlist/v1` local and deterministic, require an exact read-only preview before saving
  a rule, and materialize matches into ordinary ordered playlist items so existing playback clients
  remain unchanged. Refresh stale rules before reads and playback. Only accepted/manual tags and,
  when explicitly selected, current `local-metadata/v1` moods may be rule evidence; provider/model
  suggestions must never become silent automatic inputs. Lock individual item edits while the rule
  is active, and preserve the materialized list when the operator switches back to manual. A
  malformed persisted rule must not break playlist listing or playback: expose its safe error state,
  keep the last materialized rows, and let the operator replace the rule or make the playlist manual.
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
- Graceful shutdown follows the same restartability policy as crash recovery. Never requeue a
  non-restartable provider job after it may have incurred cost; retain its latest safe checkpoint
  and mark it interrupted instead.
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
