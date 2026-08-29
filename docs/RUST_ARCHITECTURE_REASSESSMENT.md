# Rust rewrite architecture reassessment

**Status:** Historical input, accepted and implemented

**Date:** 2026-08-27

**Related decisions:** [ADR-015](ADR-015-complete-rust-rewrite.md),
[ADR-016](ADR-016-rust-native-runtime-boundaries.md)

This review challenged the first Rust blueprint against the final Python implementation. It is
retained as design history; current code ownership lives in
[the production architecture](RUST_REWRITE_ARCHITECTURE.md). Python source links below point to the
frozen `b93f91d` legacy revision.

The project owner accepted ADR-016 and the explicit compatibility differences on 2026-08-27. The
recommendations below became implementation constraints, and Phase 1 captured the displaced Python
behavior as executable evidence before dependent Rust paths replaced it.

## Evidence reviewed

- Startup and composition in [`main.py`](https://github.com/pjunak/music/blob/b93f91dece3afa5ef395ebf676d7aedc51559e96/backend/app/main.py).
- Playback mutation, persistence, connection projection, and WebSocket dispatch in
  [`sync/state.py`](https://github.com/pjunak/music/blob/b93f91dece3afa5ef395ebf676d7aedc51559e96/backend/app/sync/state.py),
  [`sync/connection.py`](https://github.com/pjunak/music/blob/b93f91dece3afa5ef395ebf676d7aedc51559e96/backend/app/sync/connection.py), and
  [`sync/router.py`](https://github.com/pjunak/music/blob/b93f91dece3afa5ef395ebf676d7aedc51559e96/backend/app/sync/router.py).
- SQLite setup, the ad-hoc additive schema path, and the 18 current tables under
  [`core/db.py`](https://github.com/pjunak/music/blob/b93f91dece3afa5ef395ebf676d7aedc51559e96/backend/app/core/db.py) and
  [`models/`](https://github.com/pjunak/music/tree/b93f91dece3afa5ef395ebf676d7aedc51559e96/backend/app/models).
- Filesystem indexing and mutation in
  [`library/index.py`](https://github.com/pjunak/music/blob/b93f91dece3afa5ef395ebf676d7aedc51559e96/backend/app/library/index.py), mode
  loading/writes, Authoring commits, and cleanup journals.
- Durable job recovery and execution in
  [`jobs/`](https://github.com/pjunak/music/tree/b93f91dece3afa5ef395ebf676d7aedc51559e96/backend/app/jobs), plus the staged context and
  voice workers.
- Assistant provider, disclosure, fingerprint, quality, and review contracts in
  [the Assistant architecture map](ASSISTANT_ARCHITECTURE.md).
- The external output contract and unchanged Baton boundary in
  [the client guide](../clients/README.md).

## What is architectural intent and should remain

| Existing decision | Why it remains correct in Rust |
|---|---|
| The server owns one canonical playback state | Multiple controllers and outputs need one ordering authority regardless of language. |
| Playback transitions are reducer-like and clients reconcile full snapshots | This makes reconnect, dropped frames, legacy projections, and deterministic tests simpler. |
| SQLite is the operational database | The workload is single-instance, local, transactional, and far below the point where a database service helps. |
| Media stays in the filesystem and modes stay in YAML | Audio is naturally file-backed; mode documents are portable, operator-owned content rather than opaque runtime state. |
| Long work is durable and carries explicit restart/cost policy | Crashes and provider charges do not become less important in Rust. |
| Generated/model output remains review-only | This is an ownership and safety contract, not a Python workaround. |
| FFmpeg and mpv remain process boundaries | They provide mature codec and playback behavior that a language rewrite does not improve. |
| Baton and the existing HTTP/WebSocket contract remain compatible | Baton is a separate repository and is explicitly outside this rewrite. |
| A single deployable modular monolith remains the target | One operator and one playback universe do not justify distributed coordination. |

## Python-era structure that should not survive

| Current pattern | Why it exists now | Rust disposition |
|---|---|---|
| Route modules mix DTOs, SQL, filesystem work, and domain decisions | FastAPI dependency injection made direct handler orchestration convenient. | Add an application/use-case layer; handlers become translation and authorization adapters. |
| Mutable module globals own state, devices, modes, jobs, timers, and connections | Python modules are the current composition mechanism. | Build one `AppRuntime` at startup and inject explicit handles; no mutable process-global service state. |
| `asyncio.Lock` acts as the playback "actor" while callers provide arbitrary mutators | It serializes callers without a dedicated command owner. | Use a real bounded command task with exhaustive commands and a pure reducer. |
| Playback installs the candidate in memory before its awaited database write succeeds | The lock and mutable object are the current transaction boundary. | Compute a candidate, persist it with revision compare-and-swap, and only then replace/publish the in-memory state. |
| State broadcast waits under a global ordering lock while all socket sends complete | It prevents stale baselines but lets a slow client delay unrelated clients. | Each socket owns its send path; latest state uses `watch`, transient events use a bounded channel. |
| A full filesystem scan blocks boot before state hydration | The synchronous index was easiest to initialize in the FastAPI lifespan. | Boot from the durable index, validate live references, then run reconciliation as a durable background job. |
| `devices.json` survives because incompatible schema changes previously meant deleting `app.db` | There is no general migration ledger in the Python application. | Real migrations remove the wipe policy; import remembered devices into SQLite and retain explicit export/recovery tooling. |
| Multiple thread locks approximate SQLite and cross-resource ownership | Sync SQLAlchemy and filesystem calls run in thread pools. | One storage write gate plus typed library/mode coordinators make ownership explicit. |
| CPU work uses spawned Python processes and voice workers are recycled | The GIL and native Essentia/TensorFlow memory behavior require process isolation. | Use a fixed Rust CPU executor; try one model-owning Rust thread first and retain a Rust subprocess only if measurement proves isolation necessary. |
| Job restart policy is inferred from the current in-memory handler registry | The job table predates the mature job contract. | Persist lane, policy version, restart policy, and attempt identity with each new job. |
| `/api/health` is unconditional and unhandled exceptions are returned verbatim | The service optimized for direct personal debugging. | Preserve the compatibility health route, add component readiness, and return correlation IDs instead of internal paths/SQL/errors. |

## Recommended target changes

### 1. Add a real application boundary

The first blueprint put application services, actors, jobs, provider transport, and Axum assembly in
`music-server`. That would recreate a large framework-facing package similar to today's API modules.
The revised workspace adds `music-application` and `music-media`:

```text
crates/
  music-domain/       Pure values, aggregates, reducers, policies, result documents
  music-application/  Commands, queries, use cases, coarse external ports
  music-protocol/     Wire-only HTTP/WS DTOs, schemas, TypeScript export
  music-storage/      SQLx schema, migrations, typed transactions, repositories
  music-media/        Safe paths, metadata, streaming, staged filesystem mutation, FFmpeg
  music-analysis/     Bounded DSP and optional voice backends
  music-server/       Axum adapter, concrete provider transport, composition, server/CLI bins
  music-output/       Shared-protocol headless mpv appliance
```

`music-protocol` does not expose domain types directly. Explicit translation keeps compatibility
fields and legacy projections from distorting the internal model. `music-application` defines only
coarse ports at real side-effect boundaries; it does not create one mock repository trait per table.

### 2. Give runtime resources explicit owners

`AppRuntime` owns configuration, storage, snapshots, coordinator handles, health state, and shutdown.
It supervises these concrete owners:

- `PlaybackActor`: canonical playback, presence, revisions, clocks, and timers.
- `LibraryCoordinator`: app-managed media mutations and index reconciliation commits.
- `ModeCoordinator`: staged YAML changes and immutable catalog publication.
- `JobCoordinator`: durable job admission, recovery, attempts, and cancellation.
- `AnalysisExecutor`: fixed CPU slots and the optional model owner.

Tokio tasks are tracked and cancelled as a tree. A critical owner exiting makes readiness fail and
starts controlled shutdown; an optional analyzer becoming unavailable is reported as degradation.
This uses Tokio's documented `CancellationToken` and `TaskTracker` shutdown pattern rather than
detached tasks ([TaskTracker documentation](https://docs.rs/tokio-util/latest/tokio_util/task/task_tracker/struct.TaskTracker.html)).

### 3. Keep one real-time authority without coupling it to sockets

The playback actor owns live presence because connection count determines canonical output cleanup.
It does not own WebSocket objects or perform network writes. Each connection task:

1. subscribes to the latest immutable internal state;
2. creates its guest/protocol-specific wire projection;
3. sends on its own deadline; and
4. submits validated commands through the bounded actor mailbox.

Tokio `watch` intentionally keeps only the newest value, which matches the existing full-snapshot
reconciliation model ([Tokio watch documentation](https://docs.rs/tokio/latest/tokio/sync/watch/)).
Late transient SFX events are dropped rather than replayed. A slow socket can disconnect itself but
cannot delay the actor or another output.

Resolved playback commands carry the catalog generation they were resolved against. A committed
library or mode mutation sends a typed catalog-change command; stale commands are rejected and
retried instead of installing references to resources that were concurrently removed.

### 4. Make operational data ownership consistent

The target ownership rule is:

- SQLite: users, sessions, remembered devices, playback snapshot, library index, playlists, jobs,
  Assistant configuration/results/reviews, and recovery journals.
- YAML: operator-authored modes, cues, soundboards, and presets.
- Media roots: audio, SFX, and cover/tag bytes.
- External secret mount/environment: the credential master key.

At cutover, a migration imports `devices.json` only when the new `remembered_devices` table is empty,
records the source fingerprint, and leaves the original file untouched for rollback. Rust then uses
SQLite as authority. `music-cli devices export/import` replaces database-wipe survival with an
explicit, testable recovery path.

Moving mode YAML into SQLite is rejected: it would reduce portability and make ordinary authored
content harder to inspect or version. "One language" does not require one storage format.

### 5. Align SQLite concurrency with SQLite itself

WAL allows readers alongside a writer but still permits only one writer at a time
([SQLite WAL documentation](https://www.sqlite.org/wal.html)). `music-storage` therefore owns:

- a small bounded SQLx pool for reads;
- one process-local asynchronous write-admission gate;
- short explicit write transactions; and
- a bounded busy timeout for external contention.

No database gate is held across media, network, provider, or model work. Job checkpoints and actor
state commits enter the same short-write path. SQLx exposes WAL, foreign-key, busy-timeout, and
bounded command/row buffering directly
([SQLx SQLite options](https://docs.rs/sqlx/latest/sqlx/sqlite/struct.SqliteConnectOptions.html)).

The server also holds an exclusive lock file beside `app.db`. Rust's standard library supports
cross-platform file locks, so no FFI or extra locking crate is required
([`std::fs::File::try_lock`](https://doc.rust-lang.org/stable/std/fs/struct.File.html#method.try_lock)).
Offline mutating CLI commands require the same lock. This prevents an accidental second container
or maintenance process from creating two canonical owners.

Backup no longer builds a complete tarball in application memory. A maintenance command briefly
stops new writes, creates and verifies a SQLite-consistent snapshot in a temporary workspace,
records a manifest and secret-key fingerprint (never the key), then streams the bounded archive.
Failure before verification publishes no backup. Restore remains an offline, explicit workflow.

### 6. Remove full scanning from the critical boot path

Startup becomes staged:

1. validate immutable configuration and acquire the instance lock;
2. inspect, back up when required, and migrate SQLite;
3. load the last valid mode catalog and normalize the playback snapshot;
4. start critical coordinators and bind HTTP/WebSocket;
5. expose readiness; and
6. enqueue library reconciliation and other degradable probes.

Only currently referenced media is checked synchronously. The indexed catalog remains available
while a full scan runs, and the UI/diagnostics report `reconciling`, `current`, or `failed`. Media
serving still checks the actual file, so a stale row never turns into an unsafe read.

Discovery and metadata reads occur outside the mutation coordinator. The scan result carries the
library generation; its short database commit is rejected/reconciled if an app-managed mutation
changed the generation meanwhile. A filesystem watcher remains an optional later accelerator, not
an authority, because bind mounts and network filesystems can lose events.

### 7. Generalize the proven journal pattern, not the domain

Cleanup and Authoring already demonstrate the right principle: plan, review/stage, journal, execute,
and recover. Rust extends the infrastructure to every filesystem-plus-database mutation while
keeping domain-specific plans and rollback rules.

The shared facility allocates an operation ID, stage/backup paths on the target filesystem, and a
durable phase record. It does not pretend SQLite and a media volume can share one transaction.
Recovery either completes a validated commit or restores the recorded prior state.

### 8. Strengthen durable attempts without adding a broker

The two useful job lanes remain: one local whole-library/mutating job and one cost-bearing provider
job. A generic resource scheduler or message broker would add more policy than the workload needs.

New jobs additionally persist their lane, parameter schema version, restart policy, checkpoint
version, and a fresh `execution_id` for every claim. Progress/final writes compare that execution ID,
so a cancelled or superseded attempt cannot overwrite a newer attempt. Provider attempts retain the
current pre-call and post-call usage checkpoints and are never silently replayed.

Handlers are asynchronous coordinators. CPU work goes only through `AnalysisExecutor`; blocking
filesystem work uses bounded media workers; provider HTTP uses bounded async I/O. Tokio's general
blocking pool is not treated as the application's CPU budget.

### 9. Re-evaluate the voice process boundary

The default process boundary in ADR-015 copied a valid Python/Essentia mitigation into a different
runtime. The first Rust candidate instead gives the model to one dedicated, bounded inference
thread. That preserves a single loaded model and keeps non-`Send` inference state off Tokio while
avoiding IPC, duplicate binaries, and forced process recycling.

This is conditional, not optimistic. Phase 1 must prove exact preprocessing/output parity, bounded
per-track time, cancellation between windows, stable repeated-run RSS, and controlled recovery from
a worker panic. If the selected backend contains unsafe native code, leaks, wedges, or cannot meet
shutdown deadlines, the same `VoiceBackend` runs in a supervised Rust subprocess. No Python sidecar
is allowed.

Tract still documents TF1 frozen-graph loading, but its current public facade treats TensorFlow as a
legacy format. The gate must decide whether to load the graph directly or prepare a checksum-bound
NNEF artifact before runtime; internal tract crates are not assumed stable
([tract documentation](https://github.com/sonos/tract/blob/main/README.md)).

### 10. Separate public compatibility from internal correctness

- HTTP and WebSocket DTOs remain wire-only and versioned independently from domain types.
- Rust matches over closed command/event enums exhaustively instead of using a class-to-handler map.
- `ts-rs` warnings for unsupported Serde attributes fail contract generation rather than being
  silenced; its own documentation notes that not every Serde attribute is supported
  ([ts-rs documentation](https://docs.rs/ts-rs/latest/ts_rs/trait.TS.html)).
- Checked-in OpenAPI, JSON Schema, WebSocket fixtures, frontend guards, compat-mode tests, and Baton
  fixtures remain the compatibility evidence.
- The existing `detail` field remains, but unexpected failures return a stable code, safe message,
  and correlation ID. Full internal error chains stay in structured logs.

### 11. Make health reflect ownership

`/api/health` remains the compatibility liveness response. A new readiness/diagnostics surface
reports database/schema, instance lock, playback actor, mode catalog, job coordinator, library
reconciliation, FFmpeg, and optional voice status separately.

Core failure means not ready and controlled shutdown. Optional analysis or a failed reconciliation
means degraded-but-usable, so the operator can still open the UI and repair the problem. No external
metrics stack is required; bounded counters, timings, component state, and structured tracing are
enough for this personal deployment.

## Rust-specific safety policy

- Project crates use `#![forbid(unsafe_code)]`; any unavoidable future FFI lives in a separately
  approved adapter crate with a safety contract.
- Production paths deny uncontrolled `unwrap`, `expect`, `panic!`, `todo!`, and `unimplemented!`.
- Release builds keep panic unwinding so supervisors can observe a failed task/thread; critical
  panics initiate shutdown rather than being ignored.
- Channels, request bodies, response bodies, subprocess stderr, query result sets, and generated
  documents are bounded.
- Locks are not held across unrelated I/O. Long ownership is represented by a coordinator command,
  not a borrowed global mutex guard.
- Dependency versions and features are pinned and audited; SQLite extension loading stays disabled.

Rust makes these rules enforceable, but it does not make them automatic. Differential tests,
property tests, corpus tests, and cgroup measurements remain the acceptance mechanism.

## Tempting changes rejected or deferred

| Change | Disposition |
|---|---|
| Event-source playback | Rejected. A materialized aggregate plus revision gives deterministic recovery without replay/versioning complexity. |
| CQRS framework or in-process event bus | Rejected. Direct typed commands and narrow notifications are easier to trace at this scale. |
| Microservices or separate job service | Rejected. They would create distributed failure modes for one machine and one operator. |
| PostgreSQL or Redis | Rejected. They solve scale and coordination problems the application does not have. |
| Store modes in SQLite | Rejected. YAML portability and operator ownership are material benefits. |
| Replace FFmpeg/mpv immediately | Rejected. Codec/playback parity risk exceeds likely benefit. |
| Filesystem watcher as truth | Deferred. It may accelerate reconciliation but cannot replace explicit scans and file checks. |
| Generic concurrent resource scheduler | Deferred. Two durable lanes plus bounded executors encode the actual workload more clearly. |
| Mandatory voice subprocess | Replaced by an evidence-gated fallback. Rust removes the GIL/native-Python reason, but measurement may restore the boundary. |

## Owner decisions required before Phase 1

1. Accept the eight-crate application boundary and explicit runtime coordinators.
2. Accept migrating remembered devices from `devices.json` into SQLite with import/export tooling.
3. Accept non-blocking boot reconciliation and its temporary, visible `reconciling` state.
4. Accept safe correlation-ID errors instead of returning raw internal exception details.
5. Accept in-process Rust voice inference as the first candidate, with a Rust subprocess as the
   mandatory fallback when the feasibility or soak gate fails.

If accepted, Phase 1 captures the Python oracle and proves these boundaries before feature porting.
If any item is rejected, ADR-016 and the target architecture must be amended before implementation.
