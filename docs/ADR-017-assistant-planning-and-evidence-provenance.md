# ADR-017: Bound request plans and revision-bound catalog evidence

Status: accepted, 2026-09-05.

## Context

The rewrite retained strict model output validation, review-only proposals, and
encrypted credentials. Audit probes nevertheless exposed three gaps: a catalog
worker could restore evidence after invalidation, vocabulary edits did not expire
catalog mappings, and legal vocabularies could exceed the provider envelope limit.
Separately, a tagging response with the correct track set could fail solely because
its items were reordered.

## Decision

Keep the eight-crate architecture and move request partitioning into a typed
application planner. The provider adapter supplies exact serialization validation.
Preview, start preflight, execution, and evaluation use this same planner, including
the corrective request. Force runs obtain their request estimates from the server.
The full vocabulary remains present and each batch has at most 20 tracks. An
oversized single-track request is rejected before a live job is enqueued.

Track identity is a unique set, not array order. Duplicate, absent, additional,
unknown, or otherwise invalid identifiers still fail closed. Retry limits and
quality thresholds are unchanged.

Catalog runs hold a read lease on source settings through requests and persistence.
Setting and credential writes attempt an exclusive lease and fail promptly with a
conflict while a lookup is active. A source edit therefore cannot report success
while an old worker continues using its credentials. Name verification uses the
same lease; queued jobs acquire the current settings on execution.

Database schema 10 introduces a monotonic evidence revision. Vocabulary, source,
credential, and vault-reset mutations increment it and clear regenerable catalog
evidence atomically. Cache and analysis writes compare their captured revision
inside the write transaction. Review signatures bind metadata, mapper identity,
and revision. Accepted/manual tags remain authored state and survive invalidation.

One provider deadline covers the pinned DNS lookup and the complete HTTP response.
Existing request concurrency, redirect restrictions, address checks, and response
size limits remain enforced.

## Consequences and acceptance

- Operators finish or cancel an active catalog lookup before editing its sources.
- Existing catalog proposals expire once during migration and can be regenerated.
- Smaller request batches may increase provider request counts; previews disclose
  those counts instead of assuming every batch contains 20 tracks.
- Changed role runtime source invalidates its saved model quality/conformance results.
  Shared policy changes invalidate all affected roles. Configured models and Thinking
  are retained; unknown roles use the complete executable fingerprint.
- Tests cover source changes across service clones, stale writes after settings are
  changed back, vocabulary edits with a subsequent fresh proposal, stale review
  rejection, authored-tag preservation, unordered responses, complete request
  partitioning, all five adapter envelopes, and a resolver that never returns.

## Compatibility gate

`music-cli contracts check --root .` checks generated-file freshness and the exact
semantic changes from the frozen reference. `contracts/openapi-compatibility-review.json`
records a reason and SHA-256 of each reviewed difference. Regenerating artifacts
does not approve a difference. Changed response shapes on an already listed route
still fail. Missing operations and changed authentication requirements fail outright.
The raw compatibility report continues to describe differences from the historical
reference; a reviewed difference does not rewrite that reference.

## Follow-up architecture

The application planner establishes a reusable boundary without a new service or
agent framework. Model feature jobs and quality evaluations now live in application use cases behind
the `StructuredModelTransport` port; the server composes its bounded HTTP adapter. The four
role execution modules have explicit certification source closures, with shared policy
remaining fail-closed and a test enforcing source/suite inventory coverage.
[ADR-018](ADR-018-derived-model-schemas-and-catalog-ports.md) completes the next step:
derived static output schemas, request-specific identifier and numeric checks, and
application-owned catalog orchestration with behavior and compatibility tests.

Held-out human-labelled evaluations and physical playback acceptance remain separate
evidence from synthetic tests. They do not justify weakening gates or changing the
operator's chosen provider, model, or Thinking configuration.
