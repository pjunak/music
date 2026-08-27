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
- `deny.toml` rejects unknown registries and Git sources, wildcard requirements, OpenSSL/native-TLS
  backends, and both deprecated YAML implementations. The current 155-crate graph passes RustSec
  with warnings denied; six upstream duplicate families remain visible as review warnings (four at
  the SQLx/RustCrypto generation boundary, plus `hashbrown` and `syn`).
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

Create the eight-crate workspace and establish rules before feature code:

- pinned stable toolchain, Cargo lockfile, workspace lints, formatting, and release profiles;
- pure domain, application/use-case, wire protocol, storage, media, analysis, server, and output
  crate boundaries with a forbidden-dependency check;
- immutable configuration loader with current environment names and startup validation;
- explicit `AppRuntime`, typed error model, tracing, correlation IDs, root cancellation token,
  tracked tasks, panic supervision, and secret wrappers;
- Axum compatibility health plus component readiness, static SPA fallback/cache behavior,
  request/body limits, and security headers;
- SQLx read pool, serialized short-write admission, WAL pragmas, exclusive instance lock,
  compatibility validator, migration baseline, backup/doctor commands;
- OpenAPI and TypeScript export pipelines;
- CI verification on pull requests and `rewrite/rust` without image publishing or deployment;
- main-only build/publish/dispatch retained but still targeting the Python image until cutover.

Gate: all standard Rust/frontend/security gates pass; forbidden global state/unsafe/panic fixtures
are enforced; a copied existing database passes read-only doctor; a second writer is refused; and
the Rust container boots non-root with empty storage and serves the unchanged SPA.

## Phase 3 — playback domain, actor, and WebSocket protocol

Implement the highest-value invariant early:

- pure playback reducer and deterministic clock/random inputs;
- persisted state normalization, revision compare-and-swap, catalog generations, and boot pruning;
- state actor, bounded commands, watch snapshots, transient event channel, and supervision;
- per-connection projection/send ownership, registration, guest projections, protocol v1/v2, auth
  downgrade, send deadlines, transient lag policy, and sibling-client disconnect behavior;
- server advancer and loop timers as supervised actor effects;
- `GET /api/sync/state` and `/api/ws`;
- generated TypeScript protocol bindings without changing frontend behavior.

Gate: reducer property tests pass; every WebSocket fixture matches; frontend WebSocket validation,
compat-mode tests, and Baton serialization fixtures pass; stress tests show ordered latest-state
reconciliation and bounded slow-client behavior.

## Phase 4 — authentication, devices, diagnostics, and administration

- Users, Argon2 verification, dummy verification, login throttles, opaque sessions, cookies,
  revocation, expiry, and active-session APIs.
- SQLite remembered-device table, one-time legacy JSON import, explicit CLI export/import, and live
  connected/default-output projections.
- Diagnostics, maintenance-gated streaming backup/restore verification, storage initialization,
  create-user, set-password, and database doctor.
- Compatibility liveness, component readiness/degradation, security headers, and safe
  `detail`/error-code/correlation-ID mappings.

Gate: cross-language password/session fixtures, auth route differential tests, credential-free
backup checks, symlink/permission tests, and long-lived WebSocket downgrade tests pass.

## Phase 5 — library, metadata, streaming, uploads, and cleanup

- Rooted path types and the single-owner `LibraryCoordinator`.
- Durable-index startup, generation-checked full/incremental reconciliation, visible scan status,
  tree/folder/search APIs, metadata fallback, and source signatures.
- Full/range media and cover streaming with disconnect-safe file handles.
- Streaming uploads and explicit conflict handling.
- Shared staged-file/recovery infrastructure; metadata edits, moves, bulk operations, folders, SFX
  files, and per-item partial failures.
- Pure cleanup analysis, verification, domain-specific journaled apply, history, and revert.

Gate: generated-format metadata corpus, path property/fuzz tests, symlink/race tests, range tests,
all library/SFX/cleanup HTTP fixtures, and copied-library scan comparison pass. No private media is
committed or logged.

## Phase 6 — modes, presets, playlists, cues, and authoring

- `ModeCoordinator`, typed immutable catalog snapshots, generations, and reload status.
- Mode/soundboard/interrupt/cue/preset CRUD with staged writes, recovery journal, catalog-change
  notification, and preset revision updates.
- Playlist ordering, automatic rules, materialization, export, and playback resolution.
- SFX/loop/cue dispatch through the playback actor.
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
