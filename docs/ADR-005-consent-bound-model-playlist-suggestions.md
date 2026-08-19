# ADR-005: Consent-bound live model playlist suggestions

**Status:** Accepted

**Date:** 2026-08-19
**Decider:** Project owner

## Context

The local playlist planner already produces reviewable drafts, and an optional
configured model can pass conformance plus a fingerprint-bound synthetic quality
suite. A quality pass alone does not authorize disclosing real library metadata.
Live model requests may also take time, cost money, fail after the browser is
closed, or be interrupted after the provider received a request.

The model must not gain a second write path around Authoring import or receive
more library information than the hybrid planner needs.

## Decision

- Keep `local-planner/v2` as the default and preserve the synchronous local
  suggestion endpoint.
- Expose model readiness and the exact, versioned disclosure through a dedicated
  authenticated status endpoint. The disclosure enumerates sent and withheld
  data, the 100-candidate ceiling, and possible provider cost.
- Require `consent: true` and the exact current disclosure version in every model
  job request. Persist both in the job parameters as the authorization audit
  trail; saving a connection, enabling a role, or passing quality is not consent.
- Require the current `playlist-quality-v1` certification for the exact runtime
  fingerprint both before enqueue and before returning a result. Reject changed
  or stale role configurations.
- Reuse the hybrid planner: eligibility, exclusions, filtering, and candidate
  limits remain local; provider input contains no filesystem paths and model
  output contains IDs only. Reconstruct the complete suggestion from the trusted
  local snapshot.
- Run each request as a durable, non-restartable server job. Progress, cancel
  state, failures, and a completed draft survive browser navigation, refresh,
  and reopening. After a server restart an uncertain model call is not repeated
  automatically because that could duplicate cost.
- Store only a suggestion draft. The model job cannot create or update a
  playlist. Selected songs still pass through the existing versioned Authoring
  import preview and explicit create-only commit.
- Do not silently replace a failed model request with local output. The interface
  may offer an explicit local retry so the provenance of every draft stays clear.

## Options considered

### Replace the existing suggestion endpoint with a provider switch

This would make the local default less predictable and mix a short deterministic
request with durable paid work. Rejected.

### Treat quality certification as standing consent

Certification tests capability with synthetic data. It does not authorize
sharing private library metadata or incurring cost for a later request. Rejected.

### Automatically restart interrupted model jobs

This improves apparent resilience but can repeat a provider request after an
uncertain interruption. Rejected until provider calls have reliable idempotency
or checkpoint semantics.

### Separate consent-bound job over the existing hybrid planner

This preserves local privacy and source-integrity boundaries, gives long work a
durable lifecycle, and keeps the established review-to-import write path.
Selected.

## Consequences

- Closing the page does not cancel server work, and reopening can restore its
  progress or completed draft.
- A user must deliberately select model planning and confirm the current
  disclosure for each new request.
- Changing provider, model, timeout, output limit, or relevant connection state
  invalidates readiness until conformance and quality pass again.
- A failed or interrupted job may still have incurred provider cost. Retry and
  local fallback remain explicit operator decisions.
- Passing a synthetic suite is a safety gate, not a guarantee of subjective
  playlist quality.
