# ADR-016: Refine the Rust runtime around explicit application and resource ownership

**Status:** Accepted and implemented

**Date:** 2026-08-27

**Decider:** Project owner

**Implemented:** 2026-08-29 as the eight-crate Rust workspace described by the production
architecture and enforced by `.github/scripts/rust-architecture.mjs`.

**Implementation update (2026-08-29):** The checksum-pinned MusiCNN TensorFlow graph runs directly
through tract 0.23.5 after a checksum-specific, in-memory compatibility normalization for four fixed
padding operators and `FusedBatchNormV3`. One dedicated Rust thread owns the compiled graph and a
capacity-one request queue. This confirms the thread as the production implementation. For the
personal deployment, the owner accepts the private Essentia comparison and production-shaped
RSS/cancellation soak as post-cutover diagnostics rather than blockers; the subprocess fallback
remains available if that evidence later exposes a real fault.

## Context

ADR-015 selected a complete Rust modular-monolith rewrite. Its first blueprint correctly preserved
the canonical playback reducer, SQLite, YAML, FFmpeg, mpv, durable jobs, provider safety, and the
unchanged React/Baton boundary. A second review compared that blueprint with the actual Python
runtime and found several implementation-era assumptions that should not become permanent Rust
architecture.

The current application composes services through mutable module globals, mixes HTTP translation
with SQL and filesystem orchestration, blocks startup on a full library scan, broadcasts through a
global socket-send lock, keeps remembered devices outside SQLite because schema changes once
required database deletion, and isolates CPU/model work in processes because of the GIL and native
Python runtimes.

Rust changes those implementation forces, but it does not change product ownership, external
compatibility, provider safety, or the single-instance deployment constraint.

## Decision

Refine ADR-015's modular monolith as follows:

1. Add `music-application` between the pure domain and adapters, and add `music-media` for safe
   filesystem/metadata/streaming behavior. Keep wire DTOs separate from internal domain types.
2. Construct one explicit `AppRuntime`; do not use mutable module-global services. Supervise
   playback, library, mode, job, and analysis owners with bounded channels and structured shutdown.
3. Keep playback and live presence in one authoritative actor, but let each WebSocket connection
   own projection and network sending from latest-state and transient-event subscriptions.
4. Put all mutable operational records, including remembered devices and recovery journals, in
   SQLite. Keep media in its roots, authored mode content in YAML, and the credential key external.
5. Import the legacy `devices.json` once, preserve it untouched for rollback, and provide explicit
   device import/export commands instead of retaining a second runtime store.
6. Serialize short SQLite write admission inside `music-storage`, keep a small WAL read pool, and
   hold an exclusive standard-library lock file so only one server/offline writer owns the data.
7. Start from the durable library index and run full filesystem reconciliation after core readiness.
   Library and mode coordinators own app-managed mutations, generations, staged files, journals, and
   immutable catalog publication.
8. Persist each new durable job's lane, schema/policy versions, restart policy, and per-claim
   execution identity. Retain the two actual lanes and bounded executors rather than add a broker or
   generic scheduler.
9. Try voice inference in one dedicated Rust model-owning thread. Use the same interface in a
   supervised Rust subprocess only when feasibility, memory, cancellation, or native-runtime tests
   prove process isolation necessary.
10. Preserve `/api/health` for compatibility, add component readiness/degradation, and map unexpected
    HTTP/WebSocket failures to safe codes plus correlation IDs instead of exposing internal errors.
11. Forbid unsafe code in project crates, deny uncontrolled panics in production paths, bound every
    queue/buffer/executor, and keep panic unwinding so supervisors can initiate controlled shutdown.

## Options considered

### Keep the first Rust blueprint unchanged

| Dimension | Assessment |
|---|---|
| Complexity | Medium |
| Rewrite speed | Fastest |
| Correctness boundaries | Medium |
| Python-era coupling retained | High |

This avoids another design pass, but `music-server` would own too much orchestration, device state
would remain split without its original reason, startup would retain a scalability cliff, and the
mandatory model process would be chosen before Rust measurements exist.

### Refined modular monolith with explicit owners

| Dimension | Assessment |
|---|---|
| Complexity | Medium-high initially |
| Rewrite speed | Slightly slower foundation |
| Correctness boundaries | High |
| Operational burden | Low |

This is selected. It adds compile-time/application boundaries and small runtime coordinators where
there is real mutable ownership, while retaining one deployable server and direct in-process calls.

### Event-sourced or service-oriented redesign

| Dimension | Assessment |
|---|---|
| Complexity | Very high |
| Rewrite speed | Slow |
| Fault isolation | High in theory |
| Fit for one instance/operator | Poor |

Event logs, brokers, services, and distributed storage would add replay, versioning, networking,
deployment, and consistency problems without a scale or availability requirement that needs them.

## Trade-off analysis

The selected design deliberately spends more effort on the foundation to reduce long-term coupling.
The application layer and wire/domain translation create some duplicate structures, but prevent
legacy compatibility fields from becoming internal invariants. Coordinators serialize rare writes,
but make ownership and recovery visible. Moving devices into SQLite makes database backup quality
more important, which is acceptable because migrations and verified backups replace destructive
database resets.

An in-process voice thread is simpler and should use less memory than a worker process, but it cannot
hard-kill a wedged inference call. That is why it is a feasibility result, not an article of faith;
the Rust subprocess remains the required fallback when bounded shutdown cannot be proven.

## Consequences

- The workspace grows from six to eight crates, but feature code no longer accumulates in the Axum
  composition crate.
- Routes become thin and most use cases can be tested below HTTP with real temporary SQLite and
  focused fake external adapters.
- The critical startup path becomes faster and reports degraded components instead of hiding them.
- Slow WebSocket clients cannot stall state publication or other outputs.
- Runtime backups have one fewer mutable side file; legacy device JSON remains a migration input and
  rollback artifact only.
- SQLite writes are predictably serialized in-process, matching SQLite's one-writer reality.
- Optional voice execution uses the cheapest safe boundary proven by measurements.
- Several intentional compatibility differences require owner approval and fixture updates before
  Phase 1 can close.

## Action items

1. [x] Project owner accepts this ADR and the five explicit differences in the
   [architecture reassessment](RUST_ARCHITECTURE_REASSESSMENT.md).
2. [x] Phase 1 captures current startup, error, device-store, job, protocol, and model behavior as
   executable reference evidence.
3. [x] Phase 1 proves SQLite import/lock/write-gate and in-process voice feasibility on copied data.
   Storage, graph loading, and end-to-end inference are proven; private differential output and the
   long resource soak remain optional post-cutover diagnostics.
4. [x] Phase 2 creates the eight-crate skeleton and supervised runtime before feature porting.
5. [x] The compatibility ledger records every accepted difference and its frontend/operator effect.
