# ADR-002: Bounded model execution and per-role conformance

**Status:** Accepted

**Date:** 2026-08-19
**Decider:** Project owner

## Context

Provider connections and task roles can be configured without proving that a
chosen model accepts requests or follows the machine-readable contracts needed
by future playlist, tagging, cleanup, EQ, and audio tools. A general prompt API
would make experimentation easy, but it would also bypass task-specific data
minimization, output validation, review, and authoring boundaries.

## Decision

- Put DNS pinning, private-destination policy, TLS, authorization, timeouts,
  request limits, response limits, JSON decoding, and redirect rejection in one
  transport shared by connection verification and model execution.
- Keep provider request shapes behind adapter functions. The initial
  `openai-compatible/v1` execution adapter calls `/chat/completions`; future
  adapter differences do not leak into roles or Assistant features.
- Normalize successful model output to one JSON object plus bounded model,
  finish-reason, and usage metadata. Markdown-wrapped JSON, arrays, prose, and
  malformed provider envelopes fail closed with safe error codes. Raw prompts,
  responses, credentials, and upstream error bodies are not returned or logged.
- Do not expose the reusable execution function as an HTTP prompt endpoint.
  The authenticated API exposes only a fixed conformance action for a saved
  role. It sends a random one-time challenge and synthetic contract identifier,
  then requires the model to copy both exactly in a three-field JSON object.
- Bind a passing conformance result to a fingerprint of the connection secret
  and network settings, role, model, timeout, and output-token limit. Changing
  any runtime input or explicitly re-verifying the connection clears the result.
  A role is effective only while its connection is verified, its encrypted
  credential is readable, it is enabled, and its current fingerprint has passed.
- Make no automatic retries. A retry may duplicate cost or provider-side work;
  task-specific retry and idempotency policies belong to future durable jobs.
- Keep calls off the FastAPI event loop. The short setup test uses a worker
  thread; long-running feature work must use the existing durable job runner.

## Options considered

### Enable any model after `/models` succeeds

This proves credential access and model discovery, but not the request endpoint
or structured-output behavior. Rejected as too weak a gate for machine-consumed
results.

### Browser-facing general prompt playground

Useful for ad hoc testing, but it creates an unreviewed path for arbitrary data
disclosure and makes feature-specific validation optional. Rejected.

### Provider-specific clients inside each feature

Initially direct, but it duplicates network protections and couples playlist,
tagging, and cleanup code to provider quirks. Rejected.

### Shared transport, adapter execution, and fingerprinted role test

Adds one explicit setup step while preserving provider neutrality and a clear
boundary for later feature schemas. Selected.

## Consequences

- Connection verification and model conformance are intentionally different:
  the first lists accessible models; the second spends a small model request to
  prove the exact assignment can return strict structured data.
- A provider or model that lists successfully may still fail the model test.
  The UI keeps the failure visible and the task disabled without affecting local
  analysis or authoring.
- Conformance establishes transport and basic instruction following, not
  playlist quality, music understanding, privacy suitability, or truthfulness.
  Each model-backed feature still needs a narrow data contract, local schema
  validation, synthetic evaluation, operator disclosure, and review-first commit
  path before integration.
- No real library data leaves the server in this slice.
