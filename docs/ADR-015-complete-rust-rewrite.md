# ADR-015: Replace the Python application with a Rust modular monolith

**Status:** Accepted

**Date:** 2026-08-27

**Decider:** Project owner

## Context

Music is a personal, single-operator service. Availability during development is not critical, and
the project owner accepts a deliberate final cutover rather than requiring an always-live
incremental migration. The existing Python application is nevertheless a substantial product: it
contains about 28,000 lines of backend code and 17,000 lines of backend tests, exposes 145 HTTP and
WebSocket routes, accepts 34 WebSocket actions, owns 18 SQLite tables, runs durable local and
provider jobs, manipulates a media filesystem, and implements security-sensitive provider and
credential boundaries.

The rewrite is intended to improve the architecture and its correctness and resource guarantees,
not mechanically transliterate FastAPI and SQLAlchemy. The React/TypeScript frontend is healthy,
Baton is a separate Kotlin repository, SQLite contains valuable authored state, FFmpeg provides
broad codec support, and mpv provides reliable appliance playback. Replacing those parts would add
risk without advancing the purpose of the rewrite.

The current application remains usable while the rewrite is developed. The final system must not
retain a Python runtime or a permanent Python sidecar merely to finish a difficult feature.

## Decision

Rewrite every project-owned Python component in Rust on the long-lived `rewrite/rust` branch:

- the HTTP and WebSocket server;
- canonical playback state and timing;
- auth, sessions, configuration, storage, and diagnostics;
- library indexing, metadata editing, uploads, cleanup, modes, presets, playlists, and authoring;
- durable jobs, local Assistant engines, provider adapters, quality gates, and credential tooling;
- local signal/context analysis and optional voice inference; and
- the CLI and headless output appliance.

Keep the application a **modular monolith**. One server process owns one playback universe and one
in-process state actor. CPU-heavy signal analysis uses a bounded dedicated worker pool. Optional
voice inference uses one supervised worker process so the model is loaded once and can be reclaimed
or terminated independently. Provider calls remain bounded asynchronous I/O inside the server.
SQLite remains the authoritative database and existing data is migrated in place through explicit,
versioned, additive-first migrations.

Keep these components rather than rewrite them:

- the React/TypeScript frontend, with generated contract types where practical;
- Baton/Kotlin and its existing protocol model;
- FFmpeg/ffprobe as the initial codec and technical-probe boundary;
- SQLite as the storage engine;
- YAML mode documents as the human-owned format; and
- mpv as the headless appliance's playback engine, controlled through local JSON IPC.

The Python application remains present on `rewrite/rust` as a reference oracle until every
required compatibility gate passes. It is then removed in one explicit cleanup phase. Production
does not run a hybrid Python/Rust request path.

## Branch and release decision

- `main` remains the deployable Python version during the rewrite and should be feature-frozen.
- Urgent fixes made on `main` must be entered in the rewrite compatibility ledger and ported before
  the affected Rust phase can close.
- All rewrite work lands as logical commits on `rewrite/rust`; nothing is pushed or deployed
  without explicit authorization.
- Before cutover, preserve the final Python commit with a dated tag and a `legacy/python` branch.
- If `main` stayed frozen, move it to the completed rewrite with a fast-forward merge. Otherwise use
  a reviewed merge; never force-push or rewrite the Python history.
- Keep database changes backward-compatible through the initial Rust release so rollback can use
  the Python image and the pre-cutover backup.

## Options considered

### Continue optimizing Python

The current code can be improved further and already delegates expensive operations to NumPy,
FFmpeg, Essentia, and TensorFlow. This is the lowest-effort option, but it retains process-pool
complexity, duplicated native model memory, weaker compiler guarantees, and the split between the
runtime used for orchestration and the native work it coordinates.

### Rewrite in Go

Go would reach feature parity faster and would be easier to maintain manually. Its HTTP,
concurrency, testing, and deployment story is excellent. It was rejected for this rewrite because
the most difficult project-specific work is audio/DSP/model execution under a fixed memory budget,
and because compile-time ownership, exhaustive enums, explicit error handling, and data-race
prevention are more valuable than implementation speed under the owner's stated constraints.

### Gradually replace isolated Python extensions with Rust

This would reduce migration risk, but it would preserve Python packaging, FFI boundaries, and two
implementation languages indefinitely. It conflicts with the explicit preference for a complete
replacement once a language is selected.

### Split the rewrite into services

Separate playback, library, analysis, and provider services would isolate failure domains, but
would add network contracts, deployment units, distributed coordination, and operational work to a
single-user application. The only process boundary retained is the one justified by expensive,
optional model inference.

## Consequences

- The rewrite takes longer than a Go implementation and requires more compiler-guided iteration.
- The final backend has one language, one build graph, explicit domain types, bounded concurrency,
  and no garbage-collected application heap.
- Existing API, WebSocket, filesystem, database, and encrypted-credential behavior must be captured
  as executable compatibility fixtures before the Python implementation is removed.
- A language change invalidates implementation-bound Assistant fingerprints. Saved credentials and
  configuration remain, but provider verification, model conformance, and quality certification
  are deliberately reset and must be rerun.
- Local signal measurements receive a new analyzer identity if the Rust implementation changes
  numeric output. Existing rows are retained as history but are stale for current consumers.
- Rust does not prevent inefficient cloning, unbounded work, incorrect SQL, flawed business logic,
  or security mistakes. Workspace lints, architectural boundaries, differential tests, property
  tests, corpus benchmarks, and review remain mandatory.

## Follow-up documents

- [Rust rewrite architecture](RUST_REWRITE_ARCHITECTURE.md)
- [Rust rewrite execution plan](RUST_REWRITE_PLAN.md)
