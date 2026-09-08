# Engineering contracts

Source paths in code spans are relative to the repository root.
Read only the section for the subsystem being changed. These are product
constraints shared by the owning crates and browser client. Assistant-specific
contracts live in [the Assistant map](ASSISTANT_ARCHITECTURE.md). Verification
is selected using [the validation matrix](VALIDATION.md).

## State and synchronization

- The `PlaybackHandle` actor in `music-application` is authoritative. Every state mutation is
  reduced, persisted, and only then published; HTTP and WebSocket actions use the same owner.
- `music-protocol` is the wire contract. Update Rust schemas, frontend types and
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

- Authoring APIs resolve a `CurrentSession`; player/output read surfaces use the optional-session
  lookup where documented.
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
- Store only domain-separated session hashes and independent random management
  IDs. Never authenticate a hash or management ID or reintroduce plaintext-token
  fallback. Schema 11 deliberately revokes legacy sessions after a verified backup.
  The legacy `token_prefix` response field aliases the complete management ID;
  revocation requires that exact ID. Preserve configured cookies and logout scope.
- Check browser WebSocket Origin before every upgrade, including unavailable
  playback. Accept the Host origin using the public scheme declared by
  `SESSION_COOKIE_SECURE`, or an explicit `ALLOWED_ORIGINS` entry. Reject malformed
  or multiple Origin values; absent Origin remains valid for native clients.
  Do not infer a trusted proxy from forwarded headers.

## Library and filesystem safety

- `MUSIC_DIR` is the library. The `tracks` table is a materialized filesystem
  index keyed by normalized relative path.
- Every index write and app-owned filesystem mutation goes through the bounded
  `LibraryCoordinator`. Its recovery journal and catalog transaction must close the disk/database
  crash window before a new catalog generation is published.
- All paths under music or SFX roots pass through the existing normalization
  and containment helpers. Never use string-prefix or `..` substring checks.
- A missing or empty media directory is valid and must return coherent empty
  states.
- Moves, renames, deletes, uploads, and metadata edits happen only through
  explicit user actions. Conflict handling remains ask-first with `rename`,
  `overwrite`, and `skip` race-safe on the server.
- Tag-backed metadata round-trips through typed `TagPatch` values and the format-specific
  Lofty/FFmpeg adapters. Database-only fields stay independent. Preserve per-track
  partial-failure results for bulk operations.
- Library cleanup is propose -> review -> journal -> execute. Detection must
  remain pure and must never mutate files while merely scanning.
- SFX paths are rooted under `SFX_LIBRARY_DIR`; serving remains gated by loaded
  soundboard references.

## Devices and volume

- Activation, output-by-default designation, and volume are separate:
  - `active_output_device_ids` is live session state.
  - SQLite stores operator-curated remembered devices and default-on; `devices.json` is only a
    preserved one-time migration input.
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
- Preset effect types must stay aligned across Rust validation, editor UI,
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
- Feature styles follow `frontend/src/styles/README.md`. Preserve the single eager
  `global.css` import order; lazy view imports must not change the shared cascade.
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
  and declare restartability explicitly. CPU and blocking work goes through bounded
  analysis/media adapters rather than running directly on Tokio workers; a restartable handler
  must be idempotent or checkpointed.
- Graceful shutdown follows the same restartability policy as crash recovery. Never requeue a
  non-restartable provider job after it may have incurred cost; retain its latest safe checkpoint
  and mark it interrupted instead.
- Job ordering breaks equal creation timestamps by insertion order, not random UUID.
  Keep `jobs timing` diagnostics read-only and bounded; never load job payloads or
  treat missing, unfinished, or restarted-job timing as zero. Follow
  [`docs/JOB_DIAGNOSTICS.md`](JOB_DIAGNOSTICS.md) before changing lane fairness.
- SQLx owns an ordered migration ledger. The schema doctor accepts only documented legacy/additive
  shapes; renames, drops, type changes, and future versions require a deliberate migration and a
  verified pre-migration backup.
- Runtime persistence is `app.db`, media directories, mode data, and the separately held Assistant
  key. A legacy `devices.json` may be imported but is never a second authority. Seed modes copy
  only when the target is empty.
- The reusable verification workflow validates Rust, the frontend, architecture boundaries,
  dependencies, the final Rust-only tree, and the non-root release image. The `main` workflow calls
  it before publishing an image or dispatching infrastructure deployment.
- Private-library context/voice runs, long resource soaks, and physical-speaker checks are useful
  post-cutover operational checks for this personal deployment, not merge blockers. Keep failures
  visible and fix them normally; never weaken model or protocol contracts merely to make a check pass.
- `rewrite-tree.mjs final` protects the completed language boundary. Do not weaken its allowlist to
  make a stale runtime, workflow, generated artifact, or transition tool pass.
- This repository does not SSH to production. Deployment rollout, reverse
  proxy, bind mounts, and production `.env` live in `junak.eu`.
