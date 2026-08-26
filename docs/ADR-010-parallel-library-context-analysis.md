# ADR-010: Parallel library context analysis

**Status:** Accepted

**Date:** 2026-08-25

**Deciders:** Project maintainer

**Implementation update (2026-08-27):** The first live three-worker run with voice inference
reached the 4 GB cgroup ceiling and a native worker terminated with a general-protection fault. The
kernel did not invoke the OOM killer, but `memory.events` recorded repeated `max` events. Readiness
and source-signature checks had retained an unnecessary Essentia/TensorFlow copy in the FastAPI
parent before the three worker copies loaded. Runtime preflight now executes in a disposable
interpreter so only analysis workers retain TensorFlow. The three-worker/4 GB target requires a
live rerun and peak-memory verification after this correction.

## Context

Whole-track context analysis performs sample-by-sample accumulation, repeated spectral transforms,
local voice inference, and FFmpeg loudness measurement. The durable job previously analyzed only
one track at a time. Moving that handler to a background thread protected FastAPI's event loop but
did not provide multi-core execution for its CPU-heavy Python code. The production server has four
CPU cores, while Music must leave capacity for playback, the reverse proxy, monitoring, and other
containers. Context rows must still be checkpointed independently so cancellation or restart does
not repeat completed work, and concurrent SQLite writers must be avoided.

## Decision

Analyze independent tracks in a bounded, spawn-based process pool. The configurable
`ASSISTANT_LIBRARY_CONTEXT_WORKERS` setting accepts one through four workers and defaults to one for
portable installations. The production deployment selects three workers, three CPUs, and a 4 GB
container memory limit.

Only audio decoding and context computation run in child processes. The existing FastAPI process
remains the sole owner of job progress, SQLite writes, failure checkpoints, and the canonical
playback state. At most one task per worker is queued. Completed results return to the parent in
completion order and are committed one track at a time.

Use the multiprocessing `spawn` context rather than forking the already-threaded web process.
Workers receive a shared cancellation event; decoding and FFmpeg loudness work poll it, and the
parent stops queued work before waiting for the pool to close. The Uvicorn server must remain a
single worker because its playback and connection state is process-local.

## Options considered

### Ordinary Python threads

Rejected for analysis parallelism. The Python 3.12 runtime's global interpreter lock prevents the
pure-Python sample and FFT stages from executing bytecode on several cores. Threads remain useful
for keeping blocking work off the event loop, but they are not the multi-core boundary here.

### Multiple Uvicorn workers

Rejected. It would duplicate the process-local state machine, device registry, connection manager,
and track advancer, splitting one playback universe into incompatible server processes.

### Rewrite the analyzer before adding concurrency

Deferred. NumPy/native transforms, fewer full-file decoding passes, and a redesigned
`local-context/v2` can reduce per-track cost further, but they may change measured output and need
separate calibration and versioning. Per-track process parallelism preserves the existing analysis
document and storage contract.

### Free-threaded Python

Deferred. It would require a different interpreter build plus validation of every native extension,
including the optional Essentia/TensorFlow runtime. Process isolation works with the current pinned
runtime and keeps extension failures contained to a worker result.

## Consequences

- Three long tracks can use three CPU cores concurrently in production while short SQLite writes
  remain serialized in the parent.
- Completion order is intentionally nondeterministic, but each track's analysis document remains
  deterministic and independently checkpointed.
- Each process may load its own Essentia/TensorFlow runtime and model cache, increasing peak memory.
  Worker count and container memory therefore remain explicit deployment choices.
- Runtime readiness is probed in a disposable interpreter. The parent FastAPI process does not
  retain a redundant TensorFlow copy before the analysis workers start.
- Cancellation during native voice inference can wait for that call to return; streaming decode and
  FFmpeg loudness work stop cooperatively. A future isolated inference service may tighten this
  boundary if real-library measurements show it is needed.
- Process startup has overhead, so a one-track job continues down the in-process path.
