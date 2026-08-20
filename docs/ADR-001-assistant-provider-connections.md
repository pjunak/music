# ADR-001: Provider-neutral Assistant connections and model roles

**Status:** Accepted

**Date:** 2026-08-19
**Decider:** Project owner

**Implementation follow-up (2026-08-20):** Playlist planning, music tagging,
manual-tag cleanup, and EQ drafting now execute through dedicated quality-gated,
consent-bound feature contracts. Credential audit and offline atomic key rotation
are also implemented. The connection/role boundary below remains authoritative.

## Context

The Assistant needs optional access to user-chosen model providers without
binding authoring behavior to one company or letting a model mutate playlists,
tags, cleanup plans, or EQ presets directly. API keys must remain usable for
future calls, so password hashing is not an option. Provider failures and slow
network calls must not block playback or weaken the local fallback.

## Decision

- Store provider **connections** separately from task **roles**. A connection
  owns an adapter type, base URL, and encrypted credential. Roles such as music
  tagging or playlist planning select a connection and model independently.
- Encrypt credentials with AES-256-GCM using `cryptography`. A random nonce is
  generated for every write and the connection ID is authenticated as
  associated data. The deployment supplies a URL-safe base64 32-byte master key
  through `ASSISTANT_CREDENTIAL_KEY`; it is never stored in the database or
  returned by the API.
- Ship one initial adapter, `openai-compatible/v1`, behind a registry boundary.
  Supporting a provider means implementing an adapter, not adding provider
  fields to playlist or tag code.
- Give adapters versioned transport capabilities and roles explicit capability
  requirements. Verification persists the capabilities actually confirmed by
  the adapter. Role configuration, conformance testing, enablement, and
  execution fail closed when those requirements are not satisfied; provider
  and model names are never capability evidence.
- Saving a connection never verifies, enables, or invokes it. Verification is
  an explicit authenticated action with a bounded timeout, response-size cap,
  no redirects, safe error codes, and private-network destinations blocked
  unless the operator deliberately opts in.
- Report credential presence explicitly and return only a masked hint, never the
  credential. The operator may remove the credential without deleting the
  connection or its role assignments. Removal or replacement resets connection
  verification and every dependent model gate; configured roles remain stored
  but cannot execute until a newly saved credential passes those gates again.
- Role configuration remains inactive until its connection has verified. Local
  analyzers and `local-planner/v2` remain the defaults; implemented model roles
  execute only through their dedicated disclosure, quality, and review contracts.
- A reserved role is not a usable feature flag. Roles remain unavailable for
  configuration until their feature-specific transport, quality evaluation,
  disclosure, consent, and review/commit boundaries are implemented.
- Provider calls run outside the FastAPI event loop. Long-running model work
  uses the durable background-job runner and the established review-first
  Authoring or generated-tag review boundary.

## Options considered

### Plaintext keys in SQLite

Low implementation cost, but database backups, diagnostics, or accidental
queries would expose reusable credentials. Rejected.

### Hash provider keys

Appropriate for authenticating incoming tokens, but impossible for credentials
that must later be sent to an upstream provider. Rejected.

### One environment variable per role/provider

Keeps secrets outside SQLite but tightly couples deployment configuration to
every future role and prevents the requested in-app setup and verification.
Useful for fully managed deployments, but not the primary interface.

### Encrypted connection records plus role assignments

Adds one deployment master key and key-loss recovery requirements, but keeps
provider details isolated, supports multiple services, and lets one verified
connection serve several roles. Selected.

## Consequences

- A database backup contains only encrypted provider keys. Restoring configured
  connections also requires the same deployment master key.
- Losing the master key makes existing provider credentials unreadable; the
  operator must re-enter them. Planned rotation uses the offline, atomic
  `music-cli assistant-credentials rotate` workflow and resets every dependent
  verification and quality gate.
- Encryption protects database copies and accidental disclosure, not a fully
  compromised running server that also has the master key.
- OpenAI-compatible verification is the first adapter, not a claim that all
  providers expose identical capabilities. Specialized audio providers will
  require their own adapter and verification contract.
- Capability IDs are versioned contracts. Adding one requires enforcement and
  regression tests at every setup and execution boundary, not merely a new UI
  label.
