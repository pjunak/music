# Rust rewrite architecture

**Status:** Accepted for implementation

**Owning decisions:** [ADR-015](ADR-015-complete-rust-rewrite.md),
[ADR-016](ADR-016-rust-native-runtime-boundaries.md)
**Target branch:** `rewrite/rust`

This is the target architecture, not a map of Python files to Rust files. Existing behavior is an
important compatibility oracle, but the new module boundaries follow data ownership, side effects,
and failure domains.

The [architecture reassessment](RUST_ARCHITECTURE_REASSESSMENT.md) records which current boundaries
are enduring product intent and which are Python-era implementation compromises.

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
                              | Axum / wire adapter |
                              +----------+----------+
                                         |
                              +----------v----------+
                              | music-application   |
                              | commands / queries  |
                              +----+--------+-------+
                                   |        |
                         +---------v--+  +--v-------------------+
                         | playback  |  | library / mode / job |
                         | actor     |  | coordinators          |
                         +----+---+--+  +--+----------+---------+
                              |   |        |          |
                    watch state   |        |          |
                    transient     |        |          |
                              |   |        |          |
                      per-socket  |   +----v----+ +---v----------+
                      projection  |   | storage | | media /      |
                                  |   | write   | | analysis     |
                                  |   | gate    | | adapters     |
                                  |   +----+----+ +------+-------+
                                  |        |             |
                                  +--------v-------------v--+
                                           SQLite / files /
                                      FFmpeg / optional model
```

This is a modular monolith. The diagram shows ownership and worker boundaries, not independently
deployed services. `AppRuntime` creates these owners once, injects their handles, tracks their
tasks, and coordinates shutdown. No mutable module-global service state is permitted.

## Workspace and dependency boundaries

```text
Cargo.toml
crates/
  music-domain/       Pure domain models, typed IDs, reducers, rules, analysis documents
  music-application/  Commands, queries, use cases, actors, coordinator ports
  music-protocol/     Wire-only HTTP/WS DTOs, tagged messages, schemas, TS export
  music-storage/      SQLx migrations, rows, repositories, transactions
  music-media/        Safe paths, metadata, streaming, staged mutation, FFmpeg adapters
  music-analysis/     Streaming signal pipeline, DSP, voice backend interface
  music-server/       Axum/provider adapters, composition, server and CLI binaries
  music-output/       Rust headless appliance using the shared protocol and mpv JSON IPC
frontend/             Existing React application
clients/              Protocol documentation and appliance packaging
modes/                Existing seed documents
```

Dependency direction is one-way:

```text
music-application -> music-domain
music-storage     -> music-application + music-domain
music-media       -> music-application + music-domain
music-analysis    -> music-application + music-domain
music-server      -> all internal crates
music-output      -> music-protocol
```

`music-domain` contains no database, HTTP framework, filesystem, process, or async-runtime types.
`music-protocol` is also independent of the domain: explicit edge translations stop legacy and
compatibility fields from becoming internal invariants. `music-application` defines coarse traits
only at real external boundaries. Adapters implement them in `music-storage`, `music-media`,
`music-analysis`, or `music-server`; there is no repository trait per table and no generic DI
framework.

Eight crates enforce the important boundaries without creating a crate per feature. Feature modules
remain ordinary Rust modules inside `music-application`; concrete transport and composition stay in
`music-server` until a demonstrated dependency or deployment boundary justifies extraction.

## Selected foundation

| Concern | Decision | Reason |
|---|---|---|
| HTTP and WebSocket | Axum on Tokio | Small, explicit handler model and Tower middleware; no second runtime abstraction. |
| Runtime supervision | Tokio Util `CancellationToken` and `TaskTracker` | One cancellation tree and observable shutdown instead of detached tasks. |
| Persistence | SQLx with bundled SQLite | Explicit SQL, async integration, migrations, and compile-time checked static queries. |
| Serialization | Serde and `serde_json` | Canonical Rust ecosystem and exact tagged-enum control. |
| REST documentation | `utoipa`/`utoipa-axum` | OpenAPI is generated from registered handlers and DTOs. |
| TypeScript contract export | `ts-rs` | HTTP/WS types are generated from the same Serde types and committed for frontend use. |
| Task JSON Schema | Schemars plus local validation | Draft 2020-12 schemas originate from the same strict Rust result types. |
| HTTP client | Reqwest with Rustls | Supports explicit redirect, proxy, retry, timeout, and DNS overrides needed by provider policy. |
| Audio metadata | Lofty for its native formats; FFmpeg/ffprobe adapter for ASF/WMA | Keeps the common path in-process and typed without pretending Lofty supports WMA; both remain isolated behind one tag adapter. |
| Decoding/probing | FFmpeg and ffprobe subprocesses | Preserves the existing broad format support, including formats pure-Rust decoders do not cover. |
| DSP | RustFFT plus reusable buffers | Native SIMD-capable FFT without materializing Python lists or NumPy arrays. |
| CPU execution | Dedicated fixed Rayon pool, subject to profiling | The application's CPU budget is explicit and separate from Tokio's general blocking pool. |
| Loudness | `ebur128` from the decoded stream | Standards-tested implementation; removes a second whole-file FFmpeg measurement pass after parity. |
| Voice inference | `tract` candidate behind `VoiceBackend` | Try one model-owning Rust thread; select a Rust subprocess only if the feasibility gate proves isolation necessary. |
| YAML | Typed adapter; candidate selected by corpus/security gate | Avoid deprecated `serde_yaml`/`serde_yml`; keep parser exposure small and input bounded. |
| Headless playback | mpv subprocess JSON IPC | Keeps proven playback while avoiding an unsafe libmpv FFI layer in project code. |
| Logging | `tracing` and `tracing-subscriber` | Structured request, action, worker, and job context without secret-bearing string assembly. |
| CLI | Clap | One typed command tree for administration, diagnostics, migrations, and evaluation. |
| Single-instance ownership | Standard-library file lock beside `app.db` | Prevents two servers or an offline writer from creating competing canonical owners. |

Versions are pinned in `Cargo.lock` only after the feasibility gates. Axum is built directly on
Tokio/Hyper and Tower ([Axum documentation](https://docs.rs/axum/latest/axum/)); SQLx provides a
first-class SQLite driver and migrations ([SQLx SQLite documentation](https://docs.rs/sqlx/latest/sqlx/sqlite/));
Lofty supports reading and writing the current common tag formats, but not ASF/WMA
([Lofty documentation](https://docs.rs/lofty/latest/lofty/)); and tract currently documents legacy
TensorFlow frozen-graph loading ([tract documentation](https://github.com/sonos/tract/blob/main/README.md)).
Tokio documents `CancellationToken` plus `TaskTracker` for graceful task shutdown
([TaskTracker documentation](https://docs.rs/tokio-util/latest/tokio_util/task/task_tracker/struct.TaskTracker.html)),
and the standard library has supported cross-platform file locks since Rust 1.89
([`File::try_lock`](https://doc.rust-lang.org/stable/std/fs/struct.File.html#method.try_lock)). These
are feasibility inputs, not substitutes for tests with this project's files and model.

The implemented process shell loads the current environment names and optional working-directory
`.env` exactly once, with process values taking precedence. Required paths, SQLite-only storage,
origins, cookies, limits, and worker counts fail validation before mutable owners start. Optional
provider credentials use a zeroizing, redacted wrapper and are never included in error values.
Axum currently targets 0.8.9, Tokio Util 0.7.19, Tower HTTP 0.7.0, and tracing-subscriber 0.3.23 in
`Cargo.lock`; upgrades remain ordinary reviewed dependency changes rather than floating CI inputs.

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

One supervised Tokio task owns mutable `PlayerState`, live connection membership, catalog
generations, and timer registrations. A bounded command mailbox provides backpressure. Every
mutating HTTP or WebSocket path performs any required read-side resolution, sends a closed typed
command, and waits for a typed result. HTTP/WS handlers cannot supply arbitrary callbacks or obtain
a mutable state reference.

Commands that refer to tracks, playlists, modes, soundboards, or presets carry the catalog
generation used during resolution. A committed library or mode mutation sends the actor a typed
catalog-change command. The actor rejects stale resolved commands and prunes invalidated references
before publishing the next revision.

WebSocket actions follow the same rule as HTTP orchestration: mode and soundboard selection,
preset activation, direct track and queue changes, recursive folder playback, and interrupt
takeovers are resolved against immutable catalog views before admission. Queue requests validate
the complete set before changing state, and selecting an output validates only newly added IDs
against the actor-owned live connection set. Compatibility errors therefore do not leave partial
playback or device state behind.

For an accepted durable mutation, the actor:

1. reduces the command against the current state;
2. persists the new materialized state with a revision compare-and-swap;
3. publishes the newest immutable snapshot through a Tokio `watch` channel; and
4. emits transient events or timer changes after persistence.

If persistence fails, the actor keeps the prior state and returns an error. If the actor dies, the
server terminates instead of serving a second, unsupervised truth.

Presence-only changes are explicitly ephemeral: they publish a new in-process revision but are not
serialized into the durable playback DTO. Disconnect cleanup that changes the durable active-output
set still follows the normal persisted path. Position reports update live state immediately and use
the documented throttled flush policy, with a final best-effort flush during graceful shutdown.

The actor distinguishes `publication_revision` from the internal `storage_revision` used for SQL
compare-and-swap. Wire `PlayerState.revision` maps to publication ordering and may reset from the
last durable baseline after restart; storage concurrency never depends on an ephemeral connection
change having been written.

State watchers retain only the newest full snapshot. Skipping intermediate snapshots is safe
because clients reconcile rather than replay deltas. SFX and loop ticks use a separate bounded
broadcast channel because they are transient events.

Track-end and looping-SFX effects are registrations owned by the actor, not detached tasks. Before
each receive, the actor derives the earliest deadline and sleeps only until that deadline; any
command or catalog publication wakes the receive loop and causes a fresh plan. Known track
durations and follow successors come from a compact immutable library projection ordered by path.
Ambient tracks without a duration remain client-advance-only, while an unknown-duration interrupt
uses a five-minute fallback so it cannot wedge ambient playback indefinitely. The server waits
750 ms beyond a known duration, allowing a healthy client's lower-latency `ended` message to win.
Both paths carry the observed track ID into the reducer, so only one racing advance can commit.
Client-requested next-track actions enter through a dedicated actor message. The actor resolves a
follow successor from its own current state and compact path-ordered catalog, then reduces it in the
same mailbox turn; a WebSocket snapshot can never select a successor for an already-replaced track.

Loop starts replace and restart the deadline for the same loop ID. A due tick emits one transient
SFX event and schedules the next interval from the current monotonic sample, deliberately
coalescing missed ticks instead of producing a burst after actor or host suspension. Loop records
and their deadlines are removed together by stop, catalog pruning, shutdown, and restart
normalization.

Cue firing is one resolved actor command rather than a transport-side sequence. Resolution binds
the authored cue to the active mode ID and the exact library/mode generations, chooses the oldest
same-name playlist inside that mode deterministically, and validates every preset, track, and
soundboard item before admission. The reducer applies the preset override, playlist and initial
position, replacement cue loops, and transient SFX as one candidate. It persists and publishes the
single durable state before emitting any transient; a failed compare-and-swap therefore cannot
half-fire a cue. Cue loop IDs reserve `cue:<cue-id>:<index>`, so re-firing also removes stale loops
left by an older version of the same cue while leaving manual and other-cue loops untouched.

### Clock representation

The live actor uses a monotonic clock for elapsed playback. Persisted and wire DTOs materialize a
millisecond position and retain compatible anchor fields as needed by protocol fixtures. Startup
freezes elapsed clocks, clears connected and active outputs, stops loops, prunes missing resources,
and never advances through downtime.

### WebSocket sessions

Each connection has its own send task, timeout, guest/auth state, protocol version, and stable
client identity after registration. It subscribes to internal latest-state and transient-event
channels, builds its own guest/legacy wire projection, and never gives the actor a WebSocket
handle. Slow or failed sends disconnect only that socket without blocking publication or another
output.

Latest state may coalesce because clients reconcile complete snapshots. Transient SFX/cue events
have bounded fan-out and are never replayed late; a receiver that falls behind records diagnostics
and drops/disconnects according to the fixture-defined policy.

Registration and disconnect are actor commands. Multiple sockets may share one stable client ID;
disconnecting one socket removes live output membership only after the last sibling connection has
closed. Long-lived sessions are rechecked before privileged mutations and downgrade to guest on
expiry or revocation.

## HTTP and protocol contracts

- Preserve route paths, methods, status codes, cookie behavior, range streaming, and JSON field
  names unless a deliberate compatibility change is approved.
- `music-protocol` owns the WebSocket tagged unions and public state DTO. The server and Rust output
  client compile against the same crate. Wire DTOs are translated explicitly and do not double as
  internal domain objects.
- Rust types derive Serde and TypeScript bindings. Generated TypeScript is committed; CI regenerates
  it and fails on drift. Warnings for unsupported Serde attributes fail the export gate rather than
  being hidden.
- Axum routes are registered with their OpenAPI definitions. A checked-in semantic OpenAPI baseline
  detects missing operations, parameters, response statuses, and incompatible schema changes.
- Request rejection uses a single `ApiError` envelope compatible with the frontend's `detail`
  handling. Validation errors remain 422; auth remains 401/403 as currently contracted.
- Unexpected failures retain a safe `detail`, stable code, and correlation ID. Internal exception
  chains, SQL, provider bodies, secrets, and absolute paths are log-only; raw exception responses
  are an intentional compatibility improvement requiring owner acceptance.
- WebSocket protocol v1 legacy volume projection and v2 absolute per-device volume remain until a
  separately coordinated protocol decision changes them.
- Runtime validation remains in the old-TV compatibility client. Generated static types do not
  replace checks at an untrusted WebSocket boundary.

## Persistence and migration

### SQLite

SQLite stays in WAL mode with foreign keys enabled and a bounded busy timeout. Use a small SQLx
pool for concurrent reads. `music-storage` owns one asynchronous write-admission gate because WAL
still has one writer; every write transaction remains short and the gate is never held across
filesystem, network, provider, or model work. Cross-resource mutations go through typed library or
mode coordinators rather than unrelated locks.

Before opening the database for server writes, the process takes an exclusive standard-library
file lock beside `app.db` and holds the file handle for its lifetime. Offline mutating CLI commands
take the same lock. Lock contention is a clear startup/CLI error, not a second best-effort owner.

SQL is explicit. Repositories return domain objects rather than exposing SQLx rows outside
`music-storage`. Static queries use SQLx checked macros with committed offline metadata; dynamic
search/filter queries use a bounded query builder with values always bound.

The playback snapshot remains one cohesive versioned JSON aggregate, but its `storage_revision` is
also an explicit column used for compare-and-swap persistence. A mismatched revision indicates an
unexpected second/offline writer and moves the server out of readiness instead of overwriting it.

Backups acquire a maintenance admission gate, create a SQLite-consistent snapshot in a temporary
workspace, verify it, add modes and a bounded manifest, and stream the archive from disk. They do
not buffer the database/archive in process memory and never include the credential master key;
the manifest records only its one-way fingerprint for pairing checks.

### Migration bootstrap

The existing database has no general migration ledger. The first Rust migration therefore:

1. opens the database read-write only after an automatic backup succeeds;
2. inspects tables, columns, indexes, foreign keys, and SQLite version;
3. refuses unknown or incompatible shapes with a precise `music-cli db doctor` report;
4. creates the SQLx migration ledger and any missing structures, including remembered-device and
   recovery-journal tables, with idempotent statements;
5. imports legacy `devices.json` only when the target table is empty and records its fingerprint;
   and
6. records the compatibility baseline only after validation succeeds.

Initial migrations are additive. Tables/columns used by Python are not dropped or renamed during
the rollback window. A pre-cutover database copy is mandatory even though migrations are designed
to be backward-compatible.

Baseline v1 implements that bootstrap without first touching the source database: the doctor opens
the existing file read-only, runs SQLite quick/foreign-key checks, and compares tables, columns,
unique/check constraints, indexes, and foreign keys against the frozen Python schema plus explicit
Rust additions. Only the documented Python additive columns and Rust v1 objects may be absent;
unknown tables/columns or damaged constraints fail closed. For a compatible existing file,
`VACUUM INTO` creates a consistent sibling snapshot that is reopened read-only, verified, fsynced,
SHA-256 hashed, and paired with a non-secret JSON manifest before the first read-write pool opens.
The migration then records SQLx schema version 1, adds the internal playback storage revision, and
creates `remembered_devices`, `legacy_device_imports`, and `recovery_journal`. Missing databases are
created directly because there is no prior state to back up. `music-cli db doctor` is read-only;
`music-cli db migrate` takes the same lifetime lock as the server and follows this identical path.

### Compatibility-sensitive data

- Existing Argon2 password hashes are verified directly by the Rust Argon2 implementation.
- Provider credentials retain AES-256-GCM, 12-byte nonces, URL-safe base64, and the exact
  `assistant-provider-credential/v1:<connection_id>` associated data. Golden cross-language tests
  prove decrypt/encrypt compatibility before cutover.
- Existing JSON state and job documents are read through tolerant persisted DTOs, normalized, and
  written through strict current DTOs.
- Unknown historical job kinds remain listable rather than making job history unreadable.
- Remembered devices become SQLite-owned operational records. Legacy `devices.json` is a one-time
  migration input and rollback artifact; `music-cli devices export/import` provides explicit
  recovery without retaining a second mutable runtime store.
- Mode YAML remains operator-authored. Reads are typed and bounded; writes use stage, fsync, atomic
  rename, and a recoverable journal for multi-file authoring commits.

### Intentional invalidation

Implementation-bound Assistant fingerprints cannot truthfully survive a language rewrite. The
cutover migration preserves connection, credential, and role configuration but resets connection
verification, role conformance, and current model-quality certifications. Historical results stay
available. The operator must verify and certify the Rust execution path explicitly.

Rust signal analysis uses a new analyzer identity when output differs beyond the defined parity
tolerance. Existing analysis rows remain untouched but stale; the durable job builds new rows.

## Startup and component health

Startup is a supervised sequence, not one best-effort callback:

1. validate immutable configuration and acquire the instance lock;
2. inspect, back up when required, and migrate SQLite;
3. load the valid mode catalog and normalize the persisted playback snapshot;
4. start the playback, library, mode, job, and analysis owners;
5. bind HTTP/WebSocket and expose readiness; and
6. enqueue full library reconciliation and degradable capability probes.

The server validates files referenced by live playback synchronously but does not walk the complete
media tree before serving. It starts from the durable index and reports library status as
`reconciling`, `current`, or `failed`. A failed scan preserves the last index and remains visible and
retryable.

`/api/health` keeps its compatibility liveness response. A component readiness surface distinguishes
critical failure from degradation. Database/migration, instance-lock, or playback-owner failure
makes the service not ready and initiates controlled shutdown. Missing FFmpeg, unavailable optional
voice inference, malformed individual modes, or a failed reconciliation is degraded-but-usable so
the operator can enter the UI and repair it. Public health responses expose only coarse status;
component errors, versions, and timings remain behind authenticated diagnostics and never include
paths or secrets.

The compatibility `/api/diagnostics` projection reads counts and timestamps from the library
coordinator's immutable status and connection/revision data from a playback-actor snapshot. It does
not query mutable module globals or create another status owner. Until `ModeCoordinator` exists,
its mode-loader fields remain the contract's explicit not-yet-loaded value: no timestamp, no loaded
IDs, and no fabricated errors.

During the rewrite, readiness remains `starting` until the real playback owner is running. The
Phase-2 WebSocket transport shell returns a protocol-shaped availability error and closes instead
of publishing a synthetic snapshot. Static files are optional and degradable: a valid built SPA
uses no-cache entry/client routes and immutable content-hashed assets, while `/api/*` has its own
JSON 404 boundary and can never fall through to `index.html`.

## Filesystem and media architecture

All stored library and SFX paths use a validated POSIX-relative `LibraryPath`/`SfxPath` newtype.
Absolute paths, prefixes, empty forbidden components, `.`/`..`, NUL, and platform prefixes are
rejected before filesystem access. Existing targets and creation parents are canonicalized and
verified beneath their configured roots. Property and fuzz tests cover Windows and POSIX forms,
Unicode, separators, symlinks, and rename races.

`LibraryCoordinator` is the only owner of app-managed file/index mutations. A mutation is planned
and journaled durably before its rooted filesystem effect, then its index change and terminal
journal state are committed together in a short database transaction. Startup replays unfinished
domain operations before publishing the catalog. Folder renames rewrite indexed paths without
reissuing track IDs; metadata reconciliation follows the commit. Track moves refresh file-backed
and filename/folder-derived metadata, then commit the new path and metadata without changing the
track ID. Bulk track mutations keep per-item journals and publish one final catalog snapshot. The
singleton library state maintains the exact catalog cardinality, so individual commits update the
generation and count in constant time instead of rescanning every track; a versioned migration
backfills that invariant for existing Python and early Rust databases. The shared journal
infrastructure does not pretend SQLite and a media volume are one atomic filesystem.

`SfxCoordinator` is the corresponding single writer for the separate SFX root. It serializes
inventory reads with folder, file, and upload mutations; records each effect in the shared recovery
journal before touching the filesystem; and replays unfinished effects before HTTP traffic starts.
The SFX catalog is intentionally filesystem-owned rather than duplicated in SQLite. Mode catalog
publications carry a compact soundboard-to-exact-item-path map, which the playback actor uses both
to reject stale fires/loop starts and to prune loops after a soundboard item edit. Playback file
delivery checks the full immutable mode catalog before resolving the typed `SfxPath`; management
remains authenticated and may operate on unreferenced files.

Full scans discover and read metadata outside the mutation coordinator. Their result carries the
library generation; final diff application is short and is rejected/reconciled when a committed
mutation changed that generation. External filesystem edits remain eventually reconciled and media
serving always verifies the current file.

Uploads stream to uniquely named sibling staging files, enforce file-count and per-file byte limits
while streaming, then flush and sync before transferring ownership to `LibraryCoordinator`. The
coordinator serializes `rename`/`overwrite`/`skip` resolution with other managed mutations and
journals the selected destination before publishing it. Create-only publication uses a no-clobber
hard link and verifies file identity when replaying the link-created crash window; replacement uses
a deterministic journal-owned backup and completes forward on startup. Skips discard their staged
file, unsupported media is stored without entering the audio catalog, and a batch publishes one
final catalog snapshot. Multipart extraction disables Axum's small default body cap only on this
route; the global request cap includes bounded framing overhead and streaming limits remain
authoritative. No request body or whole media file is buffered in memory.

Media streaming implements single-range and normal full responses with the same headers the current
clients require. Range, conditional request, missing-file, traversal, content-type, and disconnect
behavior receive integration tests with generated media fixtures. Tower's file service is an
implementation candidate, not assumed parity; its current range behavior is wrapped or replaced
where the fixture corpus differs.

Lofty and the ASF subprocess boundary are hidden behind one metadata adapter. Concrete Lofty tag
types are used for Vorbis, Opus, FLAC, and MP4 so unrelated fields, artwork, and MP4 integer atoms
survive updates. WMA tags are read through bounded `ffprobe` JSON and changed by an FFmpeg
stream-copy remux; compressed-audio SHA-256, codec identity, compatible ASF duration, and intended
fields are verified before the staged result can be committed. Raw ADTS AAC uses Lofty for tag data
and FFprobe for duration because the corpus disproved Lofty's duration estimate; it remains readable
and streamable but metadata-read-only because neither the old Mutagen path nor the chosen safe path
can write it reliably. All writes operate on a deterministic journal-owned sibling, reread and
verify the staged media, rename the source to a backup, and publish the staged file before the
database transaction commits the refreshed metadata and journal. Startup completes any interrupted
replacement forward. WAV/AIFF container lengths are normalized after Lofty writes so repeated ID3
updates remain authoritative without losing trailing preservation bytes. Unsupported or lossy
formats return per-track partial failures rather than corrupting a batch; DB-only fields may still
commit for that item, while any ambiguous post-replacement failure stops the batch for recovery.

## Modes, playlists, authoring, and cleanup

- `ModeCoordinator` loads modes into an immutable versioned `ModeCatalog` snapshot and publishes it
  only after a complete successful parse. A bad reload leaves the last good snapshot active and
  exposes bounded per-document diagnostics.
- Preset and mode writes are staged and serialized through that coordinator. Committed changes send
  a typed catalog-change command to playback; active preset content increments `preset_revision`,
  and removed resources are pruned in the same actor revision.
- Playlist ordering and automatic-playlist materialization live in pure domain services and commit
  in one SQLite transaction.
- Authoring remains source -> pure preview -> explicit selection -> atomic commit. The commit stages
  every output and records a recovery journal before replacing target files or database rows.
- Library cleanup remains detect -> review -> journal -> execute -> optional revert. Detection has
  no write-capable dependency. Optional name verification is a separate operator-triggered adapter:
  it paces identifiable MusicBrainz searches to the public limit, bounds deadlines and response
  bodies, and may write only successful score verdicts to the dedicated cache. Lookup failures are
  not cached, so a later explicit attempt can retry them
  ([MusicBrainz rate-limit policy](https://musicbrainz.org/doc/MusicBrainz_API/Rate_Limiting)).
  Accepted cleanup operations return to `LibraryCoordinator`, so cleanup never becomes a second
  file/catalog writer. Each accepted item has a `cleanup` recovery journal before its rooted file
  effect; the refreshed catalog row or folder paths, compatible `cleanup_batches` append, and
  terminal journal state then commit in one SQLite transaction. Tag journals retain the actual
  pre-write file value rather than a filename-derived display fallback. A process restart replays
  unfinished cleanup effects forward before publishing the catalog. Batch and uploaded-journal
  reverts use the same serialized boundary, walk items in reverse order, resolve re-minted tracks
  by their journaled paths, and stale-check every inverse before touching disk. Each accepted
  inverse has its own recovery record. A batch revert additionally has a parent recovery record;
  marking the batch reverted and completing that parent share one SQLite transaction, so startup
  can safely finish an interrupted rollback without making cleanup a second file/catalog writer.

## Durable jobs

`JobKind` is a closed enum for current jobs, while historical unknown strings remain representable.
Each current handler has typed parameters and results serialized into the compatible JSON columns.
New rows also persist lane, parameter-schema version, restart policy, checkpoint version, and the
current per-claim `execution_id`; recovery does not infer historical policy solely from whichever
handler registry happens to be compiled today.

A `JobCoordinator` supervises two bounded lanes:

- **local lane:** one library-mutating or whole-library job at a time;
- **provider lane:** one provider-cost-bearing job at a time.

The coordinator claims rows transactionally, owns cancellation tokens, and checkpoints progress and
safe partial results. Checkpoint/final writes compare the current `execution_id`, so a late cancelled
attempt cannot overwrite a retry. Restartable handlers must be idempotent or checkpointed.
Non-restartable provider work is marked interrupted after an uncertain shutdown and is never
repeated silently. Changing or deleting provider configuration remains blocked while a dependent
provider job is active.

Handlers are async coordinators, not arbitrary blocking callbacks. CPU work enters the fixed
analysis pool, filesystem work enters bounded media workers, and provider calls use bounded async
I/O. The two actual lanes remain intentionally simpler than a generic scheduler or broker.

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

Independent tracks run on a dedicated fixed CPU pool rather than Tokio's general blocking pool.
FFmpeg is constrained to one thread per track. Results return to the coordinator, which alone writes
SQLite checkpoints. Concurrency is configuration-bounded and benchmarked under the three-CPU cgroup
rather than inferred from host CPU count. Cancellation is cooperative in Rust loops and actively
terminates the owned FFmpeg child.

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

The implemented candidate gives the model to exactly one dedicated, supervised Rust inference
thread. That thread creates and owns the model state, processes requests sequentially through a
capacity-one channel, runs overlapping windows without duplicating the graph, and exposes a
path-free model/runtime/preprocessor identity. FFmpeg decoding, 512-sample frames, 96-band features,
and 187-frame patches are streamed through fixed-size buffers. Cancellation is checked while
decoding and between patches. A panic closes the response channel, marks the worker dead, and makes
subsequent optional work unavailable without taking down the server.

The exact pinned graph is loaded directly as TF1 rather than converted to NNEF. Its checksum permits
a deliberately narrow in-memory importer compatibility layer: all four graph `Pad` constants are
validated as the expected fixed `(3, 3)` time-axis padding before replacement, and
`FusedBatchNormV3` is normalized to tract's compatible frozen-graph operator. No rewritten model is
persisted and no model is downloaded by the application.

Preprocessing independently implements the published MusiCNN parameters: mono 16 kHz audio,
centered 512-sample symmetric Hann frames with a 256-sample hop, a 257-bin magnitude spectrum,
96 Slaney-warped linear triangles with unit-triangle normalization and power accumulation,
`log10(energy * 10000 + 1)`, and 187-frame patches with a 93-frame hop. Silent padding is
deterministic zero rather than Essentia's default random low-level dither; this is an explicit
reproducibility hardening recorded in the compatibility ledger.

This thread boundary is conditional on evidence. If the selected backend contains unsafe native
code, leaks, wedges, cannot bound an inference call, or cannot meet shutdown deadlines, the same
`VoiceBackend` runs in a supervised Rust subprocess using versioned length-prefixed IPC. Process
isolation is therefore a tested fallback, not a Python-era default.

The Windows implementation gate verifies the official model checksum, exact graph output shape and
fixed zero-input output, end-to-end FFmpeg-to-worker inference, bounded score aggregation, frame and
patch counts, and cancellation-aware streaming. Final acceptance still requires a Linux runner to
compare representative preprocessing and outputs against Essentia and to record repeated-run RSS,
shutdown, cancellation, and panic behavior under the three-CPU/4 GB envelope. If that evidence
fails, the same interface moves to the documented Rust subprocess. There is no Python fallback.

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
- Release builds retain panic unwinding so task/thread supervisors can observe failure. Critical
  owner panics initiate controlled shutdown; they are never treated as normal request errors.
- Secrets use `secrecy`/zeroization-aware wrappers and never implement ordinary display or debug.
- Session tokens use OS randomness and database lookup; cookies keep current secure attributes.
- Password verification uses Argon2 and a dummy hash path for unknown users.
- File key initialization uses exclusive, no-follow creation and validates Unix permissions.
- Security headers, CORS, upload limits, body limits, login throttles, WebSocket auth downgrade, and
  guest projections are middleware or typed extractors rather than handler conventions.
- Dependency sources, advisories, duplicates, licenses, and banned crates are checked in CI. The
  deprecated/unsound `serde_yml` line is explicitly banned; RustSec records its unsoundness
  ([RUSTSEC-2025-0068](https://rustsec.org/advisories/RUSTSEC-2025-0068)).
- tract's unmodified `dyn-eq` 0.1.3 dependency is the sole MPL-2.0 exception. The exception is
  crate-and-version scoped, its file-level obligations do not change the license of project files,
  and binary distributions must retain the source/license notice recorded in
  [third-party notices](THIRD_PARTY_NOTICES.md).

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

`AppRuntime` owns one root cancellation token and tracks every long-lived task. Shutdown stops
admission, closes WebSocket sessions, requests job cancellation/checkpointing, terminates any
analysis children, persists final actor state, releases the database/instance lock, and closes
SQLite. A deadline then exits non-zero so the container supervisor can restart cleanly; it does not
wait forever for native work.

Task admission and tracker closure share one lock, so no owner can be detached across the shutdown
boundary. Every critical future runs behind an observed Tokio join handle: a typed failure,
premature return, or panic marks its component failed, records only a non-secret failure code,
cancels the root token, and makes the process exit non-zero after bounded cleanup. HTTP panics are
converted to a fixed safe response; request middleware replaces rather than trusts incoming
correlation IDs and returns the generated ID in the response header.

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
mounts and environment names where possible, and preserves the compatibility health endpoint while
adding component readiness. `DEVICES_FILE` becomes a migration/import setting rather than a mutable
runtime mount after cutover. `music-cli` provides the container healthcheck so the runtime does not
need curl or Python.

## Decisions to revisit only with evidence

- Replace FFmpeg decoding with Symphonia only after every supported format, metadata behavior, and
  performance characteristic passes the corpus gate.
- Promote voice inference to a Rust subprocess when repeat-run memory, panic, native-code,
  cancellation, or deadline measurements show the in-process thread is not safely bounded.
- Add more analysis workers only after cgroup CPU/RSS measurements show headroom.
- Add a filesystem watcher only as a reconciliation accelerator after bind-mount behavior is
  measured; it never replaces explicit scans or current-file checks.
- Split a crate or service only when a real dependency, fault-isolation, or deployment boundary
  appears.
- Change SQLite or the wire protocol only through a separate ADR and coordinated compatibility
  work.
