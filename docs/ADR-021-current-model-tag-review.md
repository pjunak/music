# ADR-021: Current model-tag review and atomic acceptance

**Status:** Accepted

**Date:** 2026-09-05

## Problem

The rewritten server stored model-generated mood profiles and exposed a model
review query, but the shared review projection and storage allowlist admitted only
local metadata and catalog analyzers. Model suggestions therefore disappeared from
the review list, and an explicit review request could not accept them. Existing
synthetic provider tests did not exercise this final review path.

## Decision

Compose the application Assistant service with the existing provider and local
analysis services. Listing model suggestions requires their source signature to
match the current role runtime fingerprint, vocabulary fingerprint, indexed model
evidence, and current local-context identity. Validate the stored profile's shape,
confidence, numeric bounds, and output contract before exposing it. Existing local
and catalog review behavior remains under its own source rules.

Reviewing stored output is a local operation. Reading role identity does not load
credentials, execute the model, or require that the role be enabled for new work.
It does not grant provider access. Changed configurations invalidate the affected
generated suggestions through the existing fingerprint contract.

For model decisions, pass a typed internal guard from the application service to
the repository. Within the same SQLite transaction as the decision and manual-tag
write, compare current role configuration, connection fingerprint, and vocabulary
with that guard; reload the track and its current context; recompute the proposal
source signature; validate the stored profile and selected canonical tag. The
browser cannot provide this guard. A configuration or evidence change after the
page or guard was loaded returns a stale result without adding a manual tag.

Reuse the context reader used by model-profile persistence so write and review
paths agree on voice/runtime identity and malformed or missing context. Acceptance
adds only explicitly selected tags. Rejecting or reopening a suggestion preserves
manual tags, including tags previously accepted. Reopening removes the decision
and returns the current suggestion to pending review.

The generic deterministic planner still uses the local-only projection. Restoring
the review screen does not automatically promote model suggestions into local
playlist inputs. Manual-tag patch responses retain the current review projection.

## Validation and limits

- An HTTP regression starts the real runtime with synthetic local media, stores
  a model profile, queries pending/accepted/rejected proposals through the existing
  route, applies review decisions, and verifies vocabulary invalidation and manual
  tag preservation. No provider request is made.
- SQLite regressions mutate role, credential, vocabulary, metadata, local context,
  and stored profile after guard creation. Missing guards and malformed profiles
  also fail closed. These exercise the transaction, not only a preflight helper.
- Public HTTP/WebSocket DTOs, database schema, and Baton contracts are unchanged.
- Affected model fingerprints change, so old generated profiles and certification
  need regeneration/evaluation. This repair does not recover a profile whose
  current evidence or runtime identity no longer matches.
- [ADR-022](ADR-022-current-suggestion-review-metrics.md) subsequently added current
  operator review metrics. The independent held-out quality study remains follow-up
  work; manual acceptance is not a verified training label by default.
