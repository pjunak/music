# ADR-010: Parallel library context analysis

**Status:** Accepted

**Date:** 2026-08-25

**Deciders:** Project maintainer

**Implementation update (2026-08-27):** The first live three-worker run with voice inference
reached the 4 GB cgroup ceiling and a native worker terminated with a general-protection fault. The
kernel did not invoke the OOM killer, but `memory.events` recorded repeated `max` events. Readiness
and source-signature checks had retained an unnecessary Essentia/TensorFlow copy in the FastAPI
parent before the three worker copies loaded. Runtime preflight now executes in a disposable
interpreter so only analysis workers retain TensorFlow. A second live run still reached the ceiling
after 14 tracks, indicating retained native allocations inside the persistent workers. Voice-enabled
analysis workers were then configured to recycle after at most four tracks, but a later live run
still terminated during early voice processing. The job now uses sequential resource phases: an
audio-context pool processes and checkpoints the full library without importing the voice model,
then closes before a voice-only pool starts. Voice workers still recycle after four tracks. A native
failure in that second pool leaves the completed audio pass intact and retryable. The
three-worker/4 GB target still requires live peak-memory verification with this staged design.

## Context

Whole-track context analysis performs sample-by-sample accumulation, repeated spectral transforms,
local voice inference, and FFmpeg loudness measurement. Running those stages in the same persistent
workers made their memory lifetimes overlap and allowed native voice allocations to accumulate on
top of signal-analysis state. The durable job previously analyzed only one track at a time. Moving
that handler to a background thread protected FastAPI's event loop but
did not provide multi-core execution for its CPU-heavy Python code. The production server has four
CPU cores, while Music must leave capacity for playback, the reverse proxy, monitoring, and other
containers. Context rows must still be checkpointed independently so cancellation or restart does
not repeat completed work, and concurrent SQLite writers must be avoided.

## Decision

Analyze independent tracks in two sequential bounded, spawn-based process pools. The configurable
`ASSISTANT_LIBRARY_CONTEXT_WORKERS` setting accepts one through four workers and defaults to one for
portable installations. The production deployment selects three workers, three CPUs, and a 4 GB
container memory limit.

The first pool performs audio decoding, signal context, structure, tempo, and loudness without
loading the optional voice runtime. Each successful result is committed as a partial context row.
After all eligible tracks finish, that pool closes and a fresh voice-only pool processes the saved
rows. Each voice result patches its row and promotes it to full. The existing FastAPI process
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
- Signal analysis and native voice inference no longer coexist in the same worker processes. The
  first pool exits before TensorFlow-backed workers are created.
- The UI and durable job result expose separate audio-context and voice-detection progress. A voice
  pool failure leaves partial rows behind, and retry resumes the second pass without repeating the
  first, including after a forced rebuild.
- Completion order is intentionally nondeterministic, but each track's analysis document remains
  deterministic and independently checkpointed.
- Each process may load its own Essentia/TensorFlow runtime and model cache, increasing peak memory.
  Worker count and container memory therefore remain explicit deployment choices.
- Runtime readiness is probed in a disposable interpreter. The parent FastAPI process does not
  retain a redundant TensorFlow copy before the analysis workers start.
- Voice-only workers recycle after at most four tracks. This adds bounded process/model startup
  overhead but prevents native TensorFlow allocations from accumulating across the whole library.
- Cancellation during native voice inference can wait for that call to return; streaming decode and
  FFmpeg loudness work stop cooperatively. A future isolated inference service may tighten this
  boundary if real-library measurements show it is needed.
- Process startup has overhead. A one-track signal pass remains in-process, while enabled voice
  inference still uses a child process so the web process never retains the native runtime.
