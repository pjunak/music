# ADR-001: Provider-neutral Assistant connections and model roles

**Status:** Accepted

**Date:** 2026-08-19
**Decider:** Project owner

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
- Saving a connection never verifies, enables, or invokes it. Verification is
  an explicit authenticated action with a bounded timeout, response-size cap,
  no redirects, safe error codes, and private-network destinations blocked
  unless the operator deliberately opts in.
- Role configuration remains inactive until its connection has verified. This
  slice stores configuration only; local analyzers and `local-planner/v2`
  remain the sole runtime engines.
- Provider calls run outside the FastAPI event loop. Future longer model work
  must use the durable background-job runner and the established review-first
  Authoring boundary.

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
- Losing or rotating the master key makes existing provider credentials
  unreadable; the operator must re-enter them. Key rotation tooling can be
  added before production use requires it.
- Encryption protects database copies and accidental disclosure, not a fully
  compromised running server that also has the master key.
- OpenAI-compatible verification is the first adapter, not a claim that all
  providers expose identical capabilities. Specialized audio providers will
  require their own adapter and verification contract.
