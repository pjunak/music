# Rust rewrite architecture

**Status:** Proposed for implementation review

**Owning decision:** [ADR-015](ADR-015-complete-rust-rewrite.md)
**Target branch:** `rewrite/rust`

This is the target architecture, not a map of Python files to Rust files. Existing behavior is an
important compatibility oracle, but the new module boundaries follow data ownership, side effects,
and failure domains.

## Requirements and constraints

### Functional scope

The Rust version must replace all project-owned Python behavior, including the server, CLI, local
analysis, provider-backed Assistant workflows, and headless output appliance. Unless a change is
explicitly accepted, the existing React frontend, compatibility client, external-output protocol,
and Baton must continue working without coordinated deployment.

### Non-functional requirements

- One self-hosted, single-operator application; no multi-tenancy or distributed deployment.
- One canonical playback universe and one authoritative state mutation funnel.
- Existing `app.db`, media roots, modes, provider credentials, and remembered devices survive the
  cutover.
- Production remains within the existing three-CPU and 4 GB container contract.
- Local and provider-generated work remains bounded, cancellable where possible, durable where
  promised, and review-only where the current contract requires it.
- Raw media, credentials, private paths, and model output keep their existing disclosure and
  trust boundaries.
- Empty or partially configured installations boot coherently.
- The server must stop gracefully, leave jobs in a recoverable state, and fail closed when a
  canonical owner or migration fails.

### Explicit non-goals

- Rewriting React in Rust/WASM.
- Changing Baton or inventing a new playback protocol.
- Replacing FFmpeg codecs, SQLite, YAML, or mpv merely to remove non-Rust dependencies.
- Microservices, a message broker, PostgreSQL, Kubernetes, or multi-instance server operation.
- New product features during parity work.
- Destructive schema cleanup before the Rust release has proved rollback-safe.

## High-level design

```text
                      browser / compat client / Baton / output appliance
                                      |
                         HTTP, WebSocket, media ranges
                                      |
                              +-------v-------+
                              |  music-server |
                              | Axum + Tower  |
                              +---+---+---+---+
                                  |   |   |
                 typed commands --+   |   +-- application services
                                      |          | library / modes / authoring
                              +-------v-------+  | jobs / Assistant / providers
                              | playback actor|  |
                              | sole state    |  |
                              | owner         |  |
                              +---+-------+---+  |
                                  |       |      |
                         watch snapshots  |      |
                         transient events |      |
                                  |       |      |
                           WebSocket tasks |      |
                                          |      |
                               +----------v------v--+
                               | music-storage      |
                               | SQLx + SQLite WAL  |
                               +----------+---------+
                                          |
                   +----------------------+----------------------+
                   |                                             |
          +--------v---------+                          +--------v---------+
          | signal pool      |                          | voice worker     |
          | bounded Rust DSP |                          | one model/process|
          | + FFmpeg process |                          | supervised IPC   |
          +------------------+                          +------------------+
```

This is a modular monolith. The diagram shows ownership and worker boundaries, not independently
deployed services.

## Workspace and dependency boundaries

```text
Cargo.toml
crates/
  music-domain/       Pure domain models, typed IDs, reducers, rules, analysis documents
  music-protocol/     Public HTTP/WS DTOs, tagged messages, schemas, TS export
  music-storage/      SQLx migrations, rows, repositories, transactions
  music-analysis/     Streaming signal pipeline, DSP, voice backend interface
  music-server/       Axum composition, application services, actors, jobs, provider transport
  music-output/       Rust headless appliance using the shared protocol and mpv JSON IPC
frontend/             Existing React application
clients/              Protocol documentation and appliance packaging
modes/                Existing seed documents
```

Dependency direction is one-way:

```text
music-domain <- music-protocol
music-domain <- music-storage
music-domain <- music-analysis
all four     <- music-server
music-protocol <- music-output
```

`music-domain` contains no database, HTTP framework, filesystem, process, or async-runtime types.
Application services depend on repository and adapter traits defined at the boundary they consume;
adapters implement them in `music-storage`, `music-analysis`, or `music-server`.

Six crates are enough to enforce the important boundaries without creating a crate per feature.
Feature modules remain ordinary Rust modules inside `music-server` until a demonstrated dependency
or compilation boundary justifies extraction.

## Selected foundation

| Concern | Decision | Reason |
|---|---|---|
| HTTP and WebSocket | Axum on Tokio | Small, explicit handler model and Tower middleware; no second runtime abstraction. |
| Persistence | SQLx with bundled SQLite | Explicit SQL, async integration, migrations, and compile-time checked static queries. |
| Serialization | Serde and `serde_json` | Canonical Rust ecosystem and exact tagged-enum control. |
| REST documentation | `utoipa`/`utoipa-axum` | OpenAPI is generated from registered handlers and DTOs. |
| TypeScript contract export | `ts-rs` | HTTP/WS types are generated from the same Serde types and committed for frontend use. |
| Task JSON Schema | Schemars plus local validation | Draft 2020-12 schemas originate from the same strict Rust result types. |
| HTTP client | Reqwest with Rustls | Supports explicit redirect, proxy, retry, timeout, and DNS overrides needed by provider policy. |
| Audio metadata | Lofty, subject to corpus parity | Reads and writes the formats currently handled by Mutagen; isolated behind a tag adapter. |
| Decoding/probing | FFmpeg and ffprobe subprocesses | Preserves the existing broad format support, including formats pure-Rust decoders do not cover. |
| DSP | RustFFT plus reusable buffers | Native SIMD-capable FFT without materializing Python lists or NumPy arrays. |
| Loudness | `ebur128` from the decoded stream | Standards-tested implementation; removes a second whole-file FFmpeg measurement pass after parity. |
| Voice inference | `tract` candidate behind `VoiceBackend` | Supports the current legacy TF1 frozen graph, but exact preprocessing and outputs require a gate. |
| YAML | Typed adapter; candidate selected by corpus/security gate | Avoid deprecated `serde_yaml`/`serde_yml`; keep parser exposure small and input bounded. |
| Headless playback | mpv subprocess JSON IPC | Keeps proven playback while avoiding an unsafe libmpv FFI layer in project code. |
| Logging | `tracing` and `tracing-subscriber` | Structured request, action, worker, and job context without secret-bearing string assembly. |
| CLI | Clap | One typed command tree for administration, diagnostics, migrations, and evaluation. |

Versions are pinned in `Cargo.lock` only after the feasibility gates. Axum is built directly on
Tokio/Hyper and Tower ([Axum documentation](https://docs.rs/axum/latest/axum/)); SQLx provides a
first-class SQLite driver and migrations ([SQLx SQLite documentation](https://docs.rs/sqlx/latest/sqlx/sqlite/));
Lofty supports reading and writing the current common tag formats
([Lofty documentation](https://docs.rs/lofty/latest/lofty/)); and tract currently documents legacy
TensorFlow frozen-graph loading ([tract documentation](https://github.com/sonos/tract/blob/main/README.md)).
These are feasibility inputs, not substitutes for tests with this project's files and model.

## Canonical playback model

### Pure reducer

The heart of playback is a deterministic function:

```text
reduce(current_state, resolved_command, clock_value, random_choice)
    -> { next_state, domain_events }
```

It performs no SQL, network, filesystem, logging, or timer work. Time and random selection are
inputs so pause/resume, seeking, shuffle, queue advancement, interrupts, and restart pruning are
fully deterministic in tests.

The domain uses explicit enums for loop mode, shuffle mode, crossfade type, commands, and events.
IDs use checked newtypes over fixed-width integers or bounded strings. Illegal combinations should
be unrepresentable where that does not distort the wire contract.

### State actor

One supervised Tokio task owns mutable `PlayerState`, live connection membership, and timer
registrations. A bounded command mailbox provides backpressure. Every mutating HTTP or WebSocket
path performs any required read-side resolution, sends a typed command, and waits for a typed
result.

For an accepted mutation, the actor:

1. reduces the command against the current state;
2. persists the new materialized state;
3. publishes the newest immutable snapshot through a Tokio `watch` channel; and
4. emits transient events or timer changes after persistence.

If persistence fails, the actor keeps the prior state and returns an error. If the actor dies, the
server terminates instead of serving a second, unsupervised truth.

State watchers retain only the newest full snapshot. Skipping intermediate snapshots is safe
because clients reconcile rather than replay deltas. SFX and loop ticks use a separate bounded
broadcast channel because they are transient events.

### Clock representation

The live actor uses a monotonic clock for elapsed playback. Persisted and wire DTOs materialize a
millisecond position and retain compatible anchor fields as needed by protocol fixtures. Startup
freezes elapsed clocks, clears connected and active outputs, stops loops, prunes missing resources,
and never advances through downtime.

### WebSocket sessions

Each connection has its own send task, timeout, guest/auth state, protocol version, and stable
client identity after registration. Slow or failed sends disconnect that socket without blocking
the actor. Guest and legacy projections are produced per connection from the same snapshot.

Registration and disconnect are actor commands. Multiple sockets may share one stable client ID;
disconnecting one socket removes live output membership only after the last sibling connection has
closed. Long-lived sessions are rechecked before privileged mutations and downgrade to guest on
expiry or revocation.

## HTTP and protocol contracts

- Preserve route paths, methods, status codes, cookie behavior, range streaming, and JSON field
  names unless a deliberate compatibility change is approved.
- `music-protocol` owns the WebSocket tagged unions and public state DTO. The server and Rust output
  client compile against the same crate.
- Rust types derive Serde and TypeScript bindings. Generated TypeScript is committed; CI regenerates
  it and fails on drift.
- Axum routes are registered with their OpenAPI definitions. A checked-in semantic OpenAPI baseline
  detects missing operations, parameters, response statuses, and incompatible schema changes.
- Request rejection uses a single `ApiError` envelope compatible with the frontend's `detail`
  handling. Validation errors remain 422; auth remains 401/403 as currently contracted.
- WebSocket protocol v1 legacy volume projection and v2 absolute per-device volume remain until a
  separately coordinated protocol decision changes them.
- Runtime validation remains in the old-TV compatibility client. Generated static types do not
  replace checks at an untrusted WebSocket boundary.

## Persistence and migration

### SQLite

SQLite stays in WAL mode with foreign keys enabled and a bounded busy timeout. Use a small SQLx
pool for concurrent reads; writes remain short. Cross-resource library mutations use a dedicated
`LibraryMutationGate` so a disk operation and its index transaction cannot race a full scan.

SQL is explicit. Repositories return domain objects rather than exposing SQLx rows outside
`music-storage`. Static queries use SQLx checked macros with committed offline metadata; dynamic
search/filter queries use a bounded query builder with values always bound.

### Migration bootstrap

The existing database has no general migration ledger. The first Rust migration therefore:

1. opens the database read-write only after an automatic backup succeeds;
2. inspects tables, columns, indexes, foreign keys, and SQLite version;
3. refuses unknown or incompatible shapes with a precise `music-cli db doctor` report;
4. creates the SQLx migration ledger and any missing structures with idempotent statements; and
5. records the compatibility baseline only after validation succeeds.

Initial migrations are additive. Tables/columns used by Python are not dropped or renamed during
the rollback window. A pre-cutover database copy is mandatory even though migrations are designed
to be backward-compatible.

### Compatibility-sensitive data

- Existing Argon2 password hashes are verified directly by the Rust Argon2 implementation.
- Provider credentials retain AES-256-GCM, 12-byte nonces, URL-safe base64, and the exact
  `assistant-provider-credential/v1:<connection_id>` associated data. Golden cross-language tests
  prove decrypt/encrypt compatibility before cutover.
- Existing JSON state and job documents are read through tolerant persisted DTOs, normalized, and
  written through strict current DTOs.
- Unknown historical job kinds remain listable rather than making job history unreadable.
- `devices.json` remains an atomic, separately backed-up operator file because its survival across
  a database reset is intentional.
- Mode YAML remains operator-authored. Reads are typed and bounded; writes use stage, fsync, atomic
  rename, and a recoverable journal for multi-file authoring commits.

### Intentional invalidation

Implementation-bound Assistant fingerprints cannot truthfully survive a language rewrite. The
cutover migration preserves connection, credential, and role configuration but resets connection
verification, role conformance, and current model-quality certifications. Historical results stay
available. The operator must verify and certify the Rust execution path explicitly.

Rust signal analysis uses a new analyzer identity when output differs beyond the defined parity
tolerance. Existing analysis rows remain untouched but stale; the durable job builds new rows.

## Filesystem and media architecture

All stored library and SFX paths use a validated POSIX-relative `LibraryPath`/`SfxPath` newtype.
Absolute paths, prefixes, empty forbidden components, `.`/`..`, NUL, and platform prefixes are
rejected before filesystem access. Existing targets and creation parents are canonicalized and
verified beneath their configured roots. Property and fuzz tests cover Windows and POSIX forms,
Unicode, separators, symlinks, and rename races.

Uploads stream to a uniquely named temporary file in the destination directory, enforce file-count
and byte limits while streaming, flush and sync, then atomically rename according to the operator's
`rename`/`overwrite`/`skip` decision. No request body or whole media file is buffered in memory.

Media streaming implements single-range and normal full responses with the same headers the current
clients require. Range, conditional request, missing-file, traversal, content-type, and disconnect
behavior receive integration tests with generated media fixtures.

Lofty is hidden behind `TagReader`/`TagWriter`. Before selection it must round-trip the project's
declarative tag registry across generated MP3, FLAC, Ogg/Opus, M4A/AAC, WAV, and supported edge
cases. Writes operate on a temporary copy and replace the original only after a successful reread.
Unsupported or lossy formats return per-track partial failures rather than corrupting a batch.

## Modes, playlists, authoring, and cleanup

- Modes are loaded into an immutable `ModeCatalog` snapshot and swapped atomically after a complete
  successful parse. A bad reload leaves the last good snapshot active.
- Preset and mode writes serialize through one mode-mutation gate. Active preset content changes
  notify the playback actor to increment `preset_revision`.
- Playlist ordering and automatic-playlist materialization live in pure domain services and commit
  in one SQLite transaction.
- Authoring remains source -> pure preview -> explicit selection -> atomic commit. The commit stages
  every output and records a recovery journal before replacing target files or database rows.
- Library cleanup remains detect -> review -> journal -> execute -> optional revert. Detection has
  no write-capable dependency.

## Durable jobs

`JobKind` is a closed enum for current jobs, while historical unknown strings remain representable.
Each current handler has typed parameters and results serialized into the compatible JSON columns.

A `JobCoordinator` supervises two bounded lanes:

- **local lane:** one library-mutating or whole-library job at a time;
- **provider lane:** one provider-cost-bearing job at a time.

The coordinator claims rows transactionally, owns cancellation tokens, and checkpoints progress and
safe partial results. Restartable handlers must be idempotent or checkpointed. Non-restartable
provider work is marked interrupted after an uncertain shutdown and is never repeated silently.
Changing or deleting provider configuration remains blocked while a dependent provider job is
active.

## Local audio/context analysis

Signal analysis is streaming and allocation-bounded:

```text
ffprobe technical facts
          |
FFmpeg decoded PCM stream (one process per active track)
          |
reused frame/ring buffers
    |              |
EBU R128        downmix/resample to 16 kHz mono
                   |
        RustFFT + temporal accumulators
                   |
         bounded context document
```

The target is one decode pass for signal context and loudness. The first parity implementation may
temporarily retain a separate FFmpeg loudness probe until `ebur128` agrees on the representative
corpus; that adapter is removed before the Python runtime is removed.

Independent tracks run on a dedicated bounded CPU pool. FFmpeg is constrained to one thread per
track. Results return to the coordinator, which alone writes SQLite checkpoints. Concurrency is
configuration-bounded and benchmarked under the three-CPU cgroup rather than inferred from host CPU
count. Cancellation is cooperative in Rust loops and actively terminates the owned FFmpeg child.

The Rust analyzer is calibrated against the synthetic probes that defined `local-context/v2` and a
private representative corpus. Numeric tolerances are field-specific; no semantic tags are added.
If measurement definitions change, use `local-context/v3`, retain per-field reliability, and make
old rows stale explicitly.

RustFFT supplies the SIMD-capable transform path
([RustFFT documentation](https://docs.rs/rustfft/latest/rustfft/)); the `ebur128` crate documents
the EBU conformance tests it passes ([ebur128 documentation](https://docs.rs/ebur128/latest/ebur128/)).

## Voice inference

Voice analysis remains optional, local-only, checksum-pinned, non-fatal, and independently
versioned. `music-analysis` defines a narrow `VoiceBackend`; the first candidate uses tract to load
the current frozen TensorFlow graph. The model weights remain separately operator-supplied under
their existing CC BY-NC-SA terms and are never downloaded during build or startup. Replacing the
Essentia runtime does not change the model's license or permit copying implementation code from an
incompatible source.

The model is owned by exactly one supervised `music-analysis-worker` process. The parent sends
versioned, length-prefixed requests and receives bounded results; worker logs go only to stderr.
The worker processes tracks sequentially, can batch model windows internally, exposes model/runtime
identity without paths, and exits after a configured work or memory threshold. Cancellation or a
deadline kills and replaces the worker, so native/model code cannot wedge server shutdown.

Before the main port depends on it, a feasibility gate must reproduce Essentia's preprocessing,
window ordering, output tensor, normalized score, and coverage on the exact checksum-pinned model.
If tract cannot run the graph accurately, evaluate a maintained Rust binding to a native inference
runtime behind the same process boundary. There is no Python fallback. Cutover either passes the
voice gate or receives an explicit owner decision to ship the optional stage as unavailable.

## Assistant and provider boundary

The existing algorithm-first and review-first contracts remain architectural requirements:

```text
local evidence and deterministic rules
                -> bounded provider document
                -> exact generated schema
                -> strict local validation and identity checks
                -> locally reconstructed review draft
                -> explicit operator commit
```

Provider handlers remain transport-free and selected only by versioned adapter ID. The shared
Reqwest/Rustls transport:

- permits only reviewed HTTP(S) schemes and configured ports;
- resolves every destination, validates all addresses, and pins the approved addresses while
  retaining hostname/SNI verification;
- rejects private, loopback, link-local, multicast, unspecified, and reserved destinations unless
  the explicit private-network setting permits them;
- disables redirects, environment/system proxies, cookies, automatic retries, and referer behavior;
- applies connect, whole-request, and response-body deadlines;
- streams and bounds response bytes before JSON parsing; and
- returns stable, secret-free error codes without upstream bodies.

Reqwest exposes explicit redirect policy and DNS overrides needed for this design
([Reqwest client builder](https://docs.rs/reqwest/latest/reqwest/struct.ClientBuilder.html)).
Provider conformance, task quality, disclosure, fingerprint invalidation, usage checkpoints, and
non-restartability retain their present meaning.

## Security rules

- Workspace crates forbid `unsafe` code. If a future unavoidable FFI adapter is approved, it lives
  in one small crate with a documented safety contract and dedicated tests.
- Application crates deny Clippy's `unwrap_used`, `expect_used`, and panic lints outside tests and
  explicitly documented process-fatal startup invariants.
- Secrets use `secrecy`/zeroization-aware wrappers and never implement ordinary display or debug.
- Session tokens use OS randomness and database lookup; cookies keep current secure attributes.
- Password verification uses Argon2 and a dummy hash path for unknown users.
- File key initialization uses exclusive, no-follow creation and validates Unix permissions.
- Security headers, CORS, upload limits, body limits, login throttles, WebSocket auth downgrade, and
  guest projections are middleware or typed extractors rather than handler conventions.
- Dependency sources, advisories, duplicates, licenses, and banned crates are checked in CI. The
  deprecated/unsound `serde_yml` line is explicitly banned; RustSec records its unsoundness
  ([RUSTSEC-2025-0068](https://rustsec.org/advisories/RUSTSEC-2025-0068)).

## Error handling, cancellation, and shutdown

Domain errors are typed and mapped to stable public codes at the HTTP/WS edge. Internal error
chains are logged with correlation IDs; responses do not expose secrets, SQL, absolute paths, or
provider bodies. Partial batch results remain per item.

Every long operation declares:

- its owner and concurrency limit;
- its cancellation mechanism;
- its deadline behavior;
- whether it is restartable;
- its safe checkpoint; and
- what happens when its external side effect succeeds but persistence fails.

Shutdown stops admission, closes WebSocket sessions, requests job cancellation/checkpointing,
terminates analysis children, persists final actor state, and closes SQLite. A deadline then exits
non-zero so the container supervisor can restart cleanly; it does not wait forever for native work.

## Observability and performance

Use structured tracing fields for request ID, connection ID, stable client ID only where safe,
action type, job ID/kind/lane, track ID, analyzer ID, stage, elapsed time, and outcome. Never log
tokens, provider payloads, credentials, absolute private paths, or model raw output.

Diagnostics expose build revision, schema version, FFmpeg/ffprobe presence, voice readiness,
connection counts, job-lane state, last scan, and bounded stage timings. A full metrics stack is not
required for one personal instance.

Performance is accepted against measured baselines, not language expectations:

- API and WebSocket latency must not regress materially under the representative local load.
- Full signal-context output must stay within field tolerances and be no slower than the Python
  baseline; target at least 25% lower elapsed time or peak memory on the representative corpus.
- The production analysis run must remain below the 4 GB cgroup limit with three CPUs; target a
  15% memory safety margin.
- Voice inference loads one model instance and completes repeated-corpus soak tests without upward
  unbounded resident-memory growth.
- Idle memory, startup scan, upload, range streaming, and image size are recorded before/after even
  where no hard improvement is required.

## Deployment shape

The production image remains a multi-stage build:

1. Node builds the unchanged frontend.
2. A pinned stable Rust toolchain builds locked release binaries.
3. A slim Debian runtime receives `music-server`, `music-cli`, the static frontend, seed modes,
   CA certificates, FFmpeg/ffprobe, and no compiler or Python runtime.

The server runs as the current non-root UID, serves the SPA and API from one origin, uses the same
mounts and environment names where possible, and exposes the same health endpoint. `music-cli`
provides the container healthcheck so the runtime does not need curl or Python.

## Decisions to revisit only with evidence

- Replace FFmpeg decoding with Symphonia only after every supported format, metadata behavior, and
  performance characteristic passes the corpus gate.
- Run voice inference in-process only after repeat-run memory and cancellation measurements prove
  the process boundary unnecessary.
- Add more analysis workers only after cgroup CPU/RSS measurements show headroom.
- Split a crate or service only when a real dependency, fault-isolation, or deployment boundary
  appears.
- Change SQLite or the wire protocol only through a separate ADR and coordinated compatibility
  work.
