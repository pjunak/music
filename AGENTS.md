# Music

Self-hosted Rust + React music player and tabletop-session orchestrator.
The server owns canonical playback, library, authentication, and device state.
Browsers, headless appliances, and the optional Baton Android client consume it.

## Read by task

Read only the relevant owner and section before changing a subsystem. Follow
this table even when the task begins at the repository root or edits tests.

| Task or touched area | Owner/reference |
|---|---|
| Setup, product use, configuration | [README](README.md), [configuration defaults](.env.example) |
| Commands, development environment, completion checks | [Validation matrix](docs/VALIDATION.md); checked-in toolchains and workflows own exact versions |
| Architecture and crate boundaries | [Rust architecture](docs/RUST_REWRITE_ARCHITECTURE.md) |
| Playback, auth, library, devices, authoring, effects, UI or persistence | Matching section of [engineering contracts](docs/ENGINEERING.md) |
| HTTP/WS or external output behavior | [Client protocol](clients/README.md), `crates/music-protocol`, `crates/music-application/src/playback/`; coordinate affected consumers |
| Assistant/harness, providers, quality, tags, consent, credentials or review | [Assistant map](docs/ASSISTANT_ARCHITECTURE.md): relevant ownership table and detailed task subsection, across Rust crates and frontend |
| Operator AI configuration or acceptance | [Operator guide](ASSISTANT.md), [dated acceptance evidence](docs/AI_ACCEPTANCE.md) |
| Job fairness and timing | [Job diagnostics](docs/JOB_DIAGNOSTICS.md) |
| Feature CSS and cascade | [Style ownership](frontend/src/styles/README.md) |
| Other maintained references or historical decisions | [Documentation index](docs/README.md) |

## Ownership

```text
crates/music-domain/       pure domain rules
crates/music-application/  actors, coordinators, jobs, Assistant use cases
crates/music-storage/      SQLx persistence, migrations, crypto and recovery
crates/music-media/        rooted files, metadata, YAML and delivery
crates/music-analysis/     local DSP and optional voice inference
crates/music-protocol/     edge DTOs and generated bindings
crates/music-server/       Axum routes, composition, CLI and transport
crates/music-output/       native headless mpv appliance
frontend/src/core/         validated API/WS clients, stores and audio
frontend/src/views/        feature UI and review workflows
clients/                   external output and authoring contracts
```

## Invariants

- `PlaybackHandle` owns playback mutations: reduce, persist, then publish.
  HTTP and WebSocket actions use the same owner. Clients reconcile snapshots;
  reconnect never replays stale mutations. Stable client IDs can span tabs.
- Wire changes update typed schemas, guards, tests, external-client docs and
  affected Baton models together. Inspect sibling source only for an affected
  contract; this repository remains usable without a sibling checkout.
- Authentication, WebSocket Origin checks and path containment stay at their
  existing server boundaries. Preserve guest read/output behavior and keep
  authoring mutations authenticated. Never substitute path-prefix checks.
- The bounded library coordinator owns filesystem/index changes and recovery.
  Scanning is read-only; selected writes use reviewed plans and transactions.
- Output activation, default-on designation and canonical per-device volume
  are separate. Preserve multi-tab disconnect safety and output-client behavior.
- Model input/output is untrusted and task-bounded. Local validation and
  reconstruction remain authoritative; suggestions require operator review.
  Verification, conformance, quality and live-data consent are separate gates.
- Providers and models are swappable through the server UI; no provider choice
  is permanent. Use Astra when selected for harness engineering, with explicit
  task contracts and provider-neutral tests. Keep adapter-specific behavior
  behind declared capabilities and revalidate a changed role configuration.
- Checkpoint provider attempts before external cost; never automatically repeat
  an uncertain paid request. Preserve the separately held credential master key.
- Durable jobs declare restartability and bounded cancellation. Preserve
  transaction/recovery ordering and never substitute development data for live
  data. Detailed persistence rules are in the engineering reference.
- Production rollout belongs to `pjunak/infra`, target `music`. This repo
  verifies/publishes its image and waits for the exact main-only infrastructure
  run to deploy the immutable digest and verify health. It never SSHes to
  production. The [deployment guide](README.md#automated-deployment) and infra's
  [shared contract](https://github.com/pjunak/infra/blob/main/docs/application-deployments.md)
  own token scope, pinned client use and **Deploy published release** retries.
  `INFRA_SERVICE` is unused here; headless output and Baton installation remain
  separate from server rollout.

## Completion

Select final checks from [the validation matrix](docs/VALIDATION.md). Prose-only
changes need diff/link/claim verification; runtime and shared-contract changes
need their applicable regression and repository gates. Report checks actually
run and any outstanding provider, Docker, physical-audio or manual acceptance.
Keep relevant generated-contract, architecture and documentation checks intact.

Update the owning reference when its contract changes; avoid copying detailed
rules back into this entrypoint. Keep this file comfortably below the project
instruction budget, leaving room for narrower guidance.

Create logical local commits after applicable validation and preserve unrelated
changes. Never push, deploy, or release unless explicitly requested. Do not
commit media, databases, secrets, transient builds or local tool configuration.
