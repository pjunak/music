# Rust rewrite execution plan

**Status:** Implementation in progress

**Branch:** `rewrite/rust`
**Architecture:** [RUST_REWRITE_ARCHITECTURE.md](RUST_REWRITE_ARCHITECTURE.md)
**Reassessment:** [RUST_ARCHITECTURE_REASSESSMENT.md](RUST_ARCHITECTURE_REASSESSMENT.md)

This is the maintained plan for the complete rewrite. It is intentionally gate-driven: a phase is
not complete because its files exist or compile; it is complete only when its behavior, data, and
resource acceptance checks pass.

## Operating rules

1. Keep `main` deployable and feature-frozen. Develop only on `rewrite/rust` until cutover.
2. Keep the Python server and client runnable as reference oracles until the final removal phase.
3. Implement vertical behavior slices. Do not translate directories file-by-file.
4. Preserve existing data and public contracts by default; record intentional differences before
   changing the candidate implementation.
5. Do not move to the next phase with unexplained differential failures, failing gates, unbounded
   work, or an unresolved persistence/security boundary.
6. Prefer safe Rust and small explicit adapters. A compiler-clean design that relies on pervasive
   clones, global mutexes, blocking Tokio workers, or unbounded channels is not accepted.
7. Compose one explicit runtime. HTTP handlers translate and authorize; application use cases own
   orchestration; mutable resources have named coordinators rather than module globals.
8. Update this plan's phase status and evidence with each phase-closing commit.
9. Commit logical, validated scopes. Never push, merge to `main`, deploy, tag, or alter production
   data without explicit authorization.

## Branch and compatibility ledger

- Baseline commit: `b93f91d` on 2026-08-27.
- Rewrite branch: `rewrite/rust`, created directly from that clean baseline.
- The compatibility ledger below records every post-baseline Python fix or intentional contract
  difference, its Rust disposition, tests, and owner decision.
- If urgent Python fixes land on `main`, merge or cherry-pick their test/fixture evidence into the
  rewrite branch immediately; do not defer reconciliation to cutover week.
- The branch may contain both implementations for development, but production routing never mixes
  them.

### Compatibility ledger

| Source reference | Change or difference | Rust disposition | Evidence | Status |
|---|---|---|---|---|
| `b93f91d` | Rewrite baseline | Capture executable contracts in Phase 1 | `contracts/reference/v1` plus `music-protocol` corpus tests | In progress |
| `b93f91d` | `devices.json` is a mutable runtime store | Import to SQLite; add explicit CLI export/import; preserve source file for rollback | [Reassessment](RUST_ARCHITECTURE_REASSESSMENT.md#4-make-operational-data-ownership-consistent) | Accepted 2026-08-27 |
| `b93f91d` | Full library scan blocks startup | Serve the durable index, expose reconciliation state, and scan as a durable job | [Reassessment](RUST_ARCHITECTURE_REASSESSMENT.md#6-remove-full-scanning-from-the-critical-boot-path) | Accepted 2026-08-27 |
| `b93f91d` | Unexpected HTTP/WS errors expose exception text | Return safe code/message/correlation ID; keep internal detail in logs | [Reassessment](RUST_ARCHITECTURE_REASSESSMENT.md#10-separate-public-compatibility-from-internal-correctness) | Accepted 2026-08-27 |
| `b93f91d` | Voice isolation uses recycled Python processes | Try one model-owning Rust thread; require a Rust subprocess when the gate shows it is needed | [Reassessment](RUST_ARCHITECTURE_REASSESSMENT.md#9-re-evaluate-the-voice-process-boundary) | Accepted 2026-08-27 |

Add one row immediately for every later `main` fix or accepted deviation. Do not close a phase with
an open row in its subsystem.

## Tooling plan

The current Windows workstation has Node 26 and rustup-managed Rust 1.97.1. Endpoint protection
repeatedly removed Rust 1.98's bundled `ld.lld.exe`, and the Visual Studio Build Tools elevation path
did not complete, so local verification uses rustup's official `x86_64-pc-windows-gnu` host
explicitly. The pinned repository toolchain remains host-neutral for Linux CI and release builds.
FFmpeg, uv, and Docker are not exposed on `PATH`; the existing backend virtual environment remains
the Python reference runner, while Docker/Linux-specific checks run in CI or on a capable host.

### Required development tools

- Stable Rust 1.94 or newer installed through rustup and pinned by `rust-toolchain.toml`, including
  rustfmt and Clippy. Standard-library cross-platform file locks stabilized in 1.89; SQLx 0.9 raises
  the effective workspace minimum to 1.94.
- `cargo-nextest` for the main test suite; `cargo test --doc` separately because nextest does not
  run doctests.
- `cargo-deny` and `cargo-audit` for source/license/advisory policy.
- `cargo-fuzz` on Linux/nightly for protocol, path, YAML, import, and provider-response parsers.
- SQLx CLI for migration creation/checking and offline query metadata.
- Node/npm for the existing frontend and generated binding checks.
- FFmpeg/ffprobe for media integration and corpus generation.
- Docker/BuildKit for release-image, cgroup, healthcheck, and non-root filesystem verification.

Optional diagnostic tools are Criterion for pure DSP/reducer microbenchmarks, `cargo llvm-cov` for
coverage gaps, `cargo-mutants` for critical reducer/path tests, and Linux `perf`/heap profiling for
measured bottlenecks. They are used where evidence is needed, not added as ceremonial gates.

### Standard gates after the workspace exists

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo nextest run --workspace --all-features
cargo test --workspace --all-features --doc
cargo deny check
cargo audit --deny warnings
cargo test -p music-protocol export_bindings
npm.cmd run lint
npm.cmd run typecheck
npm.cmd run test
npm.cmd run build
git diff --exit-code -- frontend/src/generated contracts/generated .sqlx
```

The exact commands will be added to `AGENTS.md`, CI, and repository scripts when their files exist.
Voice tests that require the separately licensed model are an explicit operator acceptance gate,
not a public CI dependency.

## Phase 0 — architecture and baseline

**Status:** Complete

Deliverables:

- [x] Choose Rust and the complete-replacement boundary.
- [x] Create `rewrite/rust` from a clean `main` at `b93f91d`.
- [x] Record the modular-monolith architecture, ownership, concurrency, storage, and cutover model.
- [x] Reassess Python-era boundaries against Rust runtime capabilities and record ADR-016.
- [x] Owner accepts ADR-016, the explicit compatibility differences, and this plan.
- [x] Treat `main` as feature-frozen for ordinary development during the rewrite.

Gate: no application implementation begins until the unchecked owner decision is resolved.

## Phase 1 — executable reference and feasibility gates

**Status:** In progress

Evidence so far:

- `contracts/reference/v1` deterministically captures 144 HTTP operations, 205 OpenAPI schemas,
  all 33 client WebSocket actions, all four server message forms, the 18-table SQLite DDL, and the
  authored mode/preset schemas. `backend/tests/test_reference_contracts.py` fails on drift.
- `music-protocol` parses and canonically re-serializes valid/defaulted Python examples for every
  action and server message type, and rejects the shared representative invalid corpus. Bounded
  wire scalars make the Python validation limits explicit before transport code consumes them.
- `music-storage` opens bundled SQLite with WAL, foreign keys, a five-second busy timeout, and at
  most four pooled connections; its standard-library lock refuses a second owner and releases on
  drop. Playback persistence uses a separate `storage_revision` compare-and-swap, and a 16-writer
  concurrency test proves one winner with stale writes rejected.
- A deterministic, synthetic-only persistence fixture populates every one of the 18 Python tables,
  preserves SQLite timestamp/JSON encodings, and covers missing, corrupt, and representative legacy
  device JSON. Rust opens that full shape with no foreign-key failures.
- RustCrypto `aes-gcm` reproduces the existing AES-256-GCM ciphertext byte-for-byte for fixed
  key/nonce input, including AAD, padded URL-safe base64, key fingerprint, and credential hint.
  Rust verifies Python Argon2id PHC hashes; new hashes explicitly retain Python's
  `m=65536,t=3,p=4` parameters. Decrypted secrets redact `Debug` output and zeroize their owned
  storage on drop.
- [`serde-saphyr` 1.1](https://github.com/bourumir-wyngs/serde-saphyr) is selected as the
  pure-safe-Rust YAML adapter. Includes and property interpolation are not compiled in; a one-MiB
  outer limit plus parser event/node/depth/alias/scalar budgets reject oversized, duplicate-key,
  multi-document, deep-flow, and alias-bomb inputs. Three current files and synthetic full cue,
  soundboard, and preset documents match Python's canonical values and round-trip semantically.
- [`lofty` 0.25.1](https://github.com/Serial-ATA/lofty-rs) is selected for native metadata formats.
  Nine synthetic Python/Mutagen files now cover AIFF, FLAC, M4A, MP3, Ogg Vorbis, Opus, WAV, WMA,
  and raw AAC. The four encoded seed containers record an exact FFmpeg build and checksum while
  remaining tiny generated silence. The corpus caught both Lofty's lossy generic Vorbis conversion
  and its hidden MP4 integer-BPM remainder, so FLAC/Vorbis/Opus and MP4 writes use concrete tag
  types. Rust writes only to create-new staged files, verifies format, duration, intended fields,
  artwork and unrelated markers, leaves the source byte-identical, and removes abandoned stages.
  WMA uses bounded FFmpeg/ffprobe subprocesses, a safe bounded ASF-duration reader, and exact
  compressed-stream hashes. Raw ADTS AAC uses FFprobe's compatible technical duration after the
  corpus rejected Lofty's estimate and is an explicit read-only metadata capability instead of
  reproducing Mutagen's current internal write failure.
- `deny.toml` rejects unknown registries and Git sources, wildcard requirements, OpenSSL/native-TLS
  backends, and both deprecated YAML implementations. RustSec findings remain denied except for
  `RUSTSEC-2024-0436`: Lofty alone pulls the archived `paste` 1.0.15 proc macro at compile time, and
  that informational exception carries an explicit removal condition. The resulting 175-crate
  graph has no reported vulnerability, unsoundness, or yanked-package finding. Six upstream
  duplicate families remain visible as review warnings (four at the SQLx/RustCrypto generation
  boundary, plus `hashbrown` and `syn`).
- The eight-crate workspace shell compiles with the accepted dependency direction and workspace
  safe-Rust/panic lints. It exists now to host Phase 1 contract and feasibility tests; no production
  endpoint is routed to Rust before its parity gate.

Capture the old system before replacing it:

- Export and normalize the current OpenAPI document.
- Build a WebSocket fixture corpus covering every action, guest/auth rules, protocol v1/v2
  projections, reconnect, sibling-client disconnect, state revisions, position epochs, queues,
  interrupts, cues, SFX, loops, and restart pruning.
- Build HTTP contract fixtures for routes, status codes, validation errors, cookies, range requests,
  multipart conflicts, partial batch failures, and SPA caching/fallback.
- Snapshot the 18-table schema, indexes, foreign keys, representative rows, JSON documents, Argon2
  hashes, AES-GCM credential records, device JSON, and mode YAML. Capture corrupt/missing device
  JSON behavior and prove deterministic one-time import plus export round trips.
- Create a deterministic test-data builder. Generate media fixtures during tests; do not commit
  private music or generated media artifacts.
- Record Python baselines for startup/scan, representative API/WS load, media streaming, full
  signal analysis, voice inference, peak RSS, and image size. Store summaries, not private paths.
- Create a differential harness capable of launching Python and Rust candidates on separate ports
  against cloned temporary state and normalizing clocks, tokens, IDs, and timestamps.

Run feasibility spikes before depending on uncertain adapters:

1. **Voice:** load the exact pinned TF1 model with tract, decide direct TF1 versus checksum-bound
   NNEF preparation, reproduce Essentia preprocessing/outputs, and soak one model-owning Rust thread
   under the 4 GB limit. Measure cancellation, panic recovery, and per-call bounds; select the Rust
   worker-process fallback if any hard-isolation condition fails.
2. **Metadata:** prove Lofty read/write round trips across every supported format/tag registry field
   without damaging audio or unrelated tags.
3. **YAML:** select a maintained parser/serializer by loading and rewriting every current mode and
   adversarial bounded fixtures; exclude deprecated/unsound crate lines.
4. **Media stream:** prove Axum range and disconnect behavior with the browser, compatibility
   client, Baton assumptions, and the headless reference.
5. **SQLite/crypto:** open a copied real-shape database and verify timestamp, JSON, Argon2, AES-GCM,
   legacy-device import, exclusive instance locking, revision compare-and-swap, and serialized
   short-write behavior from Rust.
6. **Startup/reconciliation:** compare blocking Python startup with durable-index Rust startup;
   prove missing current media is pruned, stale rows cannot escape the media root, and a failed or
   concurrent reconciliation remains visible and retryable.

Gate: every spike has measured evidence and a recorded choice. A failed in-process voice spike
selects the supervised Rust subprocess or another Rust/native runtime behind `VoiceBackend`; it does
not introduce a Python sidecar.

## Phase 2 — workspace, process shell, and CI

**Status:** In progress — the workspace, immutable configuration, supervised process shell,
compatibility health/readiness, SPA serving, generated contract pipelines, and rewrite-only
container/CI path are implemented. The container smoke gate still needs to execute on CI because
Docker is not installed in the current Windows development environment.

Create the eight-crate workspace and establish rules before feature code:

- [x] Pinned stable toolchain, Cargo lockfile, workspace lints, formatting, and release profiles.
- [x] Pure domain, application/use-case, wire protocol, storage, media, analysis, server, and output
  crate boundaries with a forbidden-dependency check;
- [x] Immutable configuration loader with current environment names, `.env` precedence, secret
  redaction, and startup validation.
- [x] Explicit `AppRuntime`, typed error model, structured tracing, server-generated correlation
  IDs, root cancellation token,
  tracked tasks, panic supervision, and secret wrappers;
- [x] Axum compatibility health plus component readiness, static SPA fallback/cache behavior,
  request/body limits, and security headers;
- [x] SQLx read pool, serialized short-write admission, WAL pragmas, exclusive instance lock,
  compatibility validator, migration baseline, backup/doctor commands;
- [x] Route-integrated OpenAPI, semantic parity report, and generated TypeScript WebSocket
  bindings, with `music-cli contracts export/check` drift enforcement.
- [x] CI verification on pull requests and `rewrite/rust` without image publishing or deployment.
- [x] Main-only build/publish/dispatch remains unchanged and still targets the Python image until
  cutover.

Schema baseline v1 derives its expected tables, columns, unique constraints, check constraints,
indexes, and foreign keys from the frozen Python contract. It accepts only documented additive
legacy gaps, refuses unknown/incompatible structures before a writable connection, and exposes the
same report through `music-cli db doctor`. A non-empty compatible legacy database is copied with
SQLite `VACUUM INTO`, reopened read-only for integrity/shape verification, hashed, fsynced, and
paired with a non-secret manifest before normalization or SQLx migration begins. Migration v1 adds
`playback_state.storage_revision`, the SQLite-owned remembered-device/import tables, and the shared
recovery journal; representative Python rows remain intact and later boots make no backup or schema
change.

The Phase-3 playback owner now starts under runtime supervision, so the `playback` readiness
component becomes `ready` only after the persisted aggregate is loaded, restart-normalized, and
owned by the bounded actor. `/api/health` remains the exact `{"status":"ok"}` liveness contract.
`GET /api/sync/state` and `/api/ws` project that same owner. Valid sessions receive the canonical
projection and can dispatch the implemented catalog-independent mutations; guests receive the
bounded self projection and cannot mutate control state. Long-lived WebSockets recheck session
state and downgrade in place after logout, revocation, or expiry.

Playback-owned effects now share that same actor instead of creating detached timer tasks. A
deadline-driven scheduler sleeps until the earliest known track end or looping-SFX tick and is
recomputed after every actor message. The library publishes an ordered `id`/`path`/`duration`
projection rather than loading complete metadata rows. Ambient tracks with unknown duration retain
the client-ended path, unknown interrupts use the compatible five-minute safety bound, and every
automatic skip carries the observed track ID so a simultaneous client skip remains idempotent.

The semantic HTTP report intentionally remains `incomplete`: it records 144 frozen Python
operations versus 90 currently registered Rust operations, rather than presenting partial work as
parity. Eighty-nine operations overlap the reference and 88 are fully schema-compatible; the
remaining implemented mismatch is the deliberately visible Rust `PlayerState` OpenAPI schema work.
The browser imports generated Rust WebSocket DTOs; a generated compatibility layer models accepted
omitted defaults and the deliberate cached-client window, while `wsValidate.ts` continues to
validate untrusted frames at runtime.

Gate: all standard Rust/frontend/security gates pass; forbidden global state/unsafe/panic fixtures
are enforced; a copied existing database passes read-only doctor; a second writer is refused; and
the Rust container boots non-root with empty storage and serves the unchanged SPA.

## Phase 3 — playback domain, actor, and WebSocket protocol

Implement the highest-value invariant early:

- [x] pure playback reducer and deterministic clock/random inputs;
- [x] persisted state normalization, revision compare-and-swap, catalog generations, and boot
  pruning;
- [x] state actor, bounded commands, watch snapshots, transient event channel, and supervision;
- [x] per-connection projection/send ownership, registration, guest projections, protocol v1/v2,
  session-backed auth downgrade, send deadlines, transient lag policy, and sibling-client
  disconnect behavior;
- [x] server advancer and loop timers as deadline-driven, supervised actor effects;
- [x] `GET /api/sync/state` and a real guest-safe `/api/ws` transport;
- [x] preserve the generated TypeScript playback bindings; the actor effects add no new wire DTOs
  or frontend behavior.

Gate: reducer property tests pass; every WebSocket fixture matches; frontend WebSocket validation,
compat-mode tests, and Baton serialization fixtures pass; stress tests show ordered latest-state
reconciliation and bounded slow-client behavior.

## Phase 4 — authentication, devices, diagnostics, and administration

**Status:** In progress — runtime authentication, remembered-device ownership, diagnostics, and
maintenance-gated backup/restore are implemented; the remaining Phase 4 work is hardening against
the full gate before moving the phase marker to complete.

- [x] Users, Python-compatible Argon2 verification, dummy verification, bounded login hashing,
  direct-peer/global throttles, opaque sessions, configured cookies, revocation, expiry, and
  active-session APIs.
- [x] SQLite remembered-device table, audited one-time bounded legacy JSON import that preserves
  its source, and live connected/default-output projections without coupling saved designation to
  current activation.
- [x] Versioned remembered-device CLI export/import with no-clobber export, bounded strict input,
  transactional replacement, and an explicit `--replace` gate for populated targets.
- [x] Offline `create-user`, `set-password`, database doctor/migrate, healthcheck, and contract
  commands. Password input defaults to a hidden prompt; password replacement atomically revokes
  active sessions unless the operator explicitly preserves them.
- [x] Authenticated diagnostics from the live playback actor, library coordinator, and mode
  coordinator, preserving the existing frontend response contract without exposing log-only
  internals.
- [x] Maintenance-gated, authenticated backup streams a verified versioned `tar.gz` from disk.
  The manifest hashes the SQLite snapshot and every mode file, records empty mode directories, and
  carries only the one-way assistant credential-key ID. The master key and legacy `devices.json`
  are never archived; remembered devices are already SQLite-owned.
- [x] Offline `music-cli backup restore` requires both `--replace` and `--server-stopped`, verifies
  archive bounds, paths, payload hashes, schema integrity, and credential-key pairing before any
  replacement, stages on each target filesystem, and retains the prior database, WAL/SHM files,
  and modes tree. `music-cli backup recover` rolls back an interrupted journaled restore, and the
  server refuses startup while such a journal exists.
- [x] Compatibility liveness, component readiness/degradation, security headers, and safe
  `detail`/error-code/correlation-ID mappings.

The HTTP and WebSocket paths share one opaque-session service and the configured cookie name.
Authenticated WebSockets periodically revalidate against SQLite and become guests without a
reconnect when their session disappears. Password verification is capped at two concurrent Argon2
calls; login attempts have both a direct-peer bucket and a global process bucket, and forwarded
address headers are deliberately ignored until a trusted-proxy policy exists. The frozen OpenAPI
comparison reports all eight auth/device operations as fully compatible, while runtime-only failure
statuses remain safe and tested.

Gate: cross-language password/session fixtures, auth route differential tests, credential-free
backup checks, symlink/permission tests, and long-lived WebSocket downgrade tests pass.

## Phase 5 — library, metadata, streaming, uploads, and cleanup

- [x] Typed `LibraryPath`/`SfxPath` values with canonical POSIX-relative encoding and matching
  rooted filesystem capabilities that reject absolute, traversal, platform-prefix, control-byte,
  and symlink-escape inputs.
- [x] Single-owner `LibraryCoordinator` for reconciliation and journaled folder mutations, with
  startup replay before catalog publication and transactional path rewrites that preserve track IDs.
- [x] Typed catalog records and query ports, SQLite-backed literal search/stable sorting/batch and
  directory lookup, plus durable generation/reconciliation state and catalog-count backfill in
  schema v3.
- [x] Durable-index startup, generation-checked full reconciliation, visible scan status,
  tree/folder/search/batch/rescan HTTP APIs, metadata fallback, and source signatures.
- [x] Incremental catalog updates after every committed app-managed library mutation. Folder and
  track move/delete operations update the durable catalog directly; metadata is refreshed as part
  of the same journaled operation or through generation-checked reconciliation where required.
  Optional watcher hints remain future work.
  Folder delete updates the catalog directly; folder rename does the same and then refreshes metadata
  through a generation-checked reconciliation.
- [x] Chunked full/single-range media streaming with ETag/conditional handling, bounded cover
  extraction and folder fallback, inert MIME allow-listing, and disconnect-safe file bodies.
- [x] Streaming uploads with bounded multipart framing, file-count and per-file byte limits;
  serialized `rename`/`overwrite`/`skip` resolution; no-clobber create publication; journaled
  replacement and startup replay; one final catalog publication; and compatible upload/check APIs.
- [x] Shared bounded recovery-journal types and compare-and-swap persistence with explicit legal
  transitions and cross-domain ownership.
- [x] Staged metadata editing with journal replay, verified tag replacement, atomic index commits,
  stable track identities, explicit clear/unset semantics, and compatible single/bulk HTTP routes.
  Mixed bulk updates report per-track tag failures while still applying DB-only fields; a failure
  after file replacement stops for forward recovery.
- [x] Separate `SfxCoordinator` with coherent bounded inventories and journaled folder/file
  create/rename/move/delete plus serialized `rename`/`overwrite`/`skip` upload publication.
  Typed-root effects skip symlinks, replay interrupted operations forward, clean only exact
  internal upload artifacts, and expose all 10 frozen reference-gated playback and authenticated
  management operations with compatible OpenAPI contracts.
- [x] Pure deterministic cleanup analysis with no write-capable dependency, cached-verdict reads,
  bounded all/folder/track scopes, conservative collision handling, and a schema-compatible
  `/api/library/cleanup/analyze` route.
- [x] Bounded, authenticated cleanup-name verification with process-wide MusicBrainz pacing,
  identifiable requests, strict time/response limits, idempotent per-name cache commits, retryable
  failures, and a schema-compatible `/api/library/cleanup/verify` route.
- [x] Cleanup batch history reads and domain-specific journaled apply. Accepted tag, track-rename,
  and deepest-first folder-rename operations enter the single library writer; the catalog update,
  compatible history append, and cleanup-journal completion share one SQLite transaction. Exact
  pre-write file tags are retained for faithful revert, and startup replay closes the filesystem /
  database crash window.
- [x] Domain-specific cleanup batch and uploaded-journal revert. Inverses run in reverse journal
  order through `LibraryCoordinator`, stale-check IDs plus durable paths, preserve exact pre-write
  tags, and emit per-item skips rather than clobbering drift. Child recovery journals close each
  filesystem/catalog window; a batch parent journal atomically records `reverted_at` and resumes
  safely after a process interruption.

Gate: generated-format metadata corpus, path property/fuzz tests, symlink/race tests, range tests,
all library/SFX/cleanup HTTP fixtures, and copied-library scan comparison pass. No private media is
committed or logged.

## Phase 6 — modes, presets, playlists, cues, and authoring

**Status:** In progress — Rust now owns the bounded mode catalog, all existing mode, soundboard,
interrupt, cue, and preset CRUD, the complete playlist HTTP surface, resolved playlist/cue playback,
exact-item SFX/loop dispatch, and all catalog-scoped WebSocket playback actions. Import authoring
remains.

- [x] `ModeCoordinator`, typed immutable catalog snapshots, generations, last-good reload behavior,
  health/diagnostic status, and authenticated plus guest-compatible read routes.
- [x] Mode/soundboard/interrupt/cue/preset CRUD through the single coordinator, with typed
  validation, staged and hashed YAML candidates, SQLite recovery journals, startup rollback,
  catalog-change publication, and active-preset revision updates. All 19 frozen write operations
  are OpenAPI-compatible.
- [x] Playlist CRUD and duplicate-preserving contiguous ordering, automatic-rule preview and atomic
  materialization, current manual/local-analysis tag sources, last-good damaged-rule behavior,
  M3U/JSON export, and generation-checked playback resolution through the playback actor. All 14
  frozen playlist operations are OpenAPI-compatible.
- [x] Generation-checked SFX and looping-SFX dispatch through the playback actor. Compact mode
  publications carry exact soundboard item paths, invalid items are rejected before broadcast,
  and a mode edit prunes loops whose item disappeared even when the soundboard still exists.
- [x] Atomic generation-checked cue dispatch through the playback actor. Preset activation,
  mode-scoped named-playlist resolution, initial position, stable replacement loops, durable state,
  and transient SFX are validated before one reducer/persistence boundary; a failed persistence
  cannot emit a partial cue.
- [x] Generation-checked mode selection, direct track, whole-queue, enqueue, recursive folder,
  soundboard, preset, and single-track interrupt WebSocket actions. Queue validation is all-or-none,
  preset crossfade remains last-active-wins, output selection rejects newly added disconnected IDs,
  and manual follow-mode advance resolves inside the playback actor.
- Authoring document schema, source adapters, preview/dependency validation, journaled commit, and
  create-only conflict behavior.

Gate: all current modes load; YAML round-trip diffs are understood; authoring v1 fixtures remain
compatible; playback integration covers cue and preset effects; failed multi-file commits recover
without half-authored state.

## Phase 7 — durable job framework

- Typed job registry with persisted lane/schema/restart/checkpoint policy, per-claim execution IDs,
  transactional claim, checkpoints, cancellation, retry, recovery, and shutdown behavior.
- Async coordinator handlers with CPU work restricted to the fixed analysis executor, filesystem
  work restricted to bounded media workers, and provider calls restricted to bounded async I/O.
- Historical unknown-job rendering.
- Jobs HTTP API and generated frontend types.
- Test-only fault injection at claim, external effect, checkpoint, completion, and shutdown points.

Gate: restartable work resumes only from safe checkpoints; provider work never repeats silently;
lane isolation, cancellation, refresh restoration, and SQLite contention tests pass.

## Phase 8 — deterministic Assistant and human-owned review flows

- Local playlist planner, automatic playlist evidence, audio-analysis record handling, and
  evaluation suites.
- Mood vocabulary, manual tags, analysis review decisions, cleanup rules, and atomic bulk changes.
- Structured task definitions, strict result types/schemas/examples, fingerprints, and local
  identity/bounds checks.
- EQ baseline/envelope and Authoring draft integration.

Gate: every checked-in synthetic suite produces equivalent or intentionally rebaselined results;
generated output cannot mutate manual tags/playlists/presets without the existing explicit review
transaction; malformed/stale data fails safely.

## Phase 9 — provider connections and model jobs

- AES-GCM vault and offline audit/rotation workflows.
- Hardened pinned-DNS transport and versioned transport-free adapter handlers.
- Verification, role assignment, conformance, runtime fingerprints, certification reset, and
  capability gates.
- Playlist, EQ, tagging, cleanup, and quality jobs with exact disclosures, request bounds,
  correction budget, usage checkpoints, and review-only results.

Gate: crypto compatibility, SSRF/DNS rebinding, redirect, proxy, timeout, size, secret-leak, schema,
identity, retry, fingerprint, quality, and configured-provider conformance tests pass. Live provider
tests require explicit consent and never become routine CI.

## Phase 10 — local context and voice analysis

- Streaming FFmpeg PCM adapter, cancellation, deadlines, and bounded stderr.
- Reused signal buffers, RustFFT/Mel features, trajectories, tempo, structure, reliability, and
  source signatures.
- Single-pass EBU R128 integration after corpus parity.
- Bounded track pool, serialized SQLite checkpoints, partial/failure rows, profiling, and UI job
  progress compatibility.
- Supervised single-model inference thread and retryable second phase; exercise the Rust subprocess
  implementation when Phase 1 selected hard isolation.

Gate: controlled probes pass; representative numeric output is within field tolerances or carries a
new documented analyzer identity; no semantic inference is added; production-shaped three-CPU/4 GB
soak completes with the memory margin and no upward inference RSS trend; cancellation/shutdown meets
the selected thread or process boundary's deadline.

## Phase 11 — CLI and Rust headless output appliance

- Recreate every `music-cli` command with compatible safe defaults and add `db doctor`, migration,
  healthcheck, device import/export, and contract-export commands.
  The new database, contract, healthcheck, user/password, and device-transfer commands are already
  implemented; library, provider, job, and evaluation commands follow their owning coordinators.
- Implement `music-output` using the shared protocol, WebSocket ping/reconnect, stable ID,
  position-epoch reconciliation, server/local volume, position reports, SFX, and local control API.
- Supervise two local mpv processes through Unix-socket JSON IPC; use no libmpv FFI.
- Update systemd installation and client documentation; remove Python package requirements.

Gate: reconciler fixtures match the Python appliance, mpv process restart and network reconnect are
safe, local control auth/CORS behavior matches, and a Linux speaker-device smoke test is recorded.

## Phase 12 — full parity, hardening, and Python removal

- Run the complete differential harness and classify every difference.
- Run all Rust, frontend, compatibility, security, fuzz-regression, corpus, soak, and release-image
  gates.
- Confirm every Python feature and CLI/client entry in the inventory has a Rust owner and test.
- Update README, Assistant docs, client docs, environment examples, architecture map, AGENTS, and
  deployment references.
- Delete Python source, tests, lockfiles, virtual-environment instructions, and image stages only
  after their replacement evidence passes.
- Scan the final tree and image for accidental Python/runtime remnants, secrets, generated media,
  stale contract references, mutable service globals, and unused dependencies.

Gate: the final branch builds and tests from a clean clone, the release image contains no Python
runtime, and the definition of done below is satisfied.

## Phase 13 — cutover and rollback window

Cutover is a separately authorized operation:

1. Stop or finish all active durable jobs and record a final Python health/diagnostic snapshot.
2. Create an application-consistent backup of `app.db`, `devices.json`, modes, secrets-key pairing,
   and relevant deployment configuration. Media need not be duplicated if the rewrite never mutates
   it during migration, but the existing storage backup policy remains authoritative.
3. Tag the final Python `main` commit and create `legacy/python` at that exact commit.
4. Run the Rust `db doctor` and migration against a copy, start the release image against the copy,
   and execute the production smoke suite.
5. Merge `rewrite/rust` to `main` without rewriting history. Push/deploy only after explicit owner
   approval, remembering that a main push triggers the image and infrastructure workflow.
6. Stop the Python container, migrate the real database including remembered-device import, start
   Rust, and verify liveness/readiness, login, device state, library reconciliation, range playback,
   WebSocket control/output, reconnect, SFX/cues, modes/presets, jobs, provider readiness reset,
   local analysis, and the headless output.
7. Keep the Python image, legacy branch, and pre-migration backup for the agreed observation window.

Rollback stops Rust, restores the pre-cutover database, legacy `devices.json`, and paired secret key
if needed, and starts the tagged Python image. Do not attempt a code-only rollback across an
unreviewed migrated database.

## Per-slice implementation loop

For every vertical slice:

1. Read this architecture, the current Python contract, its tests, and the frontend/compat consumer.
2. Add or refine reference fixtures before candidate code.
3. Implement the smallest complete domain -> storage/adapter -> HTTP/WS path.
4. Run narrow Rust tests and the matching Python/reference differential tests.
5. Run formatter, Clippy, affected frontend tests, and schema/binding drift checks.
6. Profile only when the slice is performance-sensitive; compare against recorded evidence.
7. Review for unbounded channels/tasks/bodies, broad clones, locks across await/I/O, path escapes,
   secret-bearing errors, blocking Tokio work, and missing cancellation/checkpoints.
8. Update phase evidence and docs, then commit only that logical slice.

Any architectural deviation—new service, new persistence engine, protocol break, permanent FFI,
Python fallback, unbounded concurrency, or relaxed review/security boundary—requires an ADR update
and owner decision before implementation continues.

## Risk register

| Risk | Mitigation and stop condition |
|---|---|
| Voice graph/preprocessing cannot be reproduced or bounded in-process | Phase-1 exact-model/thread spike before dependent work; supervised Rust process or alternate native runtime behind `VoiceBackend`; no Python sidecar. |
| Metadata writes damage uncommon formats | Generated corpus plus private dry-run copies; temp-copy/reread/atomic replace; unsupported formats fail per item. |
| Hidden frontend/protocol coupling | Generated types, runtime guards, OpenAPI/WS fixtures, differential tests, old-TV and Baton compatibility gates. |
| Existing SQLite shapes differ from models | Schema doctor inspects real copies and refuses unknown shapes; mandatory backup; additive-first migrations. |
| Rust compiles but is inefficient | Explicit concurrency budgets, clone/lock review, Criterion/corpus profiling, cgroup soak, before/after report. |
| Long branch misses urgent fixes | Feature freeze plus compatibility ledger and immediate test/fixture port for every main fix. |
| LLM introduces compiler-appeasing architecture debt | Workspace lints, forbidden unsafe/panics, pure domain boundaries, bounded slice size, explicit review checklist. |
| Cutover triggers automatic deployment unexpectedly | No push without authorization; document main workflow side effect; perform copy migration and smoke gate first. |

## Definition of done

The rewrite is complete only when:

- no project-owned Python runtime code or Python production dependency remains;
- React, compatibility mode, Baton, and the Rust output appliance work against the Rust server;
- all required HTTP/WS/CLI/data compatibility fixtures pass or have explicit accepted differences;
- existing user data, credentials, modes, and device state migrate safely, and legacy device JSON
  can be explicitly exported/imported without remaining a second authority;
- every durable job obeys its restart/cancellation/checkpoint contract;
- provider and generated-content safety/review boundaries remain intact;
- analysis output is calibrated and the three-CPU/4 GB production-shaped soak passes;
- all repository and release-image gates pass from a clean checkout;
- docs and operator procedures describe only the Rust production path;
- a tested backup and rollback path exists; and
- the owner explicitly approves main replacement and deployment.
